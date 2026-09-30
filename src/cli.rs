//! Command-line argument parsing and help text.
//!
//! The renderer has five top-level modes:
//!
//! - `--headless[=N]` (default) renders N frames into `state/rhythm-*.png`.
//! - `--layout-test[=N]` runs the layout suite.
//! - `--compose-test[=N]` runs the compose suite.
//! - `--works-gallery` writes one hero frame per pinned work.
//! - `--drm-test` enters the live frame loop on a real DRM/fb0 surface.

use std::env;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Headless,
    LayoutTest,
    ComposeTest,
    WorksGallery,
    DrmTest,
    Help,
    /// A flag we don't recognise — print help *and* exit non-zero, so a typo
    /// in a systemd unit or a deploy script fails loudly instead of silently
    /// rendering a default headless run.
    BadFlag,
}

pub const DEFAULT_W: u32 = 1280;
pub const DEFAULT_H: u32 = 720;
pub const DEFAULT_FRAMES: usize = 12;

pub struct Args {
    pub mode: Mode,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
    pub out_dir: String,
}

pub fn parse() -> Args {
    let mut mode = Mode::Headless;
    let mut width = DEFAULT_W;
    let mut height = DEFAULT_H;
    let mut frames = DEFAULT_FRAMES;
    let mut out_dir = String::from("state");
    for a in env::args().skip(1) {
        if a == "--drm-test" {
            mode = Mode::DrmTest;
        } else if a == "--layout-test" {
            mode = Mode::LayoutTest;
        } else if a == "--compose-test" {
            mode = Mode::ComposeTest;
        } else if a == "--works-gallery" {
            mode = Mode::WorksGallery;
        } else if a == "--help" || a == "-h" {
            mode = Mode::Help;
        } else if a == "--headless" {
            mode = Mode::Headless;
        } else if let Some(v) = a.strip_prefix("--headless=") {
            mode = Mode::Headless;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--layout-test=") {
            mode = Mode::LayoutTest;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--compose-test=") {
            mode = Mode::ComposeTest;
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--frames=") {
            if let Ok(n) = v.parse::<usize>() {
                frames = n;
            }
        } else if let Some(v) = a.strip_prefix("--width=") {
            if let Ok(n) = v.parse::<u32>() {
                width = n;
            }
        } else if let Some(v) = a.strip_prefix("--height=") {
            if let Ok(n) = v.parse::<u32>() {
                height = n;
            }
        } else if let Some(v) = a.strip_prefix("--out=") {
            out_dir = v.to_string();
        } else if a.starts_with("--") {
            eprintln!("inkflow: unknown flag: {a}");
            mode = Mode::BadFlag;
        }
    }
    Args {
        mode,
        width,
        height,
        frames,
        out_dir,
    }
}

pub fn print_help() {
    println!(
        "inkflow — poetic phrases on a beat (zero-dep Rust, DRM/fb0/headless)

Usage:
  inkflow --headless[=N]    render N frames to state/rhythm-{{N}}.png (default 12)
  inkflow --layout-test[=N] render N frames to state/layout-{{N}}.png — full
                            composition with staggered refreshes (default 24)
  inkflow --compose-test[=N] render N frames to state/compose-{{N}}.png — clean
                            multi-phrase composition (hero + 3 supporting);
                            default 12
  inkflow --works-gallery   render one hero frame per line for every complete
                            work into state/works/{{theme}}-{{line}}.png
  inkflow --drm-test        run the live renderer against the real display stack
  inkflow --width=W --height=H  output resolution (default 1280x720)
  inkflow --out=DIR         output directory for headless frames
  inkflow --help            show this help

The headless harness uses the SAME paint path as the live renderer so the
captured PNGs are faithful previews of what the live device shows.",
    );
}
