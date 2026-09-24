//! inkflow — a poetic-phrase-on-a-beat ambient renderer.
//!
//! Zero-dep Rust, designed to run either on a Linux DRM dumb buffer or as a
//! headless harness that writes PNGs for verification.

pub mod color;
pub mod glyph;
#[allow(non_snake_case)]
pub mod glyph_table;
pub mod phrase;
pub mod png;
pub mod rhythm;
pub mod scene;
pub mod surface;
