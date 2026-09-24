// inkflow · drm.rs
//
// Direct DRM/KMS dumb-buffer surface. Walks /dev/dri/cardN, picks the first
// connected connector + a compatible encoder + CRTC, allocates a dumb
// framebuffer of the chosen mode, mmaps it, and we draw straight into
// 32bpp BGRA from the main loop. No GL, no X, no Wayland.
//
// All ioctls are routed through src/sys.rs. Struct layouts mirror
// /usr/include/drm/drm_mode.h and drm_fourcc.h on Linux 5.15+. The ioctl
// numbers themselves are derived at compile time from `mem::size_of::<T>()`
// using the standard Linux `_IOWR(type, nr, T)` macro:
//
//     (3u32 << 30) | ((size as u32) << 16) | ((type as u32) << 8) | nr as u32
//
// where type is 'd' (0x64) for DRM. Hand-typed constants are forbidden
// here — they were the root cause of the `DRM_IOCTL_VERSION failed: EINVAL`
// regression observed on real aarch64 hardware.

#![allow(dead_code, unused_mut)]

use crate::sys;
use core::ffi::{c_char, c_int, c_uchar};
use core::mem;

// ---------- ioctl encoding ----------

/// Standard Linux `_IOWR(type, nr, size)` macro. `type` is 'd' for DRM (0x64).
const fn iowr(nr: u32, size: usize) -> u64 {
    (3u64 << 30) | ((size as u64) << 16) | (0x64u64 << 8) | (nr as u64)
}

// DRM command numbers (must match /usr/include/drm/drm.h).
const NR_VERSION: u32 = 0x00;
const NR_GETRESOURCES: u32 = 0xA0;
const NR_GETCRTC: u32 = 0xA1;
const NR_SETCRTC: u32 = 0xA2;
const NR_GETENCODER: u32 = 0xA6;
const NR_GETCONNECTOR: u32 = 0xA7;
const NR_ADDFB: u32 = 0xAE;
const NR_RMFB: u32 = 0xAF;
const NR_CREATE_DUMB: u32 = 0xB2;
const NR_MAP_DUMB: u32 = 0xB3;
const NR_DESTROY_DUMB: u32 = 0xB4;

// Computed at compile time from mem::size_of::<T>() so they cannot drift from
// the struct layout. Touch the type below and the magic numbers follow.
const DRM_IOCTL_VERSION: u64 = iowr(NR_VERSION, mem::size_of::<DrmVersion>());
const DRM_IOCTL_MODE_GETRESOURCES: u64 = iowr(NR_GETRESOURCES, mem::size_of::<DrmModeRes>());
const DRM_IOCTL_MODE_GETCRTC: u64 = iowr(NR_GETCRTC, mem::size_of::<DrmModeCrtc>());
const DRM_IOCTL_MODE_SETCRTC: u64 = iowr(NR_SETCRTC, mem::size_of::<DrmModeCrtc>());
const DRM_IOCTL_MODE_GETENCODER: u64 = iowr(NR_GETENCODER, mem::size_of::<DrmModeEncoder>());
const DRM_IOCTL_MODE_GETCONNECTOR: u64 = iowr(NR_GETCONNECTOR, mem::size_of::<DrmModeConnector>());
const DRM_IOCTL_MODE_ADDFB: u64 = iowr(NR_ADDFB, mem::size_of::<DrmModeFbCmd>());
const DRM_IOCTL_MODE_RMFB: u64 = iowr(NR_RMFB, mem::size_of::<u32>());
const DRM_IOCTL_MODE_CREATE_DUMB: u64 = iowr(NR_CREATE_DUMB, mem::size_of::<DrmModeCreateDumb>());
const DRM_IOCTL_MODE_MAP_DUMB: u64 = iowr(NR_MAP_DUMB, mem::size_of::<DrmModeMapDumb>());
const DRM_IOCTL_MODE_DESTROY_DUMB: u64 =
    iowr(NR_DESTROY_DUMB, mem::size_of::<DrmModeDestroyDumb>());

// Pixel formats (kept for reference; we use XRGB8888 via ADDFB)
#[allow(dead_code)]
const DRM_FORMAT_ARGB8888: u32 = 0x34325241; // 'AR24' little-endian
#[allow(dead_code)]
const DRM_FORMAT_XRGB8888: u32 = 0x34325258;
#[allow(dead_code)]
const DRM_FORMAT_BGRA8888: u32 = 0x34324142; // 'BA24'
#[allow(dead_code)]
const DRM_FORMAT_BGRX8888: u32 = 0x34324258;

