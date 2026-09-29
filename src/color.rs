//! Restrained gallery palette + sRGB / linear helpers + soft blend ops.
//!
//! All public types are little-endian `u32` packed as `0x00RRGGBB` so a framebuffer
//! of `u32` pixels can be compared and copied without per-byte marshalling.

/// Pack three 8-bit channels into a pixel value.
#[inline]
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline]
pub const fn r(p: u32) -> u8 {
    (p >> 16) as u8
}
#[inline]
pub const fn g(p: u32) -> u8 {
    (p >> 8) as u8
}
#[inline]
pub const fn b(p: u32) -> u8 {
    p as u8
}

/// sRGB → linear. This is a gamma-2.0 approximation, not the sRGB transfer
/// function: cheap, and a matched pair with `lin_to_srgb` below. The two
/// together round-trip every 8-bit value exactly, which is the property the
/// blends rely on.
#[inline]
pub fn srgb_to_lin(v: u8) -> f32 {
    let x = v as f32 * (1.0 / 255.0);
    x * x
}

/// linear → sRGB, the gamma-2.0 reverse of `srgb_to_lin`. Clamped, so a blend
/// that overshoots saturates instead of wrapping around.
#[inline]
pub fn lin_to_srgb(x: f32) -> u8 {
    let c = x.clamp(0.0, 1.0).sqrt();
    (c * 255.0 + 0.5) as u8
}

/// Additive light blend in linear light, then tonemap back to sRGB.
/// `src` is treated as an emissive light source on top of `dst` (background).
#[inline]
pub fn blend_add_lin(dst: u32, src: u32, src_a: f32) -> u32 {
    let dr = srgb_to_lin(r(dst));
    let dg = srgb_to_lin(g(dst));
    let db = srgb_to_lin(b(dst));
    let sr = srgb_to_lin(r(src)) * src_a;
    let sg = srgb_to_lin(g(src)) * src_a;
    let sb = srgb_to_lin(b(src)) * src_a;
    rgb(
        lin_to_srgb(dr + sr),
        lin_to_srgb(dg + sg),
        lin_to_srgb(db + sb),
    )
}

/// "Screen" blend: 1 - (1 - dst) * (1 - src), a soft additive that never blows out.
#[inline]
pub fn blend_screen(dst: u32, src: u32, src_a: f32) -> u32 {
    let dr = srgb_to_lin(r(dst));
    let dg = srgb_to_lin(g(dst));
    let db = srgb_to_lin(b(dst));
    let sr = srgb_to_lin(r(src)) * src_a;
    let sg = srgb_to_lin(g(src)) * src_a;
    let sb = srgb_to_lin(b(src)) * src_a;
    let o = |d: f32, s: f32| 1.0 - (1.0 - d) * (1.0 - s);
    rgb(
        lin_to_srgb(o(dr, sr)),
        lin_to_srgb(o(dg, sg)),
        lin_to_srgb(o(db, sb)),
    )
}

/// Normal alpha-over composite in linear light.
#[inline]
pub fn blend_over_lin(dst: u32, src: u32, src_a: f32) -> u32 {
    let a = src_a.clamp(0.0, 1.0);
    let dr = srgb_to_lin(r(dst));
    let dg = srgb_to_lin(g(dst));
    let db = srgb_to_lin(b(dst));
    let sr = srgb_to_lin(r(src));
    let sg = srgb_to_lin(g(src));
    let sb = srgb_to_lin(b(src));
    let o = |d: f32, s: f32| s * a + d * (1.0 - a);
    rgb(
        lin_to_srgb(o(dr, sr)),
        lin_to_srgb(o(dg, sg)),
        lin_to_srgb(o(db, sb)),
    )
}

/// Multiply blend for ink-on-paper deepening.
#[inline]
pub fn blend_mul(dst: u32, src: u32, src_a: f32) -> u32 {
    let dr = srgb_to_lin(r(dst));
    let dg = srgb_to_lin(g(dst));
    let db = srgb_to_lin(b(dst));
    let sr = srgb_to_lin(r(src));
    let sg = srgb_to_lin(g(src));
    let sb = srgb_to_lin(b(src));
    let a = src_a.clamp(0.0, 1.0);
    let m = |d: f32, s: f32| d * (1.0 - a + a * s);
    rgb(
        lin_to_srgb(m(dr, sr)),
        lin_to_srgb(m(dg, sg)),
        lin_to_srgb(m(db, sb)),
    )
}

