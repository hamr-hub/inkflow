//! Framebuffer abstraction — three backends: memory, /dev/fb0, and DRM dumb buffer.
//!
//! The render path writes RGB u32 pixels (0x00RRGGBB) into a packed `Vec<u32>`,
//! then either keeps it in memory, blits it to the Linux framebuffer, or hands
//! it to a mapped DRM dumb buffer. All three paths share the same `Surface` API.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::{AsRawFd, RawFd};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Memory,
    Framebuffer,
    DrmDumb,
}

pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub stride: u32, // pixels per row
    pub pixels: Vec<u32>,
    pub backend: Backend,
    // native handle (kept for the lifetime of the surface)
    pub fb_file: Option<File>,
    pub drm_file: Option<File>,
    pub drm_handle: u32, // GEM handle for the dumb buffer
    pub fb_id: u32,      // DRM framebuffer id (for cleanup)
}

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
        }
    }

    pub fn try_framebuffer(width: u32, height: u32, path: &str) -> std::io::Result<Self> {
        let f = OpenOptions::new().read(true).write(true).open(path)?;
        // Query screen info: vscreeninfo tells us xres/yres.
        let mut vinfo: libc_fb_vscreeninfo = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc_ioctl(
                f.as_raw_fd(),
                FBIOGET_VSCREENINFO,
                (&mut vinfo as *mut libc_fb_vscreeninfo) as *mut std::ffi::c_void,
            )
        };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sw = vinfo.xres.max(1) as u32;
        let sh = vinfo.yres.max(1) as u32;
        // Query fix screen info: line_length, smem_len, type.
        let mut finfo: libc_fb_fix_screeninfo = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc_ioctl(
                f.as_raw_fd(),
                FBIOGET_FSCREENINFO,
                (&mut finfo as *mut libc_fb_fix_screeninfo) as *mut std::ffi::c_void,
            )
        };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // We treat the fb as 32bpp regardless of what the driver says: re-mmap.
        let bpp: u32 = 32;
        let bytes_per_row = sw * (bpp / 8);
        let map_bytes = (bytes_per_row * sh) as usize;
        // mmap the framebuffer
        let ptr = unsafe {
            libc_mmap(
                std::ptr::null_mut(),
                map_bytes,
                3, // PROT_READ | PROT_WRITE
                1, // MAP_SHARED
                f.as_raw_fd(),
                0,
            )
        };
        if unsafe { map_failed(ptr) } {
            return Err(std::io::Error::last_os_error());
        }
        // Copy pixels out of the fb into our Vec, then the caller renders, then we blit back.
        let mut pixels = vec![0u32; (sw * sh) as usize];
        unsafe {
            std::ptr::copy_nonoverlapping(
                ptr as *const u8,
                pixels.as_mut_ptr() as *mut u8,
                map_bytes,
            );
        }
        let _ = (width, height); // ignore requested dims on real fb
        Ok(Self {
            width: sw,
            height: sh,
            stride: sw,
            pixels,
            backend: Backend::Framebuffer,
            fb_file: Some(f),
            drm_file: None,
            drm_handle: 0,
            fb_id: 0,
        })
    }

    pub fn try_drm_dumb(width: u32, height: u32, card_path: &str) -> std::io::Result<Self> {
        let drm = OpenOptions::new().read(true).write(true).open(card_path)?;
        let fd = drm.as_raw_fd();
        unsafe {
            // Create dumb buffer.
            let mut cb: drm_mode_create_dumb = std::mem::zeroed();
            cb.height = height;
            cb.width = width;
            cb.bpp = 32;
            let r = libc_ioctl(
                fd,
                DRM_IOCTL_MODE_CREATE_DUMB,
                (&mut cb as *mut drm_mode_create_dumb) as *mut std::ffi::c_void,
            );
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let handle = cb.handle;
            let pitch = cb.pitch;
            // Map it.
            let mut mb: drm_mode_map_dumb = std::mem::zeroed();
            mb.handle = handle;
            let r = libc_ioctl(
                fd,
                DRM_IOCTL_MODE_MAP_DUMB,
                (&mut mb as *mut drm_mode_map_dumb) as *mut std::ffi::c_void,
            );
            if r != 0 {
                // destroy the dumb on failure
                let mut db: drm_mode_destroy_dumb = std::mem::zeroed();
                db.handle = handle;
                libc_ioctl(
                    fd,
                    DRM_IOCTL_MODE_DESTROY_DUMB,
                    (&mut db as *mut drm_mode_destroy_dumb) as *mut std::ffi::c_void,
                );
                return Err(std::io::Error::last_os_error());
            }
            let map_bytes = (pitch as usize) * (height as usize);
            let ptr = libc_mmap(std::ptr::null_mut(), map_bytes, 3, 1, fd, mb.offset as i64);
            if map_failed(ptr) {
                let mut db: drm_mode_destroy_dumb = std::mem::zeroed();
                db.handle = handle;
                libc_ioctl(
                    fd,
                    DRM_IOCTL_MODE_DESTROY_DUMB,
                    (&mut db as *mut drm_mode_destroy_dumb) as *mut std::ffi::c_void,
                );
                return Err(std::io::Error::last_os_error());
            }
            // Copy current contents of the dumb buffer into our pixel vec.
            let mut pixels = vec![0u32; (width * height) as usize];
            std::ptr::copy_nonoverlapping(
                ptr as *const u8,
                pixels.as_mut_ptr() as *mut u8,
                map_bytes.min(pixels.len() * 4),
            );
            Ok(Self {
                width,
                height,
                stride: width,
                pixels,
                backend: Backend::DrmDumb,
                fb_file: None,
                drm_file: Some(drm),
                drm_handle: handle,
                fb_id: 0,
            })
        }
    }

    /// Push the in-memory pixels into the underlying framebuffer (if any).
    pub fn present(&self) -> std::io::Result<()> {
        match self.backend {
            Backend::Framebuffer => {
                // Need to mmap again — simpler: write via mmap from our pixel vec back to /dev/fb0.
                // Since we don't keep the mapping here, just do a one-off mmap+copy.
                if let Some(f) = &self.fb_file {
                    let fd = f.as_raw_fd();
                    let bytes = (self.width * self.height * 4) as usize;
                    let ptr = unsafe { libc_mmap(std::ptr::null_mut(), bytes, 3, 1, fd, 0) };
                    if unsafe { map_failed(ptr) } {
                        return Err(std::io::Error::last_os_error());
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            self.pixels.as_ptr() as *const u8,
                            ptr as *mut u8,
                            bytes,
                        );
                        libc_munmap(ptr, bytes);
                    }
                }
                Ok(())
            }
            Backend::DrmDumb => {
                // Similar: mmap and copy.
                if let Some(d) = &self.drm_file {
                    let fd = d.as_raw_fd();
                    let mut mb = unsafe { std::mem::zeroed::<drm_mode_map_dumb>() };
                    mb.handle = self.drm_handle;
                    let r = unsafe {
                        libc_ioctl(
                            fd,
                            DRM_IOCTL_MODE_MAP_DUMB,
                            (&mut mb as *mut drm_mode_map_dumb) as *mut std::ffi::c_void,
                        )
                    };
                    if r != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let bytes = (self.stride * self.height * 4) as usize;
                    let ptr = unsafe {
                        libc_mmap(std::ptr::null_mut(), bytes, 3, 1, fd, mb.offset as i64)
                    };
                    if unsafe { map_failed(ptr) } {
                        return Err(std::io::Error::last_os_error());
                    }
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            self.pixels.as_ptr() as *const u8,
                            ptr as *mut u8,
                            bytes,
                        );
                        libc_munmap(ptr, bytes);
                    }
                }
                Ok(())
            }
            Backend::Memory => Ok(()),
        }
    }

    /// Write the pixels as 24-bit RGB to a PNG file at `path`.
    pub fn write_png(&self, path: &str) -> std::io::Result<()> {
        let mut rgb = Vec::with_capacity(self.pixels.len() * 3);
        for &p in &self.pixels {
            rgb.push(((p >> 16) & 0xFF) as u8);
            rgb.push(((p >> 8) & 0xFF) as u8);
            rgb.push((p & 0xFF) as u8);
        }
        let bytes = crate::png::encode_rgb(self.width, self.height, &rgb);
        let mut f = File::create(path)?;
        f.write_all(&bytes)?;
        f.flush()
    }
}

