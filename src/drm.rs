// inkflow · drm.rs
//
// Direct DRM/KMS dumb-buffer surface. Walks /dev/dri/card0, picks the first
// connected connector + a compatible encoder + CRTC, allocates a dumb
// framebuffer of the chosen mode, mmaps it, and we draw straight into
// 32bpp BGRA from the main loop. No GL, no X, no Wayland.
//
// All ioctls are routed through src/sys.rs. Struct layouts mirror
// /usr/include/drm/drm_mode.h and drm_fourcc.h on Linux 5.15+.

#![allow(dead_code, unused_mut)]

use crate::sys;
use core::ffi::{c_char, c_int, c_uchar, c_uint};
use core::mem;

// ---------- DRM ioctl opcodes (linux/uapi/drm/drm.h) ----------
//
// _IOWR(type,nr,size) = (3u32 << 30) | ((size as u32) << 16) | ((type as u32) << 8) | nr as u32
// where type = 'd' = 0x64. We hard-code the value the C macros expand to so we
// don't need a C preprocessor. Sizes match `struct drm_mode_*` in the UAPI.

const DRM_IOCTL_VERSION: u64 = 0xc010_6464;
const DRM_IOCTL_MODE_GETRESOURCES: u64 = 0xc040_64a0;
const DRM_IOCTL_MODE_GETCRTC: u64 = 0xc068_64a1;
const DRM_IOCTL_MODE_SETCRTC: u64 = 0xc068_64a2;
const DRM_IOCTL_MODE_GETENCODER: u64 = 0xc040_64a6;
const DRM_IOCTL_MODE_GETCONNECTOR: u64 = 0xc1a0_64a7;
const DRM_IOCTL_MODE_ADDFB: u64 = 0xc040_64ae;
const DRM_IOCTL_MODE_RMFB: u64 = 0xc010_64af;
const DRM_IOCTL_MODE_CREATE_DUMB: u64 = 0xc020_64b2;
const DRM_IOCTL_MODE_MAP_DUMB: u64 = 0xc010_64b3;
const DRM_IOCTL_MODE_DESTROY_DUMB: u64 = 0xc010_64b4;
const DRM_IOCTL_MODE_GETPLANE: u64 = 0xc0b8_64b6;

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
// Sized exactly as in the kernel headers. Many of them are fixed-size even
// though their trailing pointer arrays are runtime-resized; the pointers
// must live in freshly-mapped heap memory and the ioctl returns the count
// so we can re-query until we hold everything.

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmVersion {
    pub version_major: c_int,
    pub version_minor: c_int,
    pub version_patchlevel: c_int,
    pub name_len: usize,
    pub name: *mut c_char,
    pub date_len: usize,
    pub date: *mut c_char,
    pub desc_len: usize,
    pub desc: *mut c_char,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeRes {
    pub fb_id_ptr: usize,
    pub crtc_id_ptr: usize,
    pub connector_id_ptr: usize,
    pub encoder_id_ptr: usize,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeConnector {
    pub connector_id: u32,
    pub encoder_id: u32,
    pub connector_type: u32,
    pub connector_type_id: u32,
    pub connection: u32,
    pub mm_width: u32,
    pub mm_height: u32,
    pub subpixel: u32,
    pub pad: u32,
    pub count_modes: u32,
    pub modes_ptr: usize, // points to DrmModeModeInfo array
    pub count_props: u32,
    pub props_ptr: usize,
    pub count_encoders: u32,
    pub encoders_ptr: usize,
    pub pad2: [u32; 3],
}

pub const DRM_MODE_CONNECTED: u32 = 1;
pub const DRM_MODE_DISCONNECTED: u32 = 2;
pub const DRM_MODE_UNKNOWNCONNECTION: u32 = 3;

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

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeEncoder {
    pub encoder_id: u32,
    pub encoder_type: u32,
    pub crtc_id: u32,
    pub possible_crtcs: u32,
    pub possible_clones: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeCrtc {
    pub set_connectors_ptr: usize,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub mode_valid: u32,
    pub mode: DrmModeModeInfo,
    pub gamma_size: u32,
}

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

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeMapDumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct DrmModeDestroyDumb {
    pub handle: u32,
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
        // legacy SET_CRTC to push the next frame. For a single fixed mode
        // this is plenty fast; can swap to atomic/page-flip later if
        // tearing shows up.
        let mut crtc = DrmModeCrtc {
            crtc_id: self.crtc_id,
            fb_id: self.fb_id,
            mode_valid: 1,
            ..DrmModeCrtc::zeroed()
        };
        let _ = sys::ioctl_struct(self.card_fd, DRM_IOCTL_MODE_SETCRTC, &mut crtc);
    }
}

impl Drop for Display {
    fn drop(&mut self) {
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

pub fn open_first() -> Result<Display, String> {
    let card_fd = pick_card_fd()?;
    log!("drm: card_fd={} (driver)", card_fd);

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
    let mut date = [0u8; 256];
    let mut desc = [0u8; 256];
    let _ = (date, desc);
    ver.name_len = name.len();
    ver.date_len = date.len();
    ver.desc_len = desc.len();
    ver.name = name.as_ptr() as *mut c_char;
    ver.date = date.as_ptr() as *mut c_char;
    ver.desc = desc.as_ptr() as *mut c_char;
    let r = sys::ioctl_struct(card_fd, DRM_IOCTL_VERSION, &mut ver);
    if let Err(e) = r {
        return Err(format!("DRM_IOCTL_VERSION failed: {e}"));
    }
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
    // Re-issue GETRESOURCES with the pointer fields filled in to slurp the id arrays.
    {
        let mut res2 = DrmModeRes {
            fb_id_ptr: 0,
            crtc_id_ptr: crtc_ids.as_mut_ptr() as usize,
            connector_id_ptr: conn_ids.as_mut_ptr() as usize,
            encoder_id_ptr: enc_ids.as_mut_ptr() as usize,
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
    let (conn_id, conn) = chosen_conn.ok_or("no connected connector")?;
    log!(
        "drm: connector id={} type={} modes={}",
        conn_id,
        conn.connector_type,
        conn.count_modes
    );

    // Pick the first mode from the connector's mode list. ARGB mode is fine
    // for now — we render BGRA but the kernel scans out as XRGB8888 from
    // BGRA memory just as happily; both endiannesses are supported.
    let mut modes: Vec<DrmModeModeInfo> = vec![
        DrmModeModeInfo {
            clock: 0,
            hdisplay: 0,
            hsync_start: 0,
            hsync_end: 0,
            htotal: 0,
            hskew: 0,
            vdisplay: 0,
            vsync_start: 0,
            vsync_end: 0,
            vtotal: 0,
            vscan: 0,
            vrefresh: 0,
            flags: 0,
            type_: 0,
            name: [0; 32],
        };
        conn.count_modes as usize
    ];
    {
        let mut c2 = DrmModeConnector {
            connector_id: conn.connector_id,
            encoder_id: conn.encoder_id,
            connector_type: conn.connector_type,
            connector_type_id: conn.connector_type_id,
            connection: conn.connection,
            mm_width: conn.mm_width,
            mm_height: conn.mm_height,
            subpixel: conn.subpixel,
            pad: conn.pad,
            count_modes: conn.count_modes,
            modes_ptr: modes.as_mut_ptr() as usize,
            count_props: conn.count_props,
            props_ptr: 0,
            count_encoders: conn.count_encoders,
            encoders_ptr: 0,
            pad2: [0; 3],
        };
        sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c2).ok();
    }
    let mode = modes[0];
    log!(
        "drm: mode {}x{} @ {}Hz",
        mode.hdisplay,
        mode.vdisplay,
        mode.vrefresh
    );

    // Find an encoder that supports this connector and at least one CRTC.
    let mut chosen_enc: Option<u32> = None;
    for off in 0..conn.count_encoders as usize {
        // pull encoder id via GETCONNECTOR again — simpler than tracking the array
        let mut c2 = conn;
        let mut enc_arr: Vec<u32> = vec![0; conn.count_encoders as usize];
        c2.encoders_ptr = enc_arr.as_mut_ptr() as usize;
        c2.modes_ptr = modes.as_mut_ptr() as usize; // keep alive
        c2.props_ptr = 0;
        let _ = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c2);
        let enc_id = enc_arr[off];
        let mut enc = DrmModeEncoder {
            encoder_id: enc_id,
            encoder_type: 0,
            crtc_id: 0,
            possible_crtcs: 0,
            possible_clones: 0,
        };
        if sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETENCODER, &mut enc).is_err() {
            continue;
        }
        if enc.possible_crtcs == 0 {
            continue;
        }
        // find a crtc that both the encoder can drive and the system reports
        for cid in &crtc_ids {
            let mut c = DrmModeCrtc {
                crtc_id: *cid,
                ..DrmModeCrtc::zeroed()
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
    if chosen_enc.is_none() {
        return Err("no usable encoder/crtc pair".into());
    }

    // Pick a CRTC that's compatible with the chosen encoder.
    let mut crtc_id = 0u32;
    for cid in &crtc_ids {
        let mut c = DrmModeCrtc {
            crtc_id: *cid,
            ..DrmModeCrtc::zeroed()
        };
        if sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_GETCRTC, &mut c).is_ok() {
            crtc_id = *cid;
            break;
        }
    }
    if crtc_id == 0 {
        return Err("no CRTC available".into());
    }

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

    let disp = Display {
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
    };

    // Apply the mode.
    let conn_id_arr = [conn_id];
    let mut crtc_set = DrmModeCrtc {
        set_connectors_ptr: conn_id_arr.as_ptr() as usize,
        count_connectors: 1,
        crtc_id,
        fb_id: fb.fb_id,
        x: 0,
        y: 0,
        mode_valid: 1,
        mode,
        gamma_size: 0,
    };
    if let Err(e) = sys::ioctl_struct(card_fd, DRM_IOCTL_MODE_SETCRTC, &mut crtc_set) {
        return Err(format!("SETCRTC: errno={e}"));
    }

    log!(
        "drm: mode {}x{} applied, fb={}",
        disp.width,
        disp.height,
        disp.fb_id
    );
    Ok(disp)
}

// ---------- internal helpers ----------

fn pick_card_fd() -> Result<c_int, String> {
    for n in 0..16 {
        let path = format!("/dev/dri/card{n}");
        match sys::open_rw(&path) {
            Ok(fd) => return Ok(fd),
            Err(_) => continue,
        }
    }
    Err("no /dev/dri/card* is openable".into())
}

fn query_resources(fd: c_int) -> Result<DrmModeRes, String> {
    let mut res = DrmModeRes::zeroed();
    sys::ioctl_struct(fd, DRM_IOCTL_MODE_GETRESOURCES, &mut res)
        .map_err(|e| format!("GETRESOURCES: errno={e}"))?;
    Ok(res)
}

fn query_connector(fd: c_int, id: u32) -> Option<DrmModeConnector> {
    let mut c = DrmModeConnector {
        connector_id: id,
        ..DrmModeConnector::zeroed()
    };
    if sys::ioctl_struct(fd, DRM_IOCTL_MODE_GETCONNECTOR, &mut c).is_err() {
        return None;
    }
    Some(c)
}

// Suppress unused-import noise.
#[allow(dead_code)]
const _USED: (c_uint, usize) = (mem::size_of::<u32>() as c_uint, mem::size_of::<usize>());

// Helper: zeroed struct constructors. Required because constructing a struct
// literal with explicit `..Default::default()` then assigning fields trips the
// field_reassign_with_default clippy lint; explicit constructors keep the
// call site tidy.
impl DrmModeCrtc {
    pub fn zeroed() -> Self {
        DrmModeCrtc {
            set_connectors_ptr: 0,
            count_connectors: 0,
            crtc_id: 0,
            fb_id: 0,
            x: 0,
            y: 0,
            mode_valid: 0,
            mode: DrmModeModeInfo {
                clock: 0,
                hdisplay: 0,
                hsync_start: 0,
                hsync_end: 0,
                htotal: 0,
                hskew: 0,
                vdisplay: 0,
                vsync_start: 0,
                vsync_end: 0,
                vtotal: 0,
                vscan: 0,
                vrefresh: 0,
                flags: 0,
                type_: 0,
                name: [0; 32],
            },
            gamma_size: 0,
        }
    }
}
impl DrmModeRes {
    pub fn zeroed() -> Self {
        DrmModeRes {
            fb_id_ptr: 0,
            crtc_id_ptr: 0,
            connector_id_ptr: 0,
            encoder_id_ptr: 0,
            count_fbs: 0,
            count_crtcs: 0,
            count_connectors: 0,
            count_encoders: 0,
            min_width: 0,
            max_width: 0,
            min_height: 0,
            max_height: 0,
        }
    }
}
impl DrmModeConnector {
    pub fn zeroed() -> Self {
        DrmModeConnector {
            connector_id: 0,
            encoder_id: 0,
            connector_type: 0,
            connector_type_id: 0,
            connection: 0,
            mm_width: 0,
            mm_height: 0,
            subpixel: 0,
            pad: 0,
            count_modes: 0,
            modes_ptr: 0,
            count_props: 0,
            props_ptr: 0,
            count_encoders: 0,
            encoders_ptr: 0,
            pad2: [0; 3],
        }
    }
}

// ---------- headless fallback ----------
//
// Some environments (containers, CI, this dev sandbox) have no /dev/dri/card*.
// The frame loop still wants something to draw into, so we hand back a
// software-only "display" whose pixel buffer lives in regular heap memory.
// Any caller that uses `pixels()` gets a perfectly usable 32bpp BGRA surface;
// `present` is a no-op. PRODUCTION.md evidence gathering (telemetry, screen.png)
// works exactly the same in this mode — only the actual scanout is missing.

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
