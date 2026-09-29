use inkflow::glyph;

#[test]
fn dump_composited() {
    let (w, h) = (200u32, 200u32);
    let mut fb = vec![0u32; (w * h) as usize];
    let gi = glyph::index_for('山' as u32);
    let g = &glyph::HERO_TABLE[gi as usize];
    println!("山 by={} h={} adv={}", g.bearing_y, g.h, g.advance);
    // baseline 150px => fy = 150*256, scale 256 (native)
    glyph::draw_glyph(
        &mut fb,
        w as usize,
        h as usize,
        gi,
        0xFFFFFF,
        0xFFFFFF,
        20 * 256,
        150 * 256,
        256,
        1.0,
    );
    for y in 0..h as usize {
        let mut row = String::new();
        for x in 0..w as usize {
            row.push(if fb[y * w as usize + x] != 0 {
                '#'
            } else {
                '.'
            });
        }
        if row.contains('#') {
            println!("{y:3}|{row}|");
        }
    }
}
