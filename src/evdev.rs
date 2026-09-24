// inkflow · evdev.rs
//
// Raw evdev touch handling. Opens /dev/input/event* directly via the syscall
// wrappers in src/sys.rs, identifies touch devices via EVIOCGBIT, and reads
// input_event structs (24 bytes on Linux) for MT_SLOT / MT_POSITION_X /
// MT_POSITION_Y. Hot-plug is event-driven through inotify on /dev/input.
//
// One Touch thread per device, plus a single supervisor that opens an
// inotify watch on /dev/input and spawns readers whenever a new event*
// appears that passes the touch predicate.

use crate::sys;
use core::ffi::c_int;
use core::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ---------- UAPI ----------

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct InputEvent {
    pub time: TimeVal,
    pub type_: u16,
    pub code: u16,
    pub value: i32,
}

#[allow(dead_code)]
pub const EV_SYN: u16 = 0x00;
#[allow(dead_code)]
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;
pub const BTN_LEFT: u16 = 0x110;
#[allow(dead_code)]
pub const BTN_RIGHT: u16 = 0x111;
#[allow(dead_code)]
pub const BTN_MIDDLE: u16 = 0x112;
pub const BTN_TOUCH: u16 = 0x14a;

pub const EVIOCGBIT: u64 = 0x8004_5d22; // _IOC(0, 'E', 0x22, 128)
pub const EVIOCGABS: u64 = 0x8014_5d40; // _IOR('E', 0x40 + axis, struct input_absinfo)

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct InputAbsInfo {
    pub value: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
}

extern "C" {
    fn ioctl(fd: c_int, request: u64, arg: usize) -> c_int;
}

// EVIOCGBIT takes a fixed-size 128-byte bitmask for EV_KEY+EV_ABS+...;
// we pass the bitmask address as the third ioctl arg.

fn eviocgbit(fd: c_int, ev: u32) -> Result<[u8; 128], Errno> {
    let mut bits = [0u8; 128];
    let req = EVIOCGBIT | ((128u64) << 16); // length = 128 in upper 14 bits
    let r = unsafe { ioctl(fd, req, bits.as_mut_ptr() as usize) };
    if r < 0 {
        return Err(sys::errno());
    }
    let _ = ev;
    Ok(bits)
}

fn bit_set(bits: &[u8; 128], n: u16) -> bool {
    (bits[(n / 8) as usize] >> (n % 8)) & 1 != 0
}

fn eviocgabs(fd: c_int, code: u16) -> Result<InputAbsInfo, Errno> {
    let mut info = InputAbsInfo::default();
    let req = EVIOCGABS + code as u64;
    let r = unsafe { ioctl(fd, req, &mut info as *mut InputAbsInfo as usize) };
    if r < 0 {
        Err(sys::errno())
    } else {
        Ok(info)
    }
}

type Errno = c_int;

// ---------- public state ----------

#[derive(Clone, Copy, Default)]
pub struct Contact {
    pub x: f32, // normalized 0..1
    pub y: f32,
}

pub struct TouchState {
    pub contacts: Vec<Contact>,
    /// Synthetic contact synthesized from a USB mouse: cursor position
    /// when BTN_LEFT is held. Lives alongside the multi-touch slots so
    /// dev hosts without a touchscreen can still drive the piece. On
    /// release the contact disappears (no ghost trail).
    pub mouse: Option<Contact>,
    pub energy: f32,
    pub warmth: f32,
    pub last: Option<std::time::Instant>,
    pub device: String,
}

impl Default for TouchState {
    fn default() -> Self {
        Self {
            contacts: Vec::with_capacity(8),
            mouse: None,
            energy: 0.0,
            warmth: 0.5,
            last: None,
            device: String::new(),
        }
    }
}

pub fn open_input_dir() -> String {
    std::env::var("INKFLOW_INPUT_DIR").unwrap_or_else(|_| "/dev/input".to_string())
}

// ---------- detection ----------

pub fn is_touch_device(path: &str) -> bool {
    let Ok(fd) = sys::open_ro(path) else {
        return false;
    };
    let r = eviocgbit(fd, 0).ok();
    sys::close_fd(fd);
    let Some(bits) = r else { return false };
    // Heuristic: either ABS_MT_POSITION_X + ABS_MT_SLOT (true multitouch)
    // or ABS_X + BTN_TOUCH (single-touch resistive).
    let mt_x = bit_set(&bits, ABS_MT_POSITION_X);
    let mt_slot = bit_set(&bits, ABS_MT_SLOT);
    let x = bit_set(&bits, ABS_X);
    let btn_touch = bit_set(&bits, BTN_TOUCH);
    (mt_x && mt_slot) || (x && btn_touch)
}

