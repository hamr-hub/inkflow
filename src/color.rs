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

/// sRGB → linear (cheap gamma 2.2 — good enough for blending).
#[inline]
pub fn srgb_to_lin(v: u8) -> f32 {
    let x = v as f32 * (1.0 / 255.0);
    x * x
}

/// linear → sRGB with gamma 2.2 reverse.
#[inline]
pub fn lin_to_srgb(x: f32) -> u8 {
    let c = x.clamp(0.0, 1.0).sqrt();
    (c * 255.0 + 0.5) as u8
}

/// Additive light blend in linear light, then tonemap back to sRGB.
/// `src` is treated as an emissive light source on top of `dst` (background).
#[inline]
pub fn blend_add_lin(dst: u32, src: u32, src_a: f32) -> u32 {
    let dr = srgb_to_lin(r(dst)) as f32;
    let dg = srgb_to_lin(g(dst)) as f32;
    let db = srgb_to_lin(b(dst)) as f32;
    let sr = srgb_to_lin(r(src)) as f32 * src_a;
    let sg = srgb_to_lin(g(src)) as f32 * src_a;
    let sb = srgb_to_lin(b(src)) as f32 * src_a;
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
    let dr = srgb_to_lin(r(dst)) as f32;
    let dg = srgb_to_lin(g(dst)) as f32;
    let db = srgb_to_lin(b(dst)) as f32;
    let sr = srgb_to_lin(r(src)) as f32;
    let sg = srgb_to_lin(g(src)) as f32;
    let sb = srgb_to_lin(b(src)) as f32;
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
