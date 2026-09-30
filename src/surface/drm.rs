//! DRM dumb-buffer backend.
//!
//! Allocates a dumb GEM buffer via `DRM_IOCTL_MODE_CREATE_DUMB`, mmaps it,
//! and lets the renderer write straight into the mapped region. Needs DRM
//! master for the eventual modeset (set elsewhere), so this path is the
//! fallback after `/dev/fb0`.

use std::fs::OpenOptions;
use std::os::unix::io::AsRawFd;

use super::ffi::{
    drm_mode_create_dumb, drm_mode_destroy_dumb, drm_mode_map_dumb, libc_ioctl, libc_mmap,
    map_failed, DRM_IOCTL_MODE_CREATE_DUMB, DRM_IOCTL_MODE_DESTROY_DUMB, DRM_IOCTL_MODE_MAP_DUMB,
};
use super::{Map, Surface};

pub(super) fn open(width: u32, height: u32, card_path: &str) -> std::io::Result<Surface> {
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
        if pitch < width {
            let mut db: drm_mode_destroy_dumb = std::mem::zeroed();
            db.handle = handle;
            libc_ioctl(
                fd,
                DRM_IOCTL_MODE_DESTROY_DUMB,
                (&mut db as *mut drm_mode_destroy_dumb) as *mut std::ffi::c_void,
            );
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("drm dumb pitch {pitch} narrower than width {width}"),
            ));
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
        let map = Map {
            ptr,
            len: map_bytes,
            pitch,
        };
        // Copy current contents of the dumb buffer into our pixel vec.
        let mut pixels = vec![0u32; (width * height) as usize];
        super::copy_rows_from_map(pixels.as_mut_slice(), width, height, &map);
        Ok(Surface {
            width,
            height,
            stride: width,
            pixels,
            backend: super::Backend::DrmDumb,
            fb_file: None,
            drm_file: Some(drm),
            drm_handle: handle,
            fb_id: 0,
            map,
        })
    }
}
