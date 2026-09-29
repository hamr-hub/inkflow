// inkflow · sys.rs
//
// Zero-dependency Linux syscall surface. Every entry point here is a thin
// `extern "C"` re-declaration so the rest of the program never reaches for
// libc/libc-sys. We deliberately declare only what we actually call.
//
// ABI notes:
//   - aarch64 / x86_64 SysV pass the first six integer args in registers and
//     ignore extra stack varargs. `ioctl(fd, req, arg)` and `open(path, ...)`
//     both shove their third argument into rdx/x2 regardless of whether the
//     C prototype declares `...`. Declaring them with explicit 3-arg / 2-arg
//     signatures therefore lines up with the kernel call ABI on both
//     targets and avoids needing libc.
//
// All functions here are unsafe; callers must uphold the kernel contract.
// We never `panic!` in here — errors surface as Err with errno preserved.

#![allow(dead_code, non_camel_case_types)]

use core::ffi::{c_char, c_int, c_void};
use core::mem::{self, MaybeUninit};

pub type Errno = c_int;

// ---------- file ops ----------

pub const O_RDONLY: c_int = 0;
pub const O_WRONLY: c_int = 1;
pub const O_RDWR: c_int = 2;
pub const O_CLOEXEC: c_int = 0o2_000_000;
pub const O_NONBLOCK: c_int = 0o4_000;

extern "C" {
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, count: usize) -> c_int;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> c_int;
}

pub fn open_ro(path: &str) -> Result<c_int, Errno> {
    let p = std::ffi::CString::new(path).map_err(|_| -1)?;
    let fd = unsafe { open(p.as_ptr(), O_RDONLY | O_CLOEXEC) };
    if fd < 0 {
        Err(errno())
    } else {
        Ok(fd)
    }
}

pub fn open_rw(path: &str) -> Result<c_int, Errno> {
    let p = std::ffi::CString::new(path).map_err(|_| -1)?;
    let fd = unsafe { open(p.as_ptr(), O_RDWR | O_CLOEXEC) };
    if fd < 0 {
        Err(errno())
    } else {
        Ok(fd)
    }
}

pub fn close_fd(fd: c_int) {
    unsafe {
        let _ = close(fd);
    };
}

// ---------- mmap ----------

pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_SHARED: c_int = 1;

extern "C" {
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
}

pub fn map_shared(fd: c_int, len: usize, offset: i64) -> Result<*mut u8, Errno> {
    let p = unsafe {
        mmap(
            core::ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            fd,
            offset,
        )
    };
    if p as isize == -1 {
        Err(errno())
    } else {
        Ok(p as *mut u8)
    }
}

pub fn unmap(p: *mut u8, len: usize) {
    unsafe {
        let _ = munmap(p as *mut c_void, len);
    }
}

// ---------- ioctl ----------
//
// We declare a 3-arg `ioctl` (no varargs) because on every Linux arch we run
// on, the third arg is passed in a register, which is what the kernel wants.
// This sidesteps the C variadic ABI entirely.

extern "C" {
    fn ioctl(fd: c_int, request: u64, arg: usize) -> c_int;
}