// ---------- UAPI structs ----------
//
// Field order, padding, and `#[repr(C)]` placement MUST match the Linux
// UAPI headers. The kernel `drm_ioctl` dispatcher reads/writes by offset;
// any drift makes fields land in the wrong slots and the call returns
// `EINVAL` or silently corrupts state.

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmVersion {
    pub version_major: c_int,
    pub version_minor: c_int,
    pub version_patchlevel: c_int,
    // 4 bytes implicit padding before `name_len` so the u64s sit on a
    // 8-byte boundary on 64-bit (this matches the in-memory layout the
    // kernel uses on aarch64 / x86_64).
    pub name_len: usize,
    pub name: *mut c_char,
    pub date_len: usize,
    pub date: *mut c_char,
    pub desc_len: usize,
    pub desc: *mut c_char,
}
// total = 12 (3*int) + 4 (pad) + 6*8 (size_t/ptr pairs) = 64 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeRes {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}
// total = 4*8 + 8*4 = 64 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub type_: u32,
    pub name: [c_uchar; 32],
}
// total = 4 + 10*2 + 4 (vscan -> vrefresh no padding because 24 is 4-aligned)
//        + 4 + 4 + 4 + 32 = 68 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeCrtc {
    pub set_connectors_ptr: u64,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    // Note: gamma_size precedes mode_valid in the UAPI. Do not move.
    pub gamma_size: u32,
    pub mode_valid: u32,
    pub mode: DrmModeModeInfo,
}
// total = 8 + 7*4 + 68 = 104 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeEncoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}
// total = 5*4 = 20 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeConnector {
    pub encoders_ptr: u64,
    pub modes_ptr: u64,
    pub props_ptr: u64,
    pub prop_values_ptr: u64,
    pub count_modes: u32,
    pub count_props: u32,
    pub count_encoders: u32,
    pub encoder_id: u32,
    pub connector_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub pad: u32,
}
// total = 4*8 + 12*4 = 80 bytes (matches the kernel UAPI exactly)

pub const DRM_MODE_CONNECTED: u32 = 1;
pub const DRM_MODE_DISCONNECTED: u32 = 2;
pub const DRM_MODE_UNKNOWNCONNECTION: u32 = 3;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeFbCmd {
    pub fb_id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
    pub depth: u32,
    pub handle: u32,
}
// total = 7*4 = 28 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeCreateDumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}
// total = 6*4 + 8 = 32 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeMapDumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}
// total = 4 + 4 + 8 = 16 bytes

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeDestroyDumb {
    pub handle: u32,
}
// total = 4 bytes

/// Open the first card that responds, allocate a dumb buffer of `w × h`
/// pixels, mmap it, and add an fb for it — but do NOT touch the CRTC.
/// Used by `--drm-test` when the connector is disconnected so we can
/// still prove the create_dumb / map_dumb / addfb ioctl path end-to-end
/// and capture the framebuffer to a PNG.
pub fn open_dumb_only(w: u32, h: u32) -> Result<Display, String> {
    let mut last_err = String::new();
    for n in 0..16 {
        let path = format!("/dev/dri/card{n}");
        let card_fd = match sys::open_rw(&path) {
            Ok(fd) => fd,
            Err(_) => continue,
        };
        match build_dumb_only(card_fd, w, h) {
            Ok(d) => return Ok(d),
            Err(e) => {
                last_err = format!("{path}: {e}");
                continue;
            }
        }
    }
    Err(format!(
        "no /dev/dri/card* supports dumb buffers: {last_err}"
    ))
}