// ============================================================
// Raw FFI — libc symbols + Linux DRM/FB ioctl numbers.
// ============================================================

extern "C" {
    fn ioctl(fd: i32, req: i64, ...) -> i32;
    fn mmap(
        addr: *mut std::ffi::c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut std::ffi::c_void;
    fn munmap(addr: *mut std::ffi::c_void, len: usize) -> i32;
}

/// `MAP_FAILED` is `((void *) -1)` on Linux/musl/glibc. We define a helper
/// rather than depending on the libc crate (zero-dep target).
#[inline]
unsafe fn map_failed(p: *mut std::ffi::c_void) -> bool {
    p as isize == -1
}

#[inline]
unsafe fn libc_ioctl(fd: RawFd, req: i64, arg: *mut std::ffi::c_void) -> i32 {
    ioctl(fd, req, arg)
}
#[inline]
unsafe fn libc_mmap(
    addr: *mut std::ffi::c_void,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    offset: i64,
) -> *mut std::ffi::c_void {
    mmap(addr, len, prot, flags, fd, offset)
}
#[inline]
unsafe fn libc_munmap(addr: *mut std::ffi::c_void, len: usize) -> i32 {
    munmap(addr, len)
}

// DRM constants — see <drm.h> / <drm_mode.h>
const DRM_IOCTL_BASE: u64 = 0x64;
// 'd'=0x64; standard Linux _IOWR(type, nr, size):
// (3 << 30) | (size << 16) | (type << 8) | nr. Adding base+nr (the
// previous encoding) produced a garbage ioctl number the kernel
// rejected, so the dumb-buffer path never opened.
const fn drm_iowr(cmd: u64, _size: usize) -> i64 {
    ((3u64 << 30) | ((_size as u64) << 16) | ((DRM_IOCTL_BASE) << 8) | cmd) as i64
}
const DRM_IOCTL_MODE_CREATE_DUMB: i64 = drm_iowr(0xB2, std::mem::size_of::<drm_mode_create_dumb>());
const DRM_IOCTL_MODE_MAP_DUMB: i64 = drm_iowr(0xB3, std::mem::size_of::<drm_mode_map_dumb>());
const DRM_IOCTL_MODE_DESTROY_DUMB: i64 =
    drm_iowr(0xB4, std::mem::size_of::<drm_mode_destroy_dumb>());

#[repr(C)]
struct drm_mode_create_dumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}
#[repr(C)]
struct drm_mode_map_dumb {
    handle: u32,
    pad: u32,
    offset: u64,
}
#[repr(C)]
struct drm_mode_destroy_dumb {
    handle: u32,
}

// /dev/fb0 ioctls. FBIOGET_VSCREENINFO is 0x4600 (0x4601 is the PUT
// command — querying with it and a zeroed struct tried to set a mode).
const FBIOGET_VSCREENINFO: i64 = 0x4600;
const FBIOGET_FSCREENINFO: i64 = 0x4602;

#[repr(C)]
#[derive(Default)]
struct libc_fb_vscreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: fb_bitfield,
    green: fb_bitfield,
    blue: fb_bitfield,
    transp: fb_bitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}
#[repr(C)]
#[derive(Default, Copy, Clone)]
struct fb_bitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}
#[repr(C)]
struct libc_fb_fix_screeninfo {
    id: [u8; 16],
    smem_start: u64,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: u64,
    mmio_len: u32,
    accel: u32,
    reserved: [u16; 3],
}