pub fn ioctl_int(fd: c_int, request: u64, val: c_int) -> Result<c_int, Errno> {
    let r = unsafe { ioctl(fd, request, val as usize) };
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

pub fn ioctl_ptr<T>(fd: c_int, request: u64, val: *mut T) -> Result<c_int, Errno> {
    let r = unsafe { ioctl(fd, request, val as usize) };
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

/// ioctl whose arg is a u64 value passed by value (e.g. fuse handles).
pub fn ioctl_u64(fd: c_int, request: u64, val: u64) -> Result<c_int, Errno> {
    let r = unsafe { ioctl(fd, request, val as usize) };
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

/// Generic struct-pointer ioctl. The struct address is passed as the third arg.
pub fn ioctl_struct<T>(fd: c_int, request: u64, s: &mut T) -> Result<c_int, Errno> {
    let r = unsafe { ioctl(fd, request, s as *mut T as usize) };
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

// ---------- inotify (touch hotplug) ----------

pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_CLOEXEC: u32 = 0x0008_0000;

extern "C" {
    fn inotify_init1(flags: c_int) -> c_int;
    fn inotify_add_watch(fd: c_int, path: *const c_char, mask: u32) -> c_int;
}

pub fn inotify_new() -> Result<c_int, Errno> {
    let fd = unsafe { inotify_init1(IN_CLOEXEC as c_int) };
    if fd < 0 {
        Err(errno())
    } else {
        Ok(fd)
    }
}

pub fn inotify_watch(fd: c_int, path: &str, mask: u32) -> Result<c_int, Errno> {
    let p = std::ffi::CString::new(path).map_err(|_| -22)?;
    let r = unsafe { inotify_add_watch(fd, p.as_ptr(), mask) };
    if r < 0 {
        Err(errno())
    } else {
        Ok(r)
    }
}

// ---------- errno / time ----------

extern "C" {
    fn __errno_location() -> *mut c_int;
    fn clock_gettime(clk: c_int, ts: *mut Timespec) -> c_int;
    fn nanosleep(req: *const Timespec, rem: *mut Timespec) -> c_int;
}

pub fn errno() -> Errno {
    unsafe { *__errno_location() }
}

#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

pub const CLOCK_MONOTONIC: c_int = 2;
pub const CLOCK_REALTIME: c_int = 0;

pub fn clock_monotonic() -> Timespec {
    let mut ts = MaybeUninit::<Timespec>::uninit();
    unsafe {
        let _ = clock_gettime(CLOCK_MONOTONIC, ts.as_mut_ptr());
        ts.assume_init()
    }
}

pub fn clock_realtime() -> Timespec {
    let mut ts = MaybeUninit::<Timespec>::uninit();
    unsafe {
        let _ = clock_gettime(CLOCK_REALTIME, ts.as_mut_ptr());
        ts.assume_init()
    }
}

pub fn sleep_ms(ms: u64) {
    let req = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    unsafe {
        let mut rem = MaybeUninit::<Timespec>::uninit();
        let _ = nanosleep(&req, rem.as_mut_ptr());
    }
}

// Monotonic seconds as f64.
pub fn now_s() -> f64 {
    let ts = clock_monotonic();
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

// Wall-clock seconds since epoch.
pub fn wall_s() -> u64 {
    let ts = clock_realtime();
    ts.tv_sec as u64
}

// ---------- poll ----------

pub const POLLIN: u16 = 0x0001;

#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: u16,
    pub revents: u16,
}

extern "C" {
    fn poll(fds: *mut PollFd, nfds: u64, timeout: c_int) -> c_int;
}

pub fn poll_one(fd: c_int, events: u16, timeout_ms: c_int) -> Result<bool, Errno> {
    let mut p = PollFd {
        fd,
        events,
        revents: 0,
    };
    let r = unsafe { poll(&mut p, 1, timeout_ms) };
    if r < 0 {
        Err(errno())
    } else {
        Ok(p.revents & POLLIN != 0)
    }
}

// ---------- write helper ----------

pub fn write_all(fd: c_int, bytes: &[u8]) -> Result<usize, Errno> {
    let mut off = 0;
    while off < bytes.len() {
        let n = unsafe {
            write(
                fd,
                bytes.as_ptr().add(off) as *const c_void,
                bytes.len() - off,
            )
        };
        if n <= 0 {
            let e = errno();
            if n < 0 && e == 4
            /* EINTR */
            {
                continue;
            }
            return Err(e);
        }
        off += n as usize;
    }
    Ok(off)
}

// ---------- inotify event parsing ----------

#[repr(C)]
pub struct InotifyEvent {
    pub wd: c_int,
    pub mask: u32,
    pub cookie: u32,
    pub len: u32,
    // followed by `len` bytes of name (not in this struct)
}

pub const INOTIFY_BUF: usize = 4096;
pub fn read_inotify(fd: c_int) -> Result<[u8; INOTIFY_BUF], Errno> {
    let mut buf = [0u8; INOTIFY_BUF];
    let n = unsafe { read(fd, buf.as_mut_ptr() as *mut c_void, INOTIFY_BUF) };
    if n < 0 {
        Err(errno())
    } else {
        Ok(buf)
    }
}

// ---------- readline-ish helper for raw fds ----------

pub fn read_some(fd: c_int, buf: &mut [u8]) -> Result<usize, Errno> {
    let n = unsafe { read(fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
    if n < 0 {
        Err(errno())
    } else {
        Ok(n as usize)
    }
}

// Make sure times of constants are never accidentally shadowed by mem::size_of
// on types we don't actually have here.
const _: () = assert!(mem::size_of::<PollFd>() == 8);