/// A mouse: EV_REL + REL_X/Y + BTN_LEFT. We don't care about wheels
/// or extra buttons — the cursor position + left button is enough to
/// synthesize a single touch contact. Touchscreens and mice never
/// overlap on the same /dev/input/event* node, so this is exclusive.
pub fn is_mouse_device(path: &str) -> bool {
    let Ok(fd) = sys::open_ro(path) else {
        return false;
    };
    let r = eviocgbit(fd, 0).ok();
    sys::close_fd(fd);
    let Some(bits) = r else { return false };
    let rel = bit_set(&bits, REL_X) && bit_set(&bits, REL_Y);
    let btn = bit_set(&bits, BTN_LEFT);
    rel && btn
}

fn axis_window(fd: c_int) -> (i32, i32, i32, i32) {
    let mut win = (0i32, 4096i32, 0i32, 4096i32);
    let bits = eviocgbit(fd, 0).unwrap_or([0u8; 128]);
    let x_code = if bit_set(&bits, ABS_MT_POSITION_X) {
        ABS_MT_POSITION_X
    } else {
        ABS_X
    };
    let y_code = if bit_set(&bits, ABS_MT_POSITION_Y) {
        ABS_MT_POSITION_Y
    } else {
        ABS_Y
    };
    if let Ok(ax) = eviocgabs(fd, x_code) {
        if ax.maximum > ax.minimum {
            win.0 = ax.minimum;
            win.1 = ax.maximum;
        }
    }
    if let Ok(ay) = eviocgabs(fd, y_code) {
        if ay.maximum > ay.minimum {
            win.2 = ay.minimum;
            win.3 = ay.maximum;
        }
    }
    win
}

// ---------- main supervisor ----------

pub fn start_supervisor(st: Arc<Mutex<TouchState>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let inot = sys::inotify_new().unwrap_or(-1);
        let dir = open_input_dir();
        let mask = sys::IN_CREATE | sys::IN_DELETE | sys::IN_MOVED_FROM | sys::IN_MOVED_TO;
        if inot >= 0 {
            let _ = sys::inotify_watch(inot, &dir, mask);
        }

        // initial pass
        scan_and_spawn(&dir, &st);

        let mut buf = [0u8; sys::INOTIFY_BUF];
        loop {
            // non-blocking inotify read; if no fd we just poll periodically
            if inot >= 0 {
                let r = sys::read_some(inot, &mut buf);
                if let Ok(_n) = r {
                    scan_and_spawn(&dir, &st);
                }
            }
            sys::sleep_ms(2000);
            scan_and_spawn(&dir, &st);
        }
    })
}

fn scan_and_spawn(dir: &str, st: &Arc<Mutex<TouchState>>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("event") {
            continue;
        }
        let path_str = p.to_string_lossy().to_string();
        // Mouse takes priority over touch detection (a USB mouse that
        // happens to also expose ABS bits — rare — would still be
        // routed here first). Touch / mouse readers are exclusive on
        // a single device node.
        if is_mouse_device(&path_str) {
            spawn_mouse_reader(path_str, st.clone());
        } else if is_touch_device(&path_str) {
            spawn_reader(path_str, st.clone());
        }
    }
}

// Each spawned reader registers its path in a process-wide "claimed" set
// so multiple supervisor passes don't pile up duplicates. The set lives
// in a OnceCell-ish Mutex<Vec<String>>; we just push if absent and rely
// on the OS to give us independent reads (each reader is its own thread).

static CLAIMED: std::sync::OnceLock<Mutex<Vec<String>>> = std::sync::OnceLock::new();
static CLAIMED_FLAG: AtomicBool = AtomicBool::new(false);

fn already_running(path: &str) -> bool {
    let set = CLAIMED.get_or_init(|| Mutex::new(Vec::new()));
    let mut g = set.lock().unwrap();
    if g.iter().any(|s| s == path) {
        true
    } else {
        g.push(path.to_string());
        false
    }
}

fn spawn_reader(path: String, st: Arc<Mutex<TouchState>>) {
    if already_running(&path) {
        return;
    }
    std::thread::spawn(move || reader_loop(path, st));
}