fn build_dumb_only(card_fd: c_int, w: u32, h: u32) -> Result<Display, String> {
    let mut dumb = DrmModeCreateDumb {
        height: h,
        width: w,
        bpp: 32,
        flags: 0,
        handle: 0,
        pitch: 0,
        size: 0,
    };
    sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_CREATE_DUMB, &mut dumb)
        .map_err(|e| format!("CREATE_DUMB: errno={e}"))?;
    let mut fb = DrmModeFbCmd {
        fb_id: 0,
        width: dumb.width,
        height: dumb.height,
        pitch: dumb.pitch,
        bpp: 32,
        depth: 24,
        handle: dumb.handle,
    };
    sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_ADDFB, &mut fb)
        .map_err(|e| format!("ADDFB: errno={e}"))?;
    let mut map_off = DrmModeMapDumb {
        handle: dumb.handle,
        pad: 0,
        offset: 0,
    };
    sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_MAP_DUMB, &mut map_off)
        .map_err(|e| format!("MAP_DUMB: errno={e}"))?;
    let map_size = dumb.size as usize;
    let ptr = sys::map_shared(card_fd, map_size, map_off.offset as i64)
        .map_err(|e| format!("mmap: errno={e}"))?;
    Ok(Display {
        card_fd,
        crtc_id: 0,
        conn_id: 0,
        fb_id: fb.fb_id,
        dumb_handle: dumb.handle,
        pitch: dumb.pitch,
        width: dumb.width,
        height: dumb.height,
        bpp: 32,
        stride: (dumb.pitch / 4) as usize,
        map_ptr: ptr as *mut u32,
        map_size,
        modeset_ok: false,
        saved_crtc: None,
        via_fb0: false,
    })
}

// ---------- public surface ----------

pub struct Display {
    pub card_fd: c_int,
    pub crtc_id: u32,
    pub conn_id: u32,
    pub fb_id: u32,
    pub dumb_handle: u32,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    pub stride: usize,
    pub map_ptr: *mut u32, // 32-bit BGRA pixels
    pub map_size: usize,
    /// True when we actually pushed a mode to a CRTC. False for the
    /// "scanout-less dumb buffer" path used when the connector is
    /// disconnected — we still have an mmap'd fb that we can draw into
    /// and capture, just no real display to scan it out.
    pub modeset_ok: bool,
    /// Saved CRTC state to restore on Drop / --drm-test teardown.
    pub saved_crtc: Option<DrmModeCrtc>,
    /// True for /dev/fb0 surfaces. Suppresses SETCRTC in `present()` and
    /// any ioctl that doesn't apply to a legacy fb device.
    pub via_fb0: bool,
}

