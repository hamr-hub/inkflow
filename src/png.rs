//! Zero-dependency PNG encoder.
//!
//! Writes an 8-bit RGB PNG using stored (uncompressed) DEFLATE blocks inside a
//! minimal zlib stream. Compatible with every PNG spec-conformant decoder.

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

fn crc32(buf: &[u8]) -> u32 {
    let mut c: u32 = 0xffff_ffff;
    for &b in buf {
        c = CRC_TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    c ^ 0xffff_ffff
}

fn adler32(buf: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &v in buf {
        a = (a + v as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn with_capacity(cap: usize) -> Self {
        Self { out: Vec::with_capacity(cap) }
    }
    fn push(&mut self, b: u8) {
        self.out.push(b);
    }
    fn push_all(&mut self, s: &[u8]) {
        self.out.extend_from_slice(s);
    }
    fn push_be32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_be_bytes());
    }
    fn push_be16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_be_bytes());
    }
    fn chunk(&mut self, kind: &[u8; 4], data: &[u8]) {
        self.push_be32(data.len() as u32);
        let mut crc_buf = [0u8; 4];
        crc_buf.copy_from_slice(kind);
        self.push_all(&crc_buf);
        self.push_all(data);
        let mut tail = [0u8; 4 + data.len()];
        tail[..4].copy_from_slice(kind);
        tail[4..].copy_from_slice(data);
        self.push_be32(crc32(&tail));
    }
}

/// Encode `width × height` 8-bit RGB pixels into a PNG byte stream.
/// `pixels` is row-major, 3 bytes per pixel (R,G,B).
pub fn encode_rgb(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let bpp: u32 = 3;
    let stride = width as usize * bpp as usize;
    assert_eq!(pixels.len(), stride * height as usize, "pixel buffer size mismatch");

    let mut w = Writer::with_capacity(8 + (stride + 1) * height as usize / 4);
    // signature
    w.push_all(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    // IHDR
    let mut ihdr = [0u8; 13];
    ihdr[..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = 2; // color type RGB
    ihdr[10] = 0; // compression
    ihdr[11] = 0; // filter
    ihdr[12] = 0; // interlace
    w.chunk(b"IHDR", &ihdr);

    // IDAT — zlib stream of stored DEFLATE blocks, each preceded by filter byte (0)
    // Deflate stored block header is byte-aligned: low nibble = 0x00 means final=0 stored, 0x01 means final=1 stored.
    let max_block: usize = 65_532; // 0xFFFC minus 5 for header
    let raw_line = stride + 1; // filter byte + RGB
    let mut zbuf = Vec::with_capacity(raw_line + 8);
    zbuf.push(0x78); // CMF: deflate, window 32K
    zbuf.push(0x01); // FLG: no dict, level 0
    // For each row: filter byte 0, then RGB; we slice into blocks of <= max_block raw bytes.
    // DEFLATE stored format: 1 byte (BFINAL + BTYPE=00), then LEN, NLEN, then LEN bytes of data.
    let mut row = 0;
    while row < height as usize {
        // We can store up to (max_block - 1) rows worth of raw bytes if line fits.
        // Compute how many full rows we can pack into the next stored block.
        let max_rows = (max_block - 1) / raw_line;
        let rows_this_block = (height as usize - row).min(max_rows.max(1));
        let is_last = row + rows_this_block == height as usize;
        let block_len = rows_this_block * raw_line;
        zbuf.push(if is_last { 0x01 } else { 0x00 }); // BFINAL | BTYPE=00
        zbuf.extend_from_slice(&(block_len as u16).to_le_bytes());
        zbuf.extend_from_slice(&((!(block_len as u16)) as u16).to_le_bytes());
        for r in row..row + rows_this_block {
            zbuf.push(0); // filter type: None
            let off = r * stride;
            zbuf.extend_from_slice(&pixels[off..off + stride]);
        }
        row += rows_this_block;
    }
    // Adler32 checksum
    let a = adler32(&zbuf[2..]);
    zbuf.extend_from_slice(&a.to_be_bytes());

    w.chunk(b"IDAT", &zbuf);
    w.chunk(b"IEND", &[]);

    w.out
}