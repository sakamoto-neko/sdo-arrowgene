//! Top-level DDS decoding with BC1's one-bit alpha retained (not converted to RGB).
use anyhow::{Result, bail, ensure};

pub(super) fn decode(raw: &[u8]) -> Result<image::RgbaImage> {
    fn word(raw: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(raw[at..at + 4].try_into().unwrap())
    }
    ensure!(
        raw.len() >= 128 && &raw[..4] == b"DDS " && word(raw, 4) == 124 && word(raw, 76) == 32,
        "Invalid DDS header"
    );
    let (h, w) = (word(raw, 12), word(raw, 16));
    ensure!(
        w > 0 && h > 0 && w <= 8192 && h <= 8192,
        "Unsupported DDS dimensions {w}x{h}"
    );
    let mut out = image::RgbaImage::new(w, h);
    let fourcc = &raw[84..88];
    if word(raw, 80) & 4 != 0 {
        let block = match fourcc {
            b"DXT1" => 8,
            b"DXT3" | b"DXT5" => 16,
            _ => bail!("Unsupported DDS compression {:?}", fourcc),
        };
        let nx = w.div_ceil(4) as usize;
        let ny = h.div_ceil(4) as usize;
        ensure!(
            raw.len() >= 128 + nx * ny * block,
            "Truncated DDS block data"
        );
        for by in 0..ny {
            for bx in 0..nx {
                let b = &raw[128 + (by * nx + bx) * block..][..block];
                let colors = &b[block - 8..];
                let c0 = u16::from_le_bytes(colors[..2].try_into().unwrap());
                let c1 = u16::from_le_bytes(colors[2..4].try_into().unwrap());
                let expand = |v: u16| -> [u8; 4] {
                    let r = (v >> 11) as u8;
                    let g = ((v >> 5) & 63) as u8;
                    let b = (v & 31) as u8;
                    [
                        (r << 3) | (r >> 2),
                        (g << 2) | (g >> 4),
                        (b << 3) | (b >> 2),
                        255,
                    ]
                };
                let mut table = [expand(c0), expand(c1), [0, 0, 0, 255], [0, 0, 0, 255]];
                if c0 > c1 || block == 16 {
                    let (a, b) = (table[0], table[1]);
                    table[2] = std::array::from_fn(|c| {
                        if c == 3 {
                            255
                        } else {
                            ((2 * u16::from(a[c]) + u16::from(b[c])) / 3) as u8
                        }
                    });
                    table[3] = std::array::from_fn(|c| {
                        if c == 3 {
                            255
                        } else {
                            ((u16::from(a[c]) + 2 * u16::from(b[c])) / 3) as u8
                        }
                    });
                } else {
                    let (a, b) = (table[0], table[1]);
                    table[2] = std::array::from_fn(|c| {
                        if c == 3 {
                            255
                        } else {
                            ((u16::from(a[c]) + u16::from(b[c])) / 2) as u8
                        }
                    });
                    table[3] = [0; 4];
                }
                let indices = u32::from_le_bytes(colors[4..8].try_into().unwrap());
                let mut alphas = [b[0], b[1], 0, 0, 0, 0, 0, 255];
                if fourcc == b"DXT5" {
                    if b[0] > b[1] {
                        for i in 1..7 {
                            alphas[i + 1] =
                                (((7 - i) * usize::from(b[0]) + i * usize::from(b[1])) / 7) as u8;
                        }
                    } else {
                        for i in 1..5 {
                            alphas[i + 1] =
                                (((5 - i) * usize::from(b[0]) + i * usize::from(b[1])) / 5) as u8;
                        }
                    }
                }
                let packed = u64::from_le_bytes(b[..8].try_into().unwrap());
                for i in 0..16 {
                    let (x, y) = ((bx * 4 + i % 4) as u32, (by * 4 + i / 4) as u32);
                    if x >= w || y >= h {
                        continue;
                    }
                    let mut pixel = table[((indices >> (i * 2)) & 3) as usize];
                    if fourcc == b"DXT3" {
                        pixel[3] = ((packed >> (i * 4)) & 15) as u8 * 17;
                    }
                    if fourcc == b"DXT5" {
                        pixel[3] = alphas[((packed >> (16 + i * 3)) & 7) as usize];
                    }
                    out.put_pixel(x, y, image::Rgba(pixel));
                }
            }
        }
    } else {
        ensure!(word(raw, 80) & 0x40 != 0, "Unsupported DDS pixel format");
        let bits = word(raw, 88);
        ensure!(
            matches!(bits, 16 | 24 | 32),
            "Unsupported DDS pixel width {bits}"
        );
        let size = (bits / 8) as usize;
        let pitch = if word(raw, 8) & 8 != 0 {
            word(raw, 20) as usize
        } else {
            w as usize * size
        };
        ensure!(
            pitch >= w as usize * size && raw.len() >= 128 + pitch * h as usize,
            "Truncated DDS pixels"
        );
        let masks = [word(raw, 92), word(raw, 96), word(raw, 100), word(raw, 104)];
        for y in 0..h {
            for x in 0..w {
                let bytes = &raw[128 + y as usize * pitch + x as usize * size..][..size];
                let mut v = 0u32;
                for (i, b) in bytes.iter().enumerate() {
                    v |= u32::from(*b) << (i * 8);
                }
                let pixel = std::array::from_fn(|i| {
                    let mask = masks[i];
                    if mask == 0 {
                        if i == 3 { 255 } else { 0 }
                    } else {
                        let n = mask >> mask.trailing_zeros();
                        (((v & mask) >> mask.trailing_zeros()) as u64 * 255 / u64::from(n)) as u8
                    }
                });
                out.put_pixel(x, y, image::Rgba(pixel));
            }
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn header(code: &[u8; 4], len: usize) -> Vec<u8> {
        let mut b = vec![0; 128 + len];
        b[..4].copy_from_slice(b"DDS ");
        for (at, v) in [(4, 124u32), (12, 4), (16, 4), (76, 32), (80, 4)] {
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[84..88].copy_from_slice(code);
        b
    }
    #[test]
    fn bc1_transparency_and_bc2_bc3_alpha_are_preserved() {
        let mut b = header(b"DXT1", 8);
        b[130..132].copy_from_slice(&65535u16.to_le_bytes());
        b[132] = 3;
        let img = decode(&b).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [0; 4]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 0, 0, 255]);
        let mut b = header(b"DXT3", 16);
        b[128] = 0xf0;
        b[136..138].copy_from_slice(&0xf800u16.to_le_bytes());
        let img = decode(&b).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 0]);
        assert_eq!(img.get_pixel(1, 0).0, [255, 0, 0, 255]);
        let mut b = header(b"DXT5", 16);
        b[128] = 200;
        b[129] = 100;
        b[130] = 2;
        assert_eq!(decode(&b).unwrap().get_pixel(0, 0).0[3], 185);
        assert!(decode(&b[..130]).is_err());
    }
}
