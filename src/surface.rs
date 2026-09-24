// inkflow · surface.rs
//
// A render target the frame loop can write into, regardless of whether
// the system has a real DRM/KMS dumb buffer, a /dev/fb0 legacy surface,
// or no graphics stack at all (container, headless test rig). The
// `Surface` enum presents a uniform shape — w(), h(), pitch_px(),
// pixels_mut(), present() — over:
//
//   - `Real(Display)` — full-featured DRM dumb-buffer scanout, drops to
//     `/dev/fb0` if nvidia-drm refuses CREATE_DUMB, drops to Headless
//     if neither is reachable.
//   - `Headless(Headless)` — a heap-allocated 32bpp BGRA buffer, used
//     for telemetry, screenshot evidence, and offline proof that the
//     rendering pipeline works.
//
// The frame loop never branches on which kind of surface is active.
// The only difference is `present()` (no-op for Headless, real scan-out
// for Real), which the loop calls once per frame anyway.
//
// `Display` is reachable directly (from `crate::drm`) for callers that
// need to inspect the saved CRTC, the file descriptor, or restore the
// original mode — the surface enum hides these from the hot path.

use crate::drm::{Display, Headless};

/// A 32bpp BGRA scan-out (or a stand-in that pretends to be one).
///
/// Read-only accessors (`w`, `h`, `pitch_px`) take `&self` because the
/// frame loop never needs to mutate the geometry after open. `pixels_mut`
/// takes `&mut self` because the whole point is to write into the
/// buffer.
pub enum Surface {
    /// Real DRM/KMS dumb-buffer or /dev/fb0 legacy framebuffer.
    Real(Display),
    /// Heap-allocated stand-in for environments without a graphics stack.
    Headless(Headless),
}

impl Surface {
    /// Try the available display backends in order: DRM/KMS → /dev/fb0
    /// → in-memory Headless. The fall-through order is deliberate: if
    /// the nvidia-drm driver is in the way, /dev/fb0 still works on
    /// most Jetson installs and gives us a real scan-out; we only
    /// land in Headless when there is literally no surface to draw on.
    pub fn open() -> Self {
        match crate::drm::open_first() {
            Ok(d) => {
                crate::drm::log!("display: real DRM/KMS dumb-buffer");
                Surface::Real(d)
            }
            Err(e) => {
                crate::drm::log!("display: DRM open_first failed ({e}); trying /dev/fb0");
                match crate::drm::open_fb0() {
                    Ok(d) => {
                        crate::drm::log!(
                            "display: /dev/fb0 legacy framebuffer ({}x{})",
                            d.width,
                            d.height
                        );
                        Surface::Real(d)
                    }
                    Err(e2) => {
                        crate::drm::log!("display: /dev/fb0 unavailable ({e2}); headless fallback");
                        Surface::Headless(Headless::new(1280, 800))
                    }
                }
            }
        }
    }

    #[inline]
    pub fn w(&self) -> u32 {
        match self {
            Surface::Real(d) => d.width,
            Surface::Headless(h) => h.width,
        }
    }
    #[inline]
    pub fn h(&self) -> u32 {
        match self {
            Surface::Real(d) => d.height,
            Surface::Headless(h) => h.height,
        }
    }
    /// Pitch in **pixels** (u32s, not bytes). The drawing primitives in
    /// `crate::font` take a pitch in pixels because every pixel is one
    /// `u32` in our buffer format.
    #[inline]
    pub fn pitch_px(&self) -> usize {
        match self {
            Surface::Real(d) => (d.pitch / 4) as usize,
            Surface::Headless(h) => h.width as usize,
        }
    }
    #[inline]
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        match self {
            Surface::Real(d) => d.pixels(),
            Surface::Headless(h) => h.pixels(),
        }
    }
    #[inline]
    pub fn present(&self) {
        match self {
            Surface::Real(d) => d.present(),
            Surface::Headless(_) => {}
        }
    }

    /// Cheap O(1) byte-budget snapshot of the framebuffer for the
    /// 60-second `state/screen.ppm` writer. The frame loop calls this
    /// on a worker thread so the render thread never blocks on disk.
    pub fn snapshot_owned(&mut self) -> Vec<u32> {
        self.pixels_mut().to_vec()
    }
}