/// Hue shift in YIQ-ish space (cheap). `t` in [-1, 1].
pub fn hue_shift(p: u32, t: f32) -> u32 {
    if t.abs() < 1e-5 {
        return p;
    }
    let r = r(p) as f32 / 255.0;
    let g = g(p) as f32 / 255.0;
    let b = b(p) as f32 / 255.0;
    // luminance preserved; rotate (r-g, b-luma) by angle ~ t * 30 deg.
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    let cr = r - luma;
    let cg = g - luma;
    let cb = b - luma;
    let ang = t * 0.5;
    let (cs, sn) = (ang.cos(), ang.sin());
    let nr = (cr * cs - cb * sn) + luma;
    let ng = (cg - 0.5 * (cr * (cs - 1.0) - cb * sn)) + luma;
    let nb = (cb * cs + cr * sn) + luma;
    rgb(
        (nr.clamp(0.0, 1.0) * 255.0) as u8,
        (ng.clamp(0.0, 1.0) * 255.0) as u8,
        (nb.clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// 3-stop sRGB gradient along parameter u in [0,1].
#[inline]
pub fn grad3(c0: u32, c1: u32, c2: u32, u: f32) -> u32 {
    let u = u.clamp(0.0, 1.0);
    if u < 0.5 {
        grad2(c0, c1, u * 2.0)
    } else {
        grad2(c1, c2, (u - 0.5) * 2.0)
    }
}

/// 2-stop sRGB gradient.
#[inline]
pub fn grad2(c0: u32, c1: u32, u: f32) -> u32 {
    let u = u.clamp(0.0, 1.0);
    let r = lerp_u8(r(c0), r(c1), u);
    let g = lerp_u8(g(c0), g(c1), u);
    let b = lerp_u8(b(c0), b(c1), u);
    rgb(r, g, b)
}

#[inline]
pub fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Smoothstep easing (Perlin's classic).
#[inline]
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Quintic smoothstep (Ken Perlin's improved).
#[inline]
pub fn smootherstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

// ----- Palette -----

/// Deep background — top: midnight teal; mid: aubergine; bottom: warm umber horizon.
pub mod bg {
    use super::rgb;
    pub const SKY: u32 = rgb(8, 12, 22); // near-black midnight
    pub const MID: u32 = rgb(28, 22, 44); // aubergine
    pub const HORIZON: u32 = rgb(70, 50, 42); // warm umber glow at the bottom
    pub const DEEP: u32 = rgb(4, 6, 14); // deepest shadow
}

/// Glyph ink — warm cream with cool shadow variant.
pub mod ink {
    use super::rgb;
    pub const CREAM: u32 = rgb(232, 212, 168); // primary glyph color
    pub const WARM: u32 = rgb(248, 232, 184); // highlight
    pub const SHADOW: u32 = rgb(192, 168, 136); // shadow
    pub const GLOW: u32 = rgb(248, 224, 160); // outer glow
                                              // Cinnabar for the work seal — weathered, not lacquer-red, so the eye reads
                                              // it as a stamped ink mark rather than a UI accent.
    pub const SEAL: u32 = rgb(184, 64, 56);
}

/// Particle palette — warm + cool + neutral ink drops.
pub mod drop {
    use super::rgb;
    pub const AMBER: u32 = rgb(232, 168, 120); // warm amber
    pub const AMBER_HI: u32 = rgb(248, 216, 168); // amber highlight
    pub const CYAN: u32 = rgb(90, 200, 216); // cool cyan
    pub const CYAN_HI: u32 = rgb(154, 230, 232); // cyan highlight
    pub const PARCHMENT: u32 = rgb(216, 192, 160); // neutral parchment
    pub const PARCHMENT_HI: u32 = rgb(248, 232, 192); // parchment highlight
}

/// Distant starlight.
pub mod star {
    use super::rgb;
    pub const WARM: u32 = rgb(248, 240, 224);
    pub const COOL: u32 = rgb(200, 224, 248);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blend that contributes nothing must leave the pixel exactly as it
    /// found it. Every layer in the piece is composited through these ops, so
    /// a broken identity here is a whole-frame drift that no amount of looking
    /// at a screenshot would catch.
    #[test]
    fn zero_alpha_blends_are_the_identity() {
        for v in 0..=255u8 {
            let p = rgb(v, v.wrapping_mul(3), v.wrapping_mul(7));
            assert_eq!(blend_add_lin(p, rgb(200, 224, 248), 0.0), p);
            assert_eq!(blend_screen(p, rgb(200, 224, 248), 0.0), p);
            assert_eq!(blend_over_lin(p, rgb(200, 224, 248), 0.0), p);
            assert_eq!(blend_mul(p, rgb(200, 224, 248), 0.0), p);
        }
    }

    /// The sRGB round trip has to be exact for every 8-bit value, or a
    /// no-op round trip through a blend would quietly shift the palette.
    #[test]
    fn srgb_round_trip_is_exact() {
        for v in 0..=255u8 {
            assert_eq!(lin_to_srgb(srgb_to_lin(v)), v, "round trip broke at {v}");
        }
    }

    /// Guards the threshold the moon's glow relies on. `GLOW_MIN` in
    /// background.rs only keeps the disc boundary invisible because an alpha
    /// that small still moves a channel by at most one level; at the old
    /// 0.003 it moved three to five, which is the seam it used to print.
    /// Guards the threshold the moon's glow relies on. `GLOW_MIN` in
    /// background.rs only keeps the disc boundary invisible because an alpha
    /// that small still moves a channel by at most one level, wherever the sky
    /// actually is.
    ///
    /// The floor is level 8, and that is not a fudge: the piece's background
    /// measures around 25-29 where the glow lands, and the next test records
    /// why anything below 8 behaves differently.
    #[test]
    fn a_negligible_add_moves_at_most_one_level() {
        for v in 8..=255u8 {
            let got = blend_add_lin(rgb(v, v, v), star::COOL, 0.0003);
            let worst = [r(got), g(got), b(got)]
                .iter()
                .map(|&c| (c as i32 - v as i32).abs())
                .max()
                .unwrap();
            assert!(
                worst <= 1,
                "alpha 0.0003 moved a channel by {worst} at level {v}"
            );
        }
    }

    /// The pipeline encodes with gamma 2.0, so the darkest levels have a very
    /// high gain: on pure black an alpha of 0.0003 still lands near level 4.
    /// Nothing in the piece composites a glow onto black, but this is why
    /// "negligible alpha" is not a uniform guarantee, and why `GLOW_MIN` is
    /// not simply "as small as possible".
    #[test]
    fn near_black_has_high_gain() {
        let lifted = blend_add_lin(rgb(0, 0, 0), star::COOL, 0.0003);
        assert!(
            r(lifted) >= 3,
            "expected gamma gain near black, got level {}",
            r(lifted)
        );
    }

    /// Light only adds, and it saturates rather than wrapping around.
    #[test]
    fn additive_light_never_darkens_and_never_wraps() {
        let dst = rgb(40, 90, 160);
        for step in 0..=10 {
            let a = step as f32 / 10.0;
            let got = blend_add_lin(dst, rgb(255, 255, 255), a);
            assert!(r(got) >= r(dst) && g(got) >= g(dst) && b(got) >= b(dst));
        }
        let white = blend_add_lin(rgb(250, 250, 250), rgb(255, 255, 255), 1.0);
        assert_eq!((r(white), g(white), b(white)), (255, 255, 255));
    }

    /// Alpha-over endpoints, and that a out-of-range alpha is clamped rather
    /// than allowed to extrapolate past the source colour.
    #[test]
    fn over_lin_endpoints_and_clamping() {
        let (black, white) = (rgb(0, 0, 0), rgb(255, 255, 255));
        assert_eq!(blend_over_lin(white, black, 0.0), white);
        assert_eq!(blend_over_lin(black, white, 1.0), white);
        assert_eq!(blend_over_lin(white, black, 1.0), black);
        assert_eq!(
            blend_over_lin(white, black, 2.0),
            black,
            "alpha > 1 must clamp"
        );
        assert_eq!(
            blend_over_lin(white, black, -1.0),
            white,
            "alpha < 0 must clamp"
        );
    }

    /// Screen is the soft additive the atmosphere leans on: it must reach
    /// white but never carry past it.
    #[test]
    fn screen_soft_adds_to_white() {
        let black = rgb(0, 0, 0);
        assert_eq!(blend_screen(black, white_rgb(), 1.0), white_rgb());
        let mid = blend_screen(black, white_rgb(), 0.5);
        assert!(
            r(mid) > 0 && r(mid) < 255,
            "half-strength screen should be mid-grey"
        );
    }

    fn white_rgb() -> u32 {
        rgb(255, 255, 255)
    }

    #[test]
    fn gradients_land_on_their_stops() {
        let (a, b, c) = (rgb(10, 20, 30), rgb(120, 130, 140), rgb(240, 250, 255));
        assert_eq!(grad2(a, b, 0.0), a);
        assert_eq!(grad2(a, b, 1.0), b);
        assert_eq!(grad2(a, b, -5.0), a, "u below range clamps");
        assert_eq!(grad2(a, b, 5.0), b, "u above range clamps");
        assert_eq!(grad3(a, b, c, 0.0), a);
        assert_eq!(grad3(a, b, c, 1.0), c);
    }

    /// The window the moon's tail uses. Endpoints must be exact or the glow
    /// does not actually reach zero where the code says it does.
    #[test]
    fn smoothstep_endpoints_are_exact_and_clamped() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(2.0), 1.0);
        assert_eq!(smootherstep(-1.0), 0.0);
        assert_eq!(smootherstep(2.0), 1.0);
    }

    #[test]
    fn channel_packing_round_trips() {
        for (rr, gg, bb) in [(0u8, 0u8, 0u8), (255, 255, 255), (1, 2, 3), (200, 224, 248)] {
            let p = rgb(rr, gg, bb);
            assert_eq!((r(p), g(p), b(p)), (rr, gg, bb));
        }
    }
}