impl Display {
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        let len = (self.pitch as usize / 4) * self.height as usize;
        unsafe { core::slice::from_raw_parts_mut(self.map_ptr, len) }
    }
    /// unsafe raw write of the framebuffer slice — caller must own `&mut self`.
    pub fn pixels(&mut self) -> &mut [u32] {
        self.pixels_mut()
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn pitch(&self) -> u32 {
        self.pitch
    }

    pub fn present(&self) {
        // /dev/fb0 surfaces are scanned out by the driver automatically —
        // writes to the mmap'd buffer appear on the next vsync without an
        // extra ioctl, so skip SETCRTC entirely.
        if self.via_fb0 {
            return;
        }
        // legacy SET_CRTC to push the next frame. For a single fixed mode
        // this is plenty fast; can swap to atomic/page-flip later if
        // tearing shows up.
        let mut crtc = DrmModeCrtc {
            crtc_id: self.crtc_id,
            fb_id: self.fb_id,
            mode_valid: 1,
            ..DrmModeCrtc::default()
        };
        let _ = sys::ioctl_struct(self.card_fd, DRM_IOCTL_MODE_SETCRTC, &mut crtc);
    }

    /// Restore the CRTC to whatever it was showing before we took over.
    /// Called from Drop and from the --drm-test teardown.
    pub fn restore(&mut self) {
        if let Some(saved) = self.saved_crtc.take() {
            let mut restore = saved;
            // Force mode_valid=1 + the saved mode so the kernel puts the
            // original fb back. If we never had a saved mode (no modeset
            // occurred) this is a no-op.
            if restore.fb_id == 0 {
                restore.mode_valid = 0;
            } else {
                restore.mode_valid = 1;
            }
            let _ = sys::ioctl_struct(self.card_fd, DRM_IOCTL_MODE_SETCRTC, &mut restore);
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        self.restore();
        if self.fb_id != 0 {
            let mut fb = DrmModeFbCmd {
                fb_id: self.fb_id,
                width: 0,
                height: 0,
                pitch: 0,
                bpp: 0,
                depth: 0,
                handle: 0,
            };
            let _ = sys::ioctl_struct(self.card_fd, DRM_IOCTL_MODE_RMFB, &mut fb);
        }
        if self.dumb_handle != 0 {
            let mut d = DrmModeDestroyDumb {
                handle: self.dumb_handle,
            };
            let _ = sys::ioctl_struct(self.card_fd, DRM_IOCTL_MODE_DESTROY_DUMB, &mut d);
        }
        if !self.map_ptr.is_null() && self.map_size > 0 {
            sys::unmap(self.map_ptr as *mut u8, self.map_size);
            self.map_ptr = core::ptr::null_mut();
        }
        if self.card_fd >= 0 {
            sys::close_fd(self.card_fd);
        }
    }
}

// ---------- bootstrap ----------

/// Probe a single card. Reports whether this card has a usable
/// (connected + modes) path. Used by both `open_first()` and
/// `--drm-test`.
pub fn probe(card_idx: usize) -> Result<ProbeResult, String> {
    let path = format!("/dev/dri/card{card_idx}");
    let card_fd = sys::open_rw(&path).map_err(|e| format!("open {path}: errno={e}"))?;
    let mut r = ProbeResult {
        fd: card_fd,
        path,
        driver: String::new(),
        major: 0,
        minor: 0,
        patch: 0,
        connected: Vec::new(),
    };

    // -- DRM_IOCTL_VERSION --
    let mut ver = DrmVersion {
        version_major: 0,
        version_minor: 0,
        version_patchlevel: 0,
        name_len: 0,
        name: core::ptr::null_mut(),
        date_len: 0,
        date: core::ptr::null_mut(),
        desc_len: 0,
        desc: core::ptr::null_mut(),
    };
    let name_buf = [0u8; 256];
    let date_buf = [0u8; 256];
    let desc_buf = [0u8; 256];
    let _ = (date_buf, desc_buf);
    ver.name_len = name_buf.len();
    ver.date_len = date_buf.len();
    ver.desc_len = desc_buf.len();
    ver.name = name_buf.as_ptr() as *mut c_char;
    ver.date = date_buf.as_ptr() as *mut c_char;
    ver.desc = desc_buf.as_ptr() as *mut c_char;
    sys::ioctl_struct(card_fd, DRM_IOCTL_VERSION, &mut ver).map_err(|e| {
        sys::close_fd(card_fd);
        format!("DRM_IOCTL_VERSION failed: errno={e}")
    })?;
    r.driver = std::str::from_utf8(&name_buf[..name_buf.len().min(ver.name_len as usize)])
        .unwrap_or("")
        .trim_end_matches('\0')
        .to_string();
    r.major = ver.version_major as u32;
    r.minor = ver.version_minor as u32;
    r.patch = ver.version_patchlevel as u32;

    // -- DRM_IOCTL_MODE_GETRESOURCES (probe) --
    let res = query_resources(card_fd)?;
    if res.count_connectors == 0 {
        sys::close_fd(card_fd);
        return Ok(r);
    }

    let mut conn_ids: Vec<u32> = vec![0; res.count_connectors as usize];
    {
        let mut res2 = DrmModeRes {
            connector_id_ptr: conn_ids.as_mut_ptr() as u64,
            ..DrmModeRes {
                count_connectors: res.count_connectors,
                ..DrmModeRes::default()
            }
        };
        let _ = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res2);
    }

    for cid in &conn_ids {
        if let Some(c) = query_connector(card_fd, *cid) {
            if c.connection == DRM_MODE_CONNECTED {
                let mode_count = c.count_modes;
                r.connected
                    .push((c.connector_id, c.connector_type, mode_count));
            }
        }
    }

    sys::close_fd(card_fd);
    Ok(r)
}

pub struct ProbeResult {
    pub fd: c_int,
    pub path: String,
    pub driver: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub connected: Vec<(u32, u32, u32)>, // (connector_id, connector_type, mode_count)
}

pub fn open_first() -> Result<Display, String> {
    // Try each card and use the first one that yields a real modeset
    // path. Returns a Display with `modeset_ok=false` when no card has a
    // connector so the caller still has a drawable surface.
    let mut last_err = String::new();
    for n in 0..16 {
        let path = format!("/dev/dri/card{n}");
        let card_fd = match sys::open_rw(&path) {
            Ok(fd) => fd,
            Err(_) => continue,
        };
        match build_display(card_fd, &path) {
            Ok(d) => return Ok(d),
            Err(e) => {
                last_err = format!("{path}: {e}");
                continue;
            }
        }
    }
    Err(format!("no usable /dev/dri/card*: {last_err}"))
}