fn reader_loop(path: String, st: Arc<Mutex<TouchState>>) {
    loop {
        match run_once(&path, &st) {
            Ok(()) => { /* device closed cleanly */ }
            Err(_) => {
                sys::sleep_ms(1500);
            }
        }
    }
}

fn run_once(path: &str, st: &Arc<Mutex<TouchState>>) -> Result<(), Errno> {
    let fd = sys::open_ro(path)?;
    let name = read_device_name(fd).unwrap_or_else(|| path.to_string());
    let win = axis_window(fd);
    {
        let mut s = st.lock().unwrap();
        s.device = name.clone();
    }
    CLAIMED_FLAG.store(true, Ordering::Relaxed);

    let mut slot = 0i32;
    let mut slots: std::collections::BTreeMap<i32, (i32, i32)> = Default::default();
    let mut prev_pts: Vec<(f32, f32)> = vec![];
    let mut legacy: Option<(i32, i32)> = None;
    let mut buf = [0u8; 256];
    let event_size = mem::size_of::<InputEvent>();

    loop {
        let n = sys::read_some(fd, &mut buf)?;
        if n == 0 {
            sys::sleep_ms(2);
            continue;
        }
        let mut moved = false;
        let mut i = 0usize;
        while i + event_size <= n {
            let ev: InputEvent =
                unsafe { core::ptr::read_unaligned(buf.as_ptr().add(i) as *const InputEvent) };
            i += event_size;
            if ev.type_ == EV_ABS {
                match ev.code {
                    ABS_MT_SLOT => slot = ev.value,
                    ABS_MT_POSITION_X => {
                        slots.entry(slot).or_insert((0, 0)).0 = ev.value;
                        moved = true;
                    }
                    ABS_MT_POSITION_Y => {
                        slots.entry(slot).or_insert((0, 0)).1 = ev.value;
                        moved = true;
                    }
                    ABS_MT_TRACKING_ID => {
                        if ev.value < 0 {
                            slots.remove(&slot);
                        }
                    }
                    ABS_X => {
                        legacy = Some((ev.value, legacy.map(|p| p.1).unwrap_or(0)));
                        moved = true;
                    }
                    ABS_Y => {
                        legacy = Some((legacy.map(|p| p.0).unwrap_or(0), ev.value));
                        moved = true;
                    }
                    _ => {}
                }
            }
        }
        if !moved {
            continue;
        }
        let pts: Vec<(f32, f32)> = if !slots.is_empty() {
            slots
                .values()
                .map(|&(x, y)| {
                    (
                        ((x - win.0) as f32 / (win.1 - win.0) as f32).clamp(0., 1.),
                        ((y - win.2) as f32 / (win.3 - win.2) as f32).clamp(0., 1.),
                    )
                })
                .collect()
        } else if let Some((x, y)) = legacy {
            vec![(
                ((x - win.0) as f32 / (win.1 - win.0) as f32).clamp(0., 1.),
                ((y - win.2) as f32 / (win.3 - win.2) as f32).clamp(0., 1.),
            )]
        } else {
            vec![]
        };
        if pts.is_empty() {
            continue;
        }
        let mut speed = 0f32;
        for (i, (x, y)) in pts.iter().enumerate() {
            if let Some((px, py)) = prev_pts.get(i) {
                speed += ((x - px).powi(2) + (y - py).powi(2)).sqrt();
            }
        }
        prev_pts = pts.clone();
        let mut s = st.lock().unwrap();
        s.contacts.clear();
        for (x, y) in &pts {
            s.contacts.push(Contact { x: *x, y: *y });
        }
        s.warmth = s.warmth * 0.9 + (pts.iter().map(|p| p.0).sum::<f32>() / pts.len() as f32) * 0.1;
        let n = pts.len() as f32;
        s.energy = (s.energy + (speed * 8.0 + 0.15) * (0.5 + n * 0.3)).min(1.0);
        s.last = Some(std::time::Instant::now());
    }
}

fn read_device_name(fd: c_int) -> Option<String> {
    // EVIOCGNAME(256) — _IOR('E', 0x06, 256)
    let mut buf = [0u8; 256];
    const EVIOCGNAME: u64 = 0x8100_5d06; // _IOC(2, 'E', 0x06, 256) — read direction
    let r = unsafe { ioctl(fd, EVIOCGNAME, buf.as_mut_ptr() as usize) };
    if r < 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..len].to_vec()).ok()
}

