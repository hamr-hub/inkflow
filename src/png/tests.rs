use super::*;

/// An independent CRC-32 (IEEE), so this checks the writer's rather than
/// agreeing with it by construction.
fn crc32_of(buf: &[u8]) -> u32 {
    let mut c: u32 = 0xffff_ffff;
    for &byte in buf {
        c ^= byte as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xedb8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Split a PNG into (type, data) chunks, walking the framing properly.
fn chunks(png: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 8; // signature
    while i + 8 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&png[i + 4..i + 8]);
        let data = png[i + 8..i + 8 + len].to_vec();
        out.push((kind, data));
        i += 12 + len; // len + type + data + crc
    }
    out
}

/// Undo the zlib wrapper and the stored DEFLATE blocks. Every block here is
/// BTYPE=00, so there is nothing to inflate.
fn stored_blocks(zlib: &[u8]) -> Vec<u8> {
    assert_eq!(&zlib[..2], &[0x78, 0x01], "zlib header");
    let mut out = Vec::new();
    let mut i = 2;
    loop {
        let header = zlib[i];
        assert_eq!(header >> 1 & 0b11, 0, "expected a stored block");
        let len = u16::from_le_bytes([zlib[i + 1], zlib[i + 2]]) as usize;
        let nlen = u16::from_le_bytes([zlib[i + 3], zlib[i + 4]]) as usize;
        assert_eq!(len, !nlen & 0xffff, "stored block length complement");
        out.extend_from_slice(&zlib[i + 5..i + 5 + len]);
        i += 5 + len;
        if header & 1 == 1 {
            break; // BFINAL
        }
    }
    out
}

/// Pull the filtered scanlines back out and strip the per-row filter byte.
fn decode_rgb(png: &[u8], width: u32, height: u32) -> Vec<u8> {
    let idat = chunks(png)
        .into_iter()
        .find(|(k, _)| k == b"IDAT")
        .expect("no IDAT")
        .1;
    let raw = stored_blocks(&idat);
    let stride = width as usize * 3;
    let mut out = Vec::with_capacity(stride * height as usize);
    for r in 0..height as usize {
        let line = &raw[r * (stride + 1)..(r + 1) * (stride + 1)];
        assert_eq!(line[0], 0, "filter byte on row {r}");
        out.extend_from_slice(&line[1..]);
    }
    out
}

fn ramp(width: u32, height: u32) -> Vec<u8> {
    let mut px = Vec::with_capacity((width * height * 3) as usize);
    for i in 0..(width * height) {
        px.push((i % 251) as u8);
        px.push((i / 7 % 253) as u8);
        px.push((i / 13 % 241) as u8);
    }
    px
}

/// The whole point of stored blocks: a tall image is split across several
/// of them, and the row-packing arithmetic is the easiest thing here to get
/// subtly wrong. Decoding must give the exact bytes back.
#[test]
fn pixels_survive_the_round_trip_across_block_boundaries() {
    // 64 px wide => 193 raw bytes per row, so 700 rows spans three blocks.
    let (w, h) = (64u32, 700u32);
    let px = ramp(w, h);
    let png = encode_rgb(w, h, &px);
    assert_eq!(decode_rgb(&png, w, h), px);
}

#[test]
fn tiny_and_single_pixel_images_round_trip() {
    for (w, h) in [(1u32, 1u32), (7, 3), (1, 40), (40, 1)] {
        let px = ramp(w, h);
        let png = encode_rgb(w, h, &px);
        assert_eq!(decode_rgb(&png, w, h), px, "failed at {w}x{h}");
    }
}

#[test]
fn header_describes_the_image() {
    let (w, h) = (5u32, 9u32);
    let png = encode_rgb(w, h, &ramp(w, h));
    assert_eq!(
        &png[..8],
        &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        "PNG signature"
    );
    let ch = chunks(&png);
    assert_eq!(ch[0].0, *b"IHDR");
    assert_eq!(ch.last().unwrap().0, *b"IEND");
    let d = &ch[0].1;
    assert_eq!(u32::from_be_bytes(d[0..4].try_into().unwrap()), w);
    assert_eq!(u32::from_be_bytes(d[4..8].try_into().unwrap()), h);
    assert_eq!(d[8], 8, "bit depth");
    assert_eq!(d[9], 2, "colour type: truecolour RGB");
    assert_eq!(
        (d[10], d[11], d[12]),
        (0, 0, 0),
        "compression/filter/interlace"
    );
}

#[test]
fn every_chunk_carries_a_valid_crc() {
    let (w, h) = (33u32, 40u32);
    let png = encode_rgb(w, h, &ramp(w, h));
    let mut i = 8;
    let mut seen = 0;
    while i + 8 <= png.len() {
        let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
        let stored = u32::from_be_bytes(png[i + 8 + len..i + 12 + len].try_into().unwrap());
        let computed = crc32_of(&png[i + 4..i + 8 + len]); // type + data
        assert_eq!(stored, computed, "bad CRC on chunk at {i}");
        i += 12 + len;
        seen += 1;
    }
    assert_eq!(seen, 3, "expected IHDR, IDAT, IEND");
}

/// CI diffs the sample frames byte for byte, so the encoder has to be a
/// pure function of its input.
#[test]
fn encoding_is_deterministic() {
    let (w, h) = (17u32, 23u32);
    let px = ramp(w, h);
    assert_eq!(encode_rgb(w, h, &px), encode_rgb(w, h, &px));
}

/// A short buffer is a caller bug, and it should say so loudly rather than
/// read past the end.
#[test]
#[should_panic(expected = "pixel buffer size mismatch")]
fn a_short_pixel_buffer_is_rejected() {
    encode_rgb(4, 4, &[0u8; 10]);
}
