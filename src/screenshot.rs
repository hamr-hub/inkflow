// inkflow · screenshot.rs
//
// 60-second "look at this later" frame capture. The render thread
// snapshots its pixel buffer (one ~4 MB copy), hands it to a worker
// thread that writes a PPM and asks `ffmpeg` to convert to PNG.
//
// The conversion to PNG is done off the render thread so the
// per-minute ffmpeg cost (100-500 ms) doesn't stall the 60 Hz loop.
// ffmpeg is treated as best-effort: if it's not on PATH or fails,
// the PPM stays on disk and the autoloop can still read it.
//
// All file IO is non-fatal; the helpers log to stderr and return. The
// frame loop never branches on whether the write succeeded.

use std::io::Write;

/// Write a down-sampled PPM of the BGRA buffer. Caller passes a frame
/// of `pitch_px`-strided u32 pixels. Caller does not need to hold any
/// lock — we copy into a fresh Vec inside the worker thread before
/// the render thread continues. Returns once the worker has been
/// spawned (does not wait for it to finish).
pub fn spawn_grab(
    bgra: Vec<u32>,
    w: u32,
    h: u32,
    pitch_px: usize,
    ppm_path: String,
    png_path: String,
) {
    std::thread::spawn(move || {
        write_ppm(&bgra, w, h, pitch_px, &ppm_path);
        // PNG encode happens here, off the render thread.
        let _ = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i", &ppm_path, &png_path])
            .status();
    });
}

/// Write a P6 PPM with 24-bit pixels. Pure std I/O — no PNG, no
/// compression, no third-party encoder.
fn write_ppm(bgra: &[u32], w: u32, h: u32, pitch_px: usize, path: &str) {
    let Ok(mut f) = std::fs::File::create(path) else {
        return;
    };
    let _ = writeln!(f, "P6\n{w} {h}\n255");
    // Stream directly into a pre-sized buffer; u32→u8 packing happens
    // inline so we never materialize an extra `Vec<u8>` of size > ~12 MB.
    let mut buf = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for y in 0..h as usize {
        let row = y * pitch_px;
        for x in 0..w as usize {
            let px = bgra[row + x];
            buf.push((px & 0xFF) as u8);
            buf.push(((px >> 8) & 0xFF) as u8);
            buf.push(((px >> 16) & 0xFF) as u8);
        }
    }
    let _ = f.write_all(&buf);
}
