//! `/dev/fb0` Linux framebuffer backend.
//!
//! The driver owns the modeset and scans out on every vsync with no DRM
//! master, so an unprivileged process can light the panel directly. This is
//! the preferred backend — it doesn't need CAP_SYS_ADMIN or a privileged
//! open of the card.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

use super::ffi::{
    libc_fb_fix_screeninfo, libc_fb_vscreeninfo, libc_ioctl, libc_mmap, map_failed,
    FBIOGET_FSCREENINFO, FBIOGET_VSCREENINFO,
};
use super::{Map, Surface};

pub(super) fn open(_width: u32, _height: u32, path: &str) -> std::io::Result<Surface> {
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
    // The paint path writes packed 0x00RRGGBB `u32`s, so the panel must be
    // 32 bpp. Anything else (16/24 bpp, packed RGB565, …) has a different
    // channel order and this renderer cannot drive it — fail loudly rather
    // than scan out garbage.
    if vinfo.bits_per_pixel != 32 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "{} is {}bpp; inkflow needs 32bpp",
                path, vinfo.bits_per_pixel
            ),
        ));
    }
    // Drivers pad rows to an alignment boundary, so the mapping pitch can
    // be wider than `sw` pixels. Honour it instead of assuming contiguity.
    if finfo.line_length < sw * 4 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{}: line_length {} shorter than {} px of 32bpp",
                path, finfo.line_length, sw
            ),
        ));
    }
    let pitch = finfo.line_length / 4;
    let map_bytes = (finfo.line_length as usize) * (sh as usize);
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
    // `Map` owns the mapping from here on; seed `pixels` from whatever the
    // panel already shows so the first present blends into live content
    // instead of flashing black.
    let map = Map {
        ptr,
        len: map_bytes,
        pitch,
    };
    let mut pixels = vec![0u32; (sw * sh) as usize];
    super::copy_rows_from_map(pixels.as_mut_slice(), sw, sh, &map);
    Ok(Surface {
        width: sw,
        height: sh,
        stride: sw,
        pixels,
        backend: super::Backend::Framebuffer,
        fb_file: Some(f),
        drm_file: None,
        drm_handle: 0,
        fb_id: 0,
        map,
    })
}