#[allow(dead_code)]
fn _align_check() {
    let _ = mem::size_of::<InputEvent>() == 24;
}

// ---------- mouse reader ----------
//
// On a dev box without a touchscreen, a USB mouse is the only way to
// drive the piece. We synthesize a single virtual touch contact from
// the cursor position while BTN_LEFT is held; release clears it. The
// cursor starts at the center of the screen and accumulates REL_X /
// REL_Y counts into a normalized [0,1] position.

fn spawn_mouse_reader(path: String, st: Arc<Mutex<TouchState>>) {
    if already_running(&path) {
        return;
    }
    std::thread::spawn(move || mouse_reader_loop(path, st));
}

fn mouse_reader_loop(path: String, st: Arc<Mutex<TouchState>>) {
    loop {
        match run_mouse_once(&path, &st) {
            Ok(()) => { /* device closed cleanly */ }
            Err(_) => {
                sys::sleep_ms(1500);
            }
        }
    }
}

fn run_mouse_once(path: &str, st: &Arc<Mutex<TouchState>>) -> Result<(), Errno> {
    let fd = sys::open_ro(path)?;
    {
        let mut s = st.lock().unwrap();
        if s.device.is_empty() {
            s.device = format!(
                "mouse:{}",
                read_device_name(fd).unwrap_or_else(|| path.into())
            );
        }
    }

    // Per-event cursor position (normalized 0..1). Start at the
    // geometric center so the first click lands mid-screen rather
    // than in a corner.
    let mut cx: f32 = 0.5;
    let mut cy: f32 = 0.5;
    let mut pressed = false;

    // REL counts per event are typically in [-50, 50] for a high-DPI
    // mouse, larger for a slow movement. We need each event to feel
    // like a small but visible motion: 0.0030 / count ≈ a full
    // screen sweep after a couple of inches of mouse travel.
    let motion_scale = 0.0030f32;

    let mut buf = [0u8; 256];
    let event_size = mem::size_of::<InputEvent>();
    loop {
        let n = sys::read_some(fd, &mut buf)?;
        if n == 0 {
            sys::sleep_ms(2);
            continue;
        }
        let mut changed = false;
        let mut i = 0usize;
        while i + event_size <= n {
            let ev: InputEvent =
                unsafe { core::ptr::read_unaligned(buf.as_ptr().add(i) as *const InputEvent) };
            i += event_size;
            match ev.type_ {
                EV_REL => match ev.code {
                    REL_X => {
                        cx = (cx + (ev.value as f32) * motion_scale).clamp(0.0, 1.0);
                        changed = true;
                    }
                    REL_Y => {
                        cy = (cy + (ev.value as f32) * motion_scale).clamp(0.0, 1.0);
                        changed = true;
                    }
                    REL_WHEEL => {}
                    _ => {}
                },
                EV_KEY if ev.code == BTN_LEFT => {
                    pressed = ev.value != 0;
                    changed = true;
                }
                EV_KEY => {}
                _ => {}
            }
        }
        if !changed {
            continue;
        }

        let mut s = st.lock().unwrap();
        s.mouse = if pressed {
            Some(Contact { x: cx, y: cy })
        } else {
            None
        };

        // Synthesize a single touch contact in slot 0 so the
        // existing spawn_particles path picks up the cursor
        // without any new wiring. We replace slot 0 if it exists
        // (so we don't accidentally displace a real multitouch
        // slot added by a touchscreen), and clear slot 0 on
        // release.
        if pressed {
            let contact = Contact { x: cx, y: cy };
            if let Some(slot) = s.contacts.first_mut() {
                *slot = contact;
            } else {
                s.contacts.push(contact);
            }
            // Mouse-driven energy bump so a held click visibly
            // ignites the scene even with no motion. The decay in
            // mood::tick pulls it back down within ~0.5 s of release.
            s.energy = (s.energy + 0.30).min(1.0);
            s.warmth = (s.warmth * 0.92 + cx * 0.08).clamp(0.2, 0.8);
            s.last = Some(std::time::Instant::now());
        } else {
            // Release: keep `last` fresh so the stale filter in
            // mood::tick doesn't immediately wipe our other
            // contacts, but don't touch s.contacts — the stale
            // filter (or the next press) will reconcile.
            s.last = Some(std::time::Instant::now());
        }
    }
}