/// Build a Display from an already-opened card fd. Walks resources,
/// picks a connected connector, an encoder + crtc, allocates a dumb
/// buffer, mmaps it, addfb's it, and (if a connector is actually
/// connected) issues SETCRTC. The returned Display::modeset_ok tells
/// the caller whether the actual scanout was applied.
fn build_display(card_fd: c_int, path: &str) -> Result<Display, String> {
    log!("drm: {path} (fd {card_fd}) — VERSION…");
    let mut ver = DrmVersion {
        version_major: 0,
        version_minor: 0,
        version_patchlevel: 0,
        name_len: 0,
        name: core::ptr::null_mut(),
        date_len: 0,
        date: core::ptr::null_mut(),
        desc_len: 0,
        desc: core::ptr::null_mut(),
    };
    let name = [0u8; 256];
    let date = [0u8; 256];
    let desc = [0u8; 256];
    let _ = (date, desc);
    ver.name_len = name.len();
    ver.date_len = date.len();
    ver.desc_len = desc.len();
    ver.name = name.as_ptr() as *mut c_char;
    ver.date = date.as_ptr() as *mut c_char;
    ver.desc = desc.as_ptr() as *mut c_char;
    sys::ioctl_struct(card_fd, DRM_IOCTL_VERSION, &mut ver)
        .map_err(|e| format!("DRM_IOCTL_VERSION: errno={e}"))?;
    let nm = std::str::from_utf8(&name[..name.len().min(ver.name_len as usize)])
        .unwrap_or("")
        .trim_end_matches('\0')
        .to_string();
    log!(
        "drm: driver={} version={}.{}.{}",
        nm,
        ver.version_major,
        ver.version_minor,
        ver.version_patchlevel
    );

    // Resources → connector list.
    let res = query_resources(card_fd)?;
    log!(
        "drm: fbs={} crtcs={} connectors={} encoders={}",
        res.count_fbs,
        res.count_crtcs,
        res.count_connectors,
        res.count_encoders
    );
    if res.count_connectors == 0 {
        return Err("no connectors".into());
    }

    let mut conn_ids: Vec<u32> = vec![0; res.count_connectors as usize];
    let mut enc_ids: Vec<u32> = vec![0; res.count_encoders as usize];
    let mut crtc_ids: Vec<u32> = vec![0; res.count_crtcs as usize];
    {
        let mut res2 = DrmModeRes {
            fb_id_ptr: 0,
            crtc_id_ptr: crtc_ids.as_mut_ptr() as u64,
            connector_id_ptr: conn_ids.as_mut_ptr() as u64,
            encoder_id_ptr: enc_ids.as_mut_ptr() as u64,
            count_fbs: res.count_fbs,
            count_crtcs: res.count_crtcs,
            count_connectors: res.count_connectors,
            count_encoders: res.count_encoders,
            min_width: 0,
            max_width: 0,
            min_height: 0,
            max_height: 0,
        };
        let _ = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res2);
    }

    // Pick the first connected connector that has at least one mode.
    let mut chosen_conn: Option<(u32, DrmModeConnector)> = None;
    for cid in &conn_ids {
        let c = query_connector(card_fd, *cid);
        if let Some(c) = c {
            if c.connection == DRM_MODE_CONNECTED && c.count_modes > 0 {
                chosen_conn = Some((*cid, c));
                break;
            }
        }
    }
    let (conn_id, conn) = match chosen_conn {
        Some(x) => x,
        None => return Err("no connected connector".into()),
    };
    log!(
        "drm: connector id={} type={} modes={}",
        conn_id,
        conn.connector_type,
        conn.count_modes
    );

    // Pull modes array.
    let mut modes: Vec<DrmModeModeInfo> =
        vec![DrmModeModeInfo::default(); conn.count_modes as usize];
    {
        let mut c2 = conn;
        c2.modes_ptr = modes.as_mut_ptr() as u64;
        c2.props_ptr = 0;
        c2.encoders_ptr = 0;
        c2.prop_values_ptr = 0;
        sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c2).ok();
    }
    let mode = modes[0];
    log!(
        "drm: mode {}x{} @ {}Hz",
        mode.hdisplay,
        mode.vdisplay,
        mode.vrefresh
    );

    // Pull encoders array.
    let mut enc_arr: Vec<u32> = vec![0; conn.count_encoders as usize];
    {
        let mut c2 = conn;
        c2.encoders_ptr = enc_arr.as_mut_ptr() as u64;
        c2.modes_ptr = modes.as_mut_ptr() as u64;
        c2.props_ptr = 0;
        c2.prop_values_ptr = 0;
        let _ = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c2);
    }

    // Find an encoder that supports this connector and at least one CRTC.
    let mut chosen_enc: Option<u32> = None;
    for &enc_id in &enc_arr[..conn.count_encoders as usize] {
        let mut enc = DrmModeEncoder {
            encoder_id: enc_id,
            ..DrmModeEncoder::default()
        };
        if sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETENCODER, &mut enc).is_err() {
            continue;
        }
        if enc.possible_crtcs == 0 {
            continue;
        }
        for cid in &crtc_ids {
            let mut c = DrmModeCrtc {
                crtc_id: *cid,
                ..DrmModeCrtc::default()
            };
            if sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCRTC, &mut c).is_ok() {
                chosen_enc = Some(enc_id);
                break;
            }
        }
        if chosen_enc.is_some() {
            break;
        }
    }
    let _enc_id = chosen_enc.ok_or("no usable encoder/crtc pair")?;

    // Pick a CRTC that's compatible with the chosen encoder.
    let mut crtc_id = 0u32;
    for cid in &crtc_ids {
        let mut c = DrmModeCrtc {
            crtc_id: *cid,
            ..DrmModeCrtc::default()
        };
        if sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCRTC, &mut c).is_ok() {
            crtc_id = *cid;
            break;
        }
    }
    if crtc_id == 0 {
        return Err("no CRTC available".into());
    }

    // Save existing CRTC state so we can restore on Drop.
    let saved_crtc = {
        let mut c = DrmModeCrtc {
            crtc_id,
            ..DrmModeCrtc::default()
        };
        let _ = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCRTC, &mut c);
        c
    };

    // Allocate the dumb buffer.
    let mut dumb = DrmModeCreateDumb {
        height: mode.vdisplay as u32,
        width: mode.hdisplay as u32,
        bpp: 32,
        flags: 0,
        handle: 0,
        pitch: 0,
        size: 0,
    };
    sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_CREATE_DUMB, &mut dumb)
        .map_err(|e| format!("CREATE_DUMB: errno={e}"))?;
    log!("drm: dumb pitch={} size={}B", dumb.pitch, dumb.size);

    // Add the fb for that dumb buffer.
    let mut fb = DrmModeFbCmd {
        fb_id: 0,
        width: dumb.width,
        height: dumb.height,
        pitch: dumb.pitch,
        bpp: 32,
        depth: 24,
        handle: dumb.handle,
    };
    sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_ADDFB, &mut fb)
        .map_err(|e| format!("ADDFB: errno={e}"))?;
    log!("drm: fb_id={}", fb.fb_id);

    // mmap it.
    let mut map_off = DrmModeMapDumb {
        handle: dumb.handle,
        pad: 0,
        offset: 0,
    };
    sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_MAP_DUMB, &mut map_off)
        .map_err(|e| format!("MAP_DUMB: errno={e}"))?;
    let map_size = dumb.size as usize;
    let ptr = sys::map_shared(card_fd, map_size, map_off.offset as i64)
        .map_err(|e| format!("mmap: errno={e}"))?;
    let map_ptr = ptr as *mut u32;

    // Apply the mode.
    let conn_id_arr = [conn_id];
    let mut crtc_set = DrmModeCrtc {
        set_connectors_ptr: conn_id_arr.as_ptr() as u64,
        count_connectors: 1,
        crtc_id,
        fb_id: fb.fb_id,
        x: 0,
        y: 0,
        gamma_size: 0,
        mode_valid: 1,
        mode,
    };
    let modeset_ok = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_SETCRTC, &mut crtc_set).is_ok();

    log!(
        "drm: mode {}x{} applied={}, fb={}",
        mode.hdisplay,
        mode.vdisplay,
        modeset_ok,
        fb.fb_id
    );

    Ok(Display {
        card_fd,
        crtc_id,
        conn_id,
        fb_id: fb.fb_id,
        dumb_handle: dumb.handle,
        pitch: dumb.pitch,
        width: dumb.width,
        height: dumb.height,
        bpp: 32,
        stride: (dumb.pitch / 4) as usize,
        map_ptr,
        map_size,
        modeset_ok,
        saved_crtc: Some(saved_crtc),
        via_fb0: false,
    })
}

