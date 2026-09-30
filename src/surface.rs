//! Framebuffer abstraction — three backends: memory, /dev/fb0, and DRM dumb buffer.
//!
//! The render path writes RGB u32 pixels (0x00RRGGBB) into a packed `Vec<u32>`,
//! then either keeps it in memory, blits it to the Linux framebuffer, or hands
//! it to a mapped DRM dumb buffer. All three paths share the same `Surface` API.

mod drm;
mod fb;
mod ffi;

use std::fs::File as StdFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Memory,
    Framebuffer,
    DrmDumb,
}

pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub stride: u32, // pixels per row in `pixels`
    pub pixels: Vec<u32>,
    pub backend: Backend,
    // native handle (kept for the lifetime of the surface)
    pub fb_file: Option<StdFile>,
    pub drm_file: Option<StdFile>,
    pub drm_handle: u32, // GEM handle for the dumb buffer
    pub fb_id: u32,      // DRM framebuffer id (for cleanup)
    // Persistent write-side mapping. Held for the surface's whole lifetime so
    // `present` is a plain memcpy instead of mmap+copy+munmap per frame; the
    // kernel mapping is released in `Drop`.
    map: Map,
}

/// A kernel mapping plus the row pitch it expects.
pub(super) struct Map {
    ptr: *mut std::ffi::c_void,
    len: usize,
    /// Pixels per row in the mapped region — can exceed `width` when the
    /// driver pads rows to an alignment boundary.
    pitch: u32,
}

impl Map {
    const NONE: Self = Self {
        ptr: std::ptr::null_mut(),
        len: 0,
        pitch: 0,
    };
    fn is_armed(&self) -> bool {
        !self.ptr.is_null()
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        if self.is_armed() {
            unsafe { ffi::libc_munmap(self.ptr, self.len) };
            self.ptr = std::ptr::null_mut();
            self.len = 0;
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // Release the GEM dumb buffer so the driver reclaims the pages; the
        // `Map` field's own `Drop` releases the mmap right after.
        if self.backend == Backend::DrmDumb && self.drm_handle != 0 {
            if let Some(d) = &self.drm_file {
                let mut db: ffi::drm_mode_destroy_dumb = unsafe { std::mem::zeroed() };
                db.handle = self.drm_handle;
                unsafe {
                    ffi::libc_ioctl(
                        d.as_raw_fd(),
                        ffi::DRM_IOCTL_MODE_DESTROY_DUMB,
                        (&mut db as *mut ffi::drm_mode_destroy_dumb) as *mut std::ffi::c_void,
                    )
                };
            }
            self.drm_handle = 0;
        }
    }
}

use std::os::unix::io::AsRawFd;

impl Surface {
    pub fn memory(width: u32, height: u32) -> Self {
        let stride = width;
        let pixels = vec![0u32; (stride * height) as usize];
        Self {
            width,
            height,
            stride,
            pixels,
            backend: Backend::Memory,
            fb_file: None,
            drm_file: None,
            drm_handle: 0,
            fb_id: 0,
            map: Map::NONE,
        }
    }

    pub fn try_framebuffer(_width: u32, _height: u32, path: &str) -> std::io::Result<Self> {
        fb::open(_width, _height, path)
    }

    pub fn try_drm_dumb(width: u32, height: u32, card_path: &str) -> std::io::Result<Self> {
        drm::open(width, height, card_path)
    }

    /// Push the in-memory pixels into the underlying display (if any).
    ///
    /// A no-op for [`Backend::Memory`]. Both hardware backends share one
    /// persistent mapping taken at open time, so this is a row-by-row memcpy
    /// with no syscall in the frame loop.
    pub fn present(&self) -> std::io::Result<()> {
        if self.backend == Backend::Memory {
            return Ok(());
        }
        if !self.map.is_armed() {
            return Err(std::io::Error::other("surface has no display mapping"));
        }
        copy_rows_to_map(&self.pixels, self.width, self.height, &self.map);
        Ok(())
    }

    /// Write the pixels as 24-bit RGB to a PNG file at `path`.
    pub fn write_png(&self, path: &str) -> std::io::Result<()> {
        let mut rgb = vec![0u8; self.pixels.len() * 3];
        for (src, dst) in self.pixels.iter().zip(rgb.chunks_exact_mut(3)) {
            dst[0] = (src >> 16) as u8;
            dst[1] = (src >> 8) as u8;
            dst[2] = *src as u8;
        }
        let bytes = crate::png::encode_rgb(self.width, self.height, &rgb);
        let mut f = StdFile::create(path)?;
        std::io::Write::write_all(&mut f, &bytes)?;
        std::io::Write::flush(&mut f)
    }
}

/// Copy `h` rows of `w` pixels from the mapping into a packed row-major vec.
/// A no-op when the mapping is unarmed or its pitch doesn't cover the rows.
pub(super) fn copy_rows_from_map(pixels: &mut [u32], w: u32, h: u32, map: &Map) {
    if !map.is_armed() || map.pitch < w {
        return;
    }
    let row_bytes = (w as usize) * 4;
    let spare = ((map.pitch - w) as usize) * 4;
    let base = map.ptr as *const u8;
    for y in 0..h as usize {
        // SAFETY: the mapping covers `map.len >= (pitch * h) * 4` bytes and
        // `pixels` holds `w * h` u32s, so both row spans stay in bounds.
        unsafe {
            let src = base.add(y * (row_bytes + spare));
            let dst = pixels.as_mut_ptr().add(y * w as usize);
            std::ptr::copy_nonoverlapping(src, dst as *mut u8, row_bytes);
        }
    }
}

/// Copy `h` rows of `w` packed pixels out to the mapping, honouring its pitch.
fn copy_rows_to_map(pixels: &[u32], w: u32, h: u32, map: &Map) {
    if !map.is_armed() || map.pitch < w {
        return;
    }
    let row_bytes = (w as usize) * 4;
    let spare = ((map.pitch - w) as usize) * 4;
    let base = map.ptr as *mut u8;
    for y in 0..h as usize {
        // SAFETY: same bound argument as `copy_rows_from_map`, reversed.
        unsafe {
            let dst = base.add(y * (row_bytes + spare));
            let src = pixels.as_ptr().add(y * w as usize);
            std::ptr::copy_nonoverlapping(src as *const u8, dst, row_bytes);
        }
    }
}
