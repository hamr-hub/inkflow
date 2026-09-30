//! Harness modules — non-main code paths that drive the renderer.
//!
//! `capture` is the headless frame-grabber shared by `--headless`,
//! `--layout-test`, and `--compose-test`. `gallery` writes one hero frame
//! per pinned work for `--works-gallery`. `live` is the real frame loop on
//! `/dev/fb0` or DRM, used by `--drm-test`.

pub mod capture;
pub mod gallery;
pub mod live;