// ---------- internal helpers ----------

pub fn query_resources(fd: c_int) -> Result<DrmModeRes, String> {
    let mut res = DrmModeRes::default();
    sys::ioctl_struct(fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res)
        .map_err(|e| format!("GETRESOURCES: errno={e}"))?;
    Ok(res)
}

pub fn query_connector(fd: c_int, id: u32) -> Option<DrmModeConnector> {
    let mut c = DrmModeConnector {
        connector_id: id,
        ..DrmModeConnector::default()
    };
    if sys::ioctl_struct(fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c).is_err() {
        return None;
    }
    Some(c)
}

// Suppress unused-import noise.
#[allow(dead_code)]
const _USED: (u32, usize) = (mem::size_of::<u32>() as u32, mem::size_of::<usize>());

// Compile-time asserts that our struct sizes match the kernel UAPI on
// aarch64. If anyone changes a field type or order, these trip and the
// ioctl magic numbers are regenerated automatically.
#[allow(dead_code)]
const _: () = {
    assert!(
        mem::size_of::<DrmVersion>() == 64,
        "drm_version must be 64B on aarch64"
    );
    assert!(
        mem::size_of::<DrmModeRes>() == 64,
        "drm_mode_card_res must be 64B"
    );
    assert!(
        mem::size_of::<DrmModeModeInfo>() == 68,
        "drm_mode_modeinfo must be 68B"
    );
    assert!(
        mem::size_of::<DrmModeCrtc>() == 104,
        "drm_mode_crtc must be 104B"
    );
    assert!(
        mem::size_of::<DrmModeEncoder>() == 20,
        "drm_mode_get_encoder must be 20B"
    );
    assert!(
        mem::size_of::<DrmModeConnector>() == 80,
        "drm_mode_get_connector must be 80B"
    );
    assert!(
        mem::size_of::<DrmModeFbCmd>() == 28,
        "drm_mode_fb_cmd must be 28B"
    );
    assert!(
        mem::size_of::<DrmModeCreateDumb>() == 32,
        "drm_mode_create_dumb must be 32B"
    );
    assert!(
        mem::size_of::<DrmModeMapDumb>() == 16,
        "drm_mode_map_dumb must be 16B"
    );
    assert!(
        mem::size_of::<DrmModeDestroyDumb>() == 4,
        "drm_mode_destroy_dumb must be 4B"
    );
};

