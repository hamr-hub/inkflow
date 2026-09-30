//! Raw Linux FFI — libc symbols + DRM/fb0 ioctl numbers and structs.
//!
//! Pulled into its own module so the backends (`memory`, `fb`, `drm`) only
//! pull in the slice they actually need, and so the ioctl constant naming
//! stays close to the structures it goes with.

#![allow(non_camel_case_types)]

use std::os::unix::io::RawFd;

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
pub(super) unsafe fn map_failed(p: *mut std::ffi::c_void) -> bool {
    p as isize == -1
}

#[inline]
pub(super) unsafe fn libc_ioctl(fd: RawFd, req: i64, arg: *mut std::ffi::c_void) -> i32 {
    ioctl(fd, req, arg)
}

#[inline]
pub(super) unsafe fn libc_mmap(
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
pub(super) unsafe fn libc_munmap(addr: *mut std::ffi::c_void, len: usize) -> i32 {
    munmap(addr, len)
}

// ----- DRM -----

// DRM constants — see <drm.h> / <drm_mode.h>
const DRM_IOCTL_BASE: u64 = 0x64;
// 'd'=0x64; standard Linux _IOWR(type, nr, size):
// (3 << 30) | (size << 16) | (type << 8) | nr. Adding base+nr (the
// previous encoding) produced a garbage ioctl number the kernel
// rejected, so the dumb-buffer path never opened.
const fn drm_iowr(cmd: u64, _size: usize) -> i64 {
    ((3u64 << 30) | ((_size as u64) << 16) | ((DRM_IOCTL_BASE) << 8) | cmd) as i64
}
pub(super) const DRM_IOCTL_MODE_CREATE_DUMB: i64 =
    drm_iowr(0xB2, std::mem::size_of::<drm_mode_create_dumb>());
pub(super) const DRM_IOCTL_MODE_MAP_DUMB: i64 =
    drm_iowr(0xB3, std::mem::size_of::<drm_mode_map_dumb>());
pub(super) const DRM_IOCTL_MODE_DESTROY_DUMB: i64 =
    drm_iowr(0xB4, std::mem::size_of::<drm_mode_destroy_dumb>());

#[repr(C)]
pub(super) struct drm_mode_create_dumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}

#[repr(C)]
pub(super) struct drm_mode_map_dumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

#[repr(C)]
pub(super) struct drm_mode_destroy_dumb {
    pub handle: u32,
}

// ----- /dev/fb0 -----

// FBIOGET_VSCREENINFO is 0x4600 (0x4601 is the PUT command — querying with it
// and a zeroed struct tried to set a mode).
pub(super) const FBIOGET_VSCREENINFO: i64 = 0x4600;
pub(super) const FBIOGET_FSCREENINFO: i64 = 0x4602;

#[repr(C)]
#[derive(Default)]
pub(super) struct libc_fb_vscreeninfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub red: fb_bitfield,
    pub green: fb_bitfield,
    pub blue: fb_bitfield,
    pub transp: fb_bitfield,
    pub nonstd: u32,
    pub activate: u32,
    pub height: u32,
    pub width: u32,
    pub accel_flags: u32,
    pub pixclock: u32,
    pub left_margin: u32,
    pub right_margin: u32,
    pub upper_margin: u32,
    pub lower_margin: u32,
    pub hsync_len: u32,
    pub vsync_len: u32,
    pub sync: u32,
    pub vmode: u32,
    pub rotate: u32,
    pub colorspace: u32,
    pub reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default, Copy, Clone)]
pub(super) struct fb_bitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

#[repr(C)]
pub(super) struct libc_fb_fix_screeninfo {
    pub id: [u8; 16],
    pub smem_start: u64,
    pub smem_len: u32,
    pub type_: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub mmio_start: u64,
    pub mmio_len: u32,
    pub accel: u32,
    pub reserved: [u16; 3],
}
