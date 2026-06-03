//! NSCodec (MS-RDPNSC) decoder.
//!
//! Used here as a ClearCodec subcodec (subcodecId = 1), which this Windows
//! server uses pervasively for UI (lines, borders, small blocks). Ported from
//! FreeRDP `libfreerdp/codec/nsc.c` (Apache-2.0). Output is RGBA8888.
//!
//! Stream layout (20-byte header + planes):
//!   PlaneByteCount[0..4] : u32 LE  (Y / Co / Cg / A, compressed sizes)
//!   ColorLossLevel       : u8  (1..=7)
//!   ChromaSubsampling    : u8  (0 = none)
//!   Reserved             : u16
//!   <planes, concatenated, each PlaneByteCount[i] bytes>

fn round_up(v: usize, m: usize) -> usize {
    v.div_ceil(m) * m
}

/// NSC per-plane RLE: produce exactly `original` bytes into `out`.
fn rle_decode(mut inp: &[u8], original: usize, out: &mut Vec<u8>) -> Result<(), String> {
    out.clear();
    let mut left = original;
    while left > 4 {
        if inp.is_empty() {
            return Err("nsc rle eof".into());
        }
        let value = inp[0];
        inp = &inp[1..];

        if left == 5 {
            out.push(value);
            left -= 1;
        } else if inp.is_empty() {
            return Err("nsc rle eof2".into());
        } else if value == inp[0] {
            // Run: [value][value]([len:u8<0xFF] | [0xFF][len:u32 LE]); len includes
            // the two signalling bytes.
            inp = &inp[1..];
            if inp.is_empty() {
                return Err("nsc rle eof3".into());
            }
            let len: usize = if inp[0] < 0xFF {
                let l = inp[0] as usize + 2;
                inp = &inp[1..];
                l
            } else {
                if inp.len() < 5 {
                    return Err("nsc rle eof4".into());
                }
                let l = inp[1] as usize
                    | (inp[2] as usize) << 8
                    | (inp[3] as usize) << 16
                    | (inp[4] as usize) << 24;
                inp = &inp[5..];
                l
            };
            if len > left {
                return Err("nsc rle len".into());
            }
            out.resize(out.len() + len, value);
            left -= len;
        } else {
            out.push(value);
            left -= 1;
        }
    }
    // Trailing 4 bytes are stored raw.
    if left < 4 || inp.len() < 4 {
        return Err("nsc rle tail".into());
    }
    out.extend_from_slice(&inp[..4]);
    Ok(())
}

/// Decode an NSCodec bitmap stream into `width*height*4` RGBA bytes.
pub fn decode(data: &[u8], width: u16, height: u16) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    if data.len() < 20 {
        return Err("nsc header short".into());
    }

    let mut pbc = [0usize; 4];
    let mut total = 0usize;
    for (i, slot) in pbc.iter_mut().enumerate() {
        let o = i * 4;
        *slot = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
        total += *slot;
    }
    let cll = data[16];
    if !(1..=7).contains(&cll) {
        return Err(format!("nsc cll {cll}"));
    }
    let css = data[17] != 0;
    let planes = &data[20..];
    if planes.len() < total {
        return Err("nsc planes short".into());
    }

    let temp_w = round_up(w, 8);
    let temp_h = round_up(h, 2);

    // Uncompressed (decoded) size of each plane.
    let org = if css {
        [
            temp_w * h,
            (temp_w >> 1) * (temp_h >> 1),
            (temp_w >> 1) * (temp_h >> 1),
            w * h,
        ]
    } else {
        [w * h, w * h, w * h, w * h]
    };

    let mut plane: [Vec<u8>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut off = 0usize;
    for i in 0..4 {
        let psize = pbc[i];
        let osize = org[i];
        if off + psize > planes.len() {
            return Err("nsc plane overflow".into());
        }
        let seg = &planes[off..off + psize];
        if psize == 0 {
            // Constant plane (0xFF). For A this is opaque; for chroma it's a flat
            // value, matching FreeRDP behaviour.
            plane[i] = vec![0xFFu8; osize];
        } else if psize < osize {
            let mut out = Vec::with_capacity(osize);
            rle_decode(seg, osize, &mut out)?;
            if out.len() != osize {
                return Err(format!("nsc plane{i} {} != {osize}", out.len()));
            }
            plane[i] = out;
        } else {
            if seg.len() < osize {
                return Err("nsc raw plane short".into());
            }
            plane[i] = seg[..osize].to_vec();
        }
        off += psize;
    }

    let shift = cll - 1;
    let rw = temp_w;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (yi, ci) = if css {
                (y * rw + x, (y >> 1) * (rw >> 1) + (x >> 1))
            } else {
                (y * w + x, y * w + x)
            };
            let ai = y * w + x;
            let yv = plane[0][yi] as i16;
            let co = ((((plane[1][ci] as u16) << shift) as u8) as i8) as i16;
            let cg = ((((plane[2][ci] as u16) << shift) as u8) as i8) as i16;
            let r = (yv + co - cg).clamp(0, 255) as u8;
            let g = (yv + cg).clamp(0, 255) as u8;
            let b = (yv - co - cg).clamp(0, 255) as u8;
            let a = plane[3][ai];
            let o = (y * w + x) * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
            out[o + 3] = a;
        }
    }
    Ok(out)
}