// ---------- /dev/fb0 legacy framebuffer fallback ----------
//
// When the kernel has a working KMS driver (tegra, i915, amdgpu, nouveau)
// we go through DRM and produce a real scan-out frame. When it doesn't —
// typically on Jetson with the proprietary nvidia-drm driver, which sets
// the mode itself but refuses CREATE_DUMB — the driver still exposes its
// own framebuffer at `/dev/fb0`. Drawing into that surface makes the piece
// visible end-to-end without touching the kernel driver stack or breaking
// the zero-dep contract (we only add three ioctls to sys.rs… well, zero:
// these are the standard Linux FBIOGET_* ioctls, encoded as raw u64s).

pub const FBIOGET_VSCREENINFO: u64 = 0x4600;
pub const FBIOGET_FSCREENINFO: u64 = 0x4602;

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct FbBitfield {
    pub offset: u32,
    pub length: u32,
    pub msb_right: u32,
}

#[repr(C)]
pub struct FbVarScreeninfo {
    pub xres: u32,
    pub yres: u32,
    pub xres_virtual: u32,
    pub yres_virtual: u32,
    pub xoffset: u32,
    pub yoffset: u32,
    pub bits_per_pixel: u32,
    pub grayscale: u32,
    pub red: FbBitfield,
    pub green: FbBitfield,
    pub blue: FbBitfield,
    pub transp: FbBitfield,
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
pub struct FbFixScreeninfo {
    pub id: [c_char; 16],
    pub smem_start: usize, // unsigned long on aarch64 = u64
    pub smem_len: u32,
    pub type_: u32,
    pub type_aux: u32,
    pub visual: u32,
    pub xpanstep: u16,
    pub ypanstep: u16,
    pub ywrapstep: u16,
    pub line_length: u32,
    pub mmio_start: usize,
    pub mmio_len: u32,
    pub accel: u32,
    pub reserved: [u16; 3],
}

/// Try to claim `/dev/fb0` as a scan-out surface. Works on every Linux
/// machine that has a usable `/dev/fb0` device node (legacy framebuffer
/// interface). Returns a Display whose `pixels()` slice IS the visible
/// scan-out — writes appear on the next vsync without any extra ioctl.
pub fn open_fb0() -> Result<Display, String> {
    let fd = sys::open_rw("/dev/fb0").map_err(|e| format!("open /dev/fb0: errno={e}"))?;
    let mut var: FbVarScreeninfo = unsafe { core::mem::zeroed() };
    if let Err(e) = sys::ioctl_struct(fd, FBIOGET_VSCREENINFO, &mut var) {
        sys::close_fd(fd);
        return Err(format!("FBIOGET_VSCREENINFO: errno={e}"));
    }
    let mut fix: FbFixScreeninfo = unsafe { core::mem::zeroed() };
    if let Err(e) = sys::ioctl_struct(fd, FBIOGET_FSCREENINFO, &mut fix) {
        sys::close_fd(fd);
        return Err(format!("FBIOGET_FSCREENINFO: errno={e}"));
    }
    if var.bits_per_pixel != 32 || var.xres == 0 || var.yres == 0 {
        sys::close_fd(fd);
        return Err(format!(
            "/dev/fb0: unsupported geometry ({}x{} @ {}bpp)",
            var.xres, var.yres, var.bits_per_pixel
        ));
    }
    let map_size = fix.smem_len as usize;
    if map_size == 0 {
        sys::close_fd(fd);
        return Err("/dev/fb0: smem_len=0".into());
    }
    let ptr = sys::map_shared(fd, map_size, 0).map_err(|e| {
        sys::close_fd(fd);
        format!("mmap /dev/fb0: errno={e}")
    })?;
    log!(
        "fb0: {}x{} bpp={} stride={} smem={}B",
        var.xres,
        var.yres,
        var.bits_per_pixel,
        fix.line_length,
        map_size
    );
    Ok(Display {
        card_fd: fd,
        crtc_id: 0,
        conn_id: 0,
        fb_id: 0,
        dumb_handle: 0,
        pitch: fix.line_length,
        width: var.xres,
        height: var.yres,
        bpp: 32,
        stride: (fix.line_length / 4) as usize,
        map_ptr: ptr as *mut u32,
        map_size,
        modeset_ok: true, // /dev/fb0 IS the scan-out; the driver owns modeset
        saved_crtc: None,
        via_fb0: true,
    })
}

// ---------- headless fallback ----------
//
// Some environments (containers, CI, this dev sandbox) have no /dev/dri/card*
// or have only disconnected connectors. The frame loop still wants something
// to draw into, so we hand back a software-only "display" whose pixel buffer
// lives in regular heap memory. Any caller that uses `pixels()` gets a
// perfectly usable 32bpp BGRA surface; `present` is a no-op. PRODUCTION.md
// evidence gathering (telemetry, screen.png) works exactly the same in this
// mode — only the actual scanout is missing.

pub struct Headless {
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub buf: Vec<u32>,
}

impl Headless {
    pub fn new(w: u32, h: u32) -> Self {
        let pitch = w * 4;
        let mut buf = vec![0u32; (w as usize) * (h as usize)];
        Self {
            width: w,
            height: h,
            pitch,
            buf,
        }
    }
    pub fn pixels(&mut self) -> &mut [u32] {
        &mut self.buf
    }
    pub fn present(&self) { /* nothing — no scanout */
    }
}

#[allow(unused_macros)]
macro_rules! log {
    ($($arg:tt)*) => ({
        // single sink — the runtime logs to stderr; main.rs swaps in a
        // file-backed logger after we know the state directory.
        eprintln!($($arg)*);
    })
}
pub(crate) use log;
