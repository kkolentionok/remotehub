//! ClearCodec (MS-RDPECLEAR) decoder for the GFX pipeline.
//!
//! Format/algorithm reference: FreeRDP `libfreerdp/codec/clear.c` (Apache-2.0).
//! Reimplemented in Rust; all pixels handled directly as RGBA8888 (no format
//! conversion layer). A single decoder instance is kept per GFX session — the
//! vBar/glyph caches and sequence counter are channel-wide and persist across
//! WireToSurface PDUs (and across ResetGraphics).

use tracing::warn;

const FLAG_GLYPH_INDEX: u8 = 0x01;
const FLAG_GLYPH_HIT: u8 = 0x02;
const FLAG_CACHE_RESET: u8 = 0x04;

const VBAR_SIZE: usize = 32768;
const SHORT_VBAR_SIZE: usize = 16384;
const GLYPH_CACHE: usize = 4000;

#[derive(Clone, Default)]
struct VBar {
    px: Vec<u8>, // RGBA, `count` pixels tall
    count: u32,
}

#[derive(Clone, Default)]
struct Glyph {
    px: Vec<u8>, // RGBA, `count` = w*h pixels
    count: u32,
}

/// Minimal little-endian byte reader.
struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.pos)
    }
    fn u8(&mut self) -> Result<u8, String> {
        let v = *self.d.get(self.pos).ok_or("eof u8")?;
        self.pos += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, String> {
        if self.pos + 2 > self.d.len() {
            return Err("eof u16".into());
        }
        let v = u16::from_le_bytes([self.d[self.pos], self.d[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        if self.pos + 4 > self.d.len() {
            return Err("eof u32".into());
        }
        let v = u32::from_le_bytes([
            self.d[self.pos],
            self.d[self.pos + 1],
            self.d[self.pos + 2],
            self.d[self.pos + 3],
        ]);
        self.pos += 4;
        Ok(v)
    }
    fn skip(&mut self, n: usize) {
        self.pos = (self.pos + n).min(self.d.len());
    }
}

#[inline]
fn put(dst: &mut [u8], dw: u16, dh: u16, x: u32, y: u32, r: u8, g: u8, b: u8) {
    if x < dw as u32 && y < dh as u32 {
        let i = (y as usize * dw as usize + x as usize) * 4;
        if i + 4 <= dst.len() {
            dst[i] = r;
            dst[i + 1] = g;
            dst[i + 2] = b;
            dst[i + 3] = 255;
        }
    }
}

#[inline]
fn mask(n: u32) -> u32 {
    if n >= 32 {
        u32::MAX
    } else {
        (1u32 << n) - 1
    }
}

#[inline]
fn floor_log2(v: u32) -> u32 {
    if v == 0 {
        0
    } else {
        31 - v.leading_zeros()
    }
}

pub struct ClearDecoder {
    seq: u32,
    vbar: Vec<VBar>,
    vbar_cursor: usize,
    short_vbar: Vec<VBar>,
    short_cursor: usize,
    glyphs: Vec<Glyph>,
}

impl ClearDecoder {
    pub fn new() -> Self {
        Self {
            seq: 0,
            vbar: vec![VBar::default(); VBAR_SIZE],
            vbar_cursor: 0,
            short_vbar: vec![VBar::default(); SHORT_VBAR_SIZE],
            short_cursor: 0,
            glyphs: vec![Glyph::default(); GLYPH_CACHE],
        }
    }

    /// Decode one ClearCodec stream into `dst` (RGBA, `dw`x`dh`) at (`x0`,`y0`),
    /// where the codec's own coordinate space is `nw`x`nh`.
    pub fn decode(
        &mut self,
        data: &[u8],
        nw: u16,
        nh: u16,
        dst: &mut [u8],
        dw: u16,
        dh: u16,
        x0: u16,
        y0: u16,
    ) -> Result<(), String> {
        let mut r = Reader::new(data);
        let flags = r.u8()?;
        let seq = r.u8()? as u32;
        self.seq = (seq + 1) % 256;

        if flags & FLAG_CACHE_RESET != 0 {
            self.vbar_cursor = 0;
            self.short_cursor = 0;
        }

        // Glyph header.
        let mut glyph_index: Option<usize> = None;
        if flags & FLAG_GLYPH_INDEX != 0 {
            let gi = r.u16()? as usize;
            if gi >= GLYPH_CACHE {
                return Err(format!("glyphIndex {gi}"));
            }
            if flags & FLAG_GLYPH_HIT != 0 {
                // Blit the cached glyph straight to the destination.
                let g = &self.glyphs[gi];
                let need = nw as u32 * nh as u32;
                if g.count >= need && !g.px.is_empty() {
                    for row in 0..nh as u32 {
                        for col in 0..nw as u32 {
                            let si = ((row * nw as u32 + col) * 4) as usize;
                            if si + 4 <= g.px.len() {
                                put(
                                    dst,
                                    dw,
                                    dh,
                                    x0 as u32 + col,
                                    y0 as u32 + row,
                                    g.px[si],
                                    g.px[si + 1],
                                    g.px[si + 2],
                                );
                            }
                        }
                    }
                }
                return Ok(());
            }
            glyph_index = Some(gi);
        }

        // Composite payload header.
        if r.remaining() < 12 {
            return Ok(());
        }
        let residual = r.u32()?;
        let bands = r.u32()?;
        let subcodec = r.u32()?;

        if residual > 0 {
            self.residual(&mut r, residual, nw, nh, dst, dw, dh, x0, y0)?;
        }
        if bands > 0 {
            self.bands(&mut r, bands, dst, dw, dh, x0, y0)?;
        }
        if subcodec > 0 {
            self.subcodecs(&mut r, subcodec, dst, dw, dh, x0, y0)?;
        }

        // Store the freshly-decoded rectangle into the glyph cache.
        if let Some(gi) = glyph_index {
            let count = nw as usize * nh as usize;
            let g = &mut self.glyphs[gi];
            g.px.clear();
            g.px.resize(count * 4, 0);
            g.count = count as u32;
            for row in 0..nh as usize {
                let sy = y0 as usize + row;
                if sy >= dh as usize {
                    break;
                }
                for col in 0..nw as usize {
                    let sx = x0 as usize + col;
                    if sx >= dw as usize {
                        break;
                    }
                    let di = (sy * dw as usize + sx) * 4;
                    let gi2 = (row * nw as usize + col) * 4;
                    if di + 4 <= dst.len() && gi2 + 4 <= g.px.len() {
                        g.px[gi2..gi2 + 4].copy_from_slice(&dst[di..di + 4]);
                    }
                }
            }
        }

        Ok(())
    }

    fn residual(
        &mut self,
        r: &mut Reader<'_>,
        byte_count: u32,
        nw: u16,
        nh: u16,
        dst: &mut [u8],
        dw: u16,
        dh: u16,
        x0: u16,
        y0: u16,
    ) -> Result<(), String> {
        let pixel_count = nw as u32 * nh as u32;
        let end = (r.pos + byte_count as usize).min(r.d.len());
        let mut idx = 0u32;
        while r.pos < end {
            let b = r.u8()?;
            let g = r.u8()?;
            let rr = r.u8()?;
            let mut run = r.u8()? as u32;
            if run >= 0xFF {
                run = r.u16()? as u32;
                if run >= 0xFFFF {
                    run = r.u32()?;
                }
            }
            for _ in 0..run {
                if idx >= pixel_count {
                    break;
                }
                let col = idx % nw as u32;
                let row = idx / nw as u32;
                put(dst, dw, dh, x0 as u32 + col, y0 as u32 + row, rr, g, b);
                idx += 1;
            }
        }
        Ok(())
    }

    fn subcodecs(
        &mut self,
        r: &mut Reader<'_>,
        byte_count: u32,
        dst: &mut [u8],
        dw: u16,
        dh: u16,
        x0: u16,
        y0: u16,
    ) -> Result<(), String> {
        let end = (r.pos + byte_count as usize).min(r.d.len());
        while r.pos < end {
            let xs = r.u16()?;
            let ys = r.u16()?;
            let w = r.u16()?;
            let h = r.u16()?;
            let bcount = r.u32()? as usize;
            let sid = r.u8()?;
            let xrel = x0 as u32 + xs as u32;
            let yrel = y0 as u32 + ys as u32;
            let data_end = (r.pos + bcount).min(r.d.len());
            match sid {
                0 => {
                    // Uncompressed BGR24, w*h*3 bytes.
                    for row in 0..h as u32 {
                        for col in 0..w as u32 {
                            let b = r.u8()?;
                            let g = r.u8()?;
                            let rr = r.u8()?;
                            put(dst, dw, dh, xrel + col, yrel + row, rr, g, b);
                        }
                    }
                    r.pos = data_end;
                }
                2 => {
                    self.rlex(r, bcount, w, h, dst, dw, dh, xrel, yrel)?;
                    r.pos = data_end;
                }
                1 => {
                    // NSCodec (MS-RDPNSC). This server uses it heavily for UI.
                    let seg = &r.d[r.pos..data_end];
                    match crate::nscodec::decode(seg, w, h) {
                        Ok(rgba) if !rgba.is_empty() => {
                            for row in 0..h as u32 {
                                for col in 0..w as u32 {
                                    let o = ((row * w as u32 + col) * 4) as usize;
                                    if o + 4 <= rgba.len() {
                                        put(
                                            dst,
                                            dw,
                                            dh,
                                            xrel + col,
                                            yrel + row,
                                            rgba[o],
                                            rgba[o + 1],
                                            rgba[o + 2],
                                        );
                                    }
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!("ClearCodec: NSCodec decode failed {w}x{h}: {e}");
                        }
                    }
                    r.pos = data_end;
                }
                _ => {
                    // Unknown subcodec — leave region untouched, warn.
                    warn!("ClearCodec: subcodec sid={sid} skipped {w}x{h} at ({xrel},{yrel})");
                    r.skip(bcount);
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn rlex(
        &mut self,
        r: &mut Reader<'_>,
        byte_count: usize,
        w: u16,
        h: u16,
        dst: &mut [u8],
        dw: u16,
        dh: u16,
        x0: u32,
        y0: u32,
    ) -> Result<(), String> {
        let end = (r.pos + byte_count).min(r.d.len());
        let palette_count = r.u8()? as usize;
        if palette_count == 0 || palette_count > 127 {
            return Err(format!("rlex paletteCount {palette_count}"));
        }
        let mut palette = vec![(0u8, 0u8, 0u8); palette_count];
        for slot in palette.iter_mut() {
            let b = r.u8()?;
            let g = r.u8()?;
            let rr = r.u8()?;
            *slot = (rr, g, b);
        }
        let num_bits = floor_log2(palette_count as u32 - 1) + 1;
        let pixel_count = w as u32 * h as u32;
        let (mut x, mut y, mut pidx) = (0u32, 0u32, 0u32);
        while r.pos < end {
            let tmp = r.u8()? as u32;
            let mut run = r.u8()? as u32;
            let suite_depth = (tmp >> num_bits) & mask(8 - num_bits);
            let stop = tmp & mask(num_bits);
            let start = stop as i64 - suite_depth as i64;
            if run >= 0xFF {
                run = r.u16()? as u32;
                if run >= 0xFFFF {
                    run = r.u32()?;
                }
            }
            if start < 0 || stop as usize >= palette_count {
                return Err("rlex index".into());
            }
            let start = start as usize;
            let (rr, g, b) = palette[start];
            for _ in 0..run {
                if pidx >= pixel_count {
                    break;
                }
                put(dst, dw, dh, x0 + x, y0 + y, rr, g, b);
                pidx += 1;
                x += 1;
                if x >= w as u32 {
                    y += 1;
                    x = 0;
                }
            }
            let mut si = start;
            for _ in 0..=suite_depth {
                if pidx >= pixel_count || si >= palette_count {
                    break;
                }
                let (rr, g, b) = palette[si];
                put(dst, dw, dh, x0 + x, y0 + y, rr, g, b);
                si += 1;
                pidx += 1;
                x += 1;
                if x >= w as u32 {
                    y += 1;
                    x = 0;
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn bands(
        &mut self,
        r: &mut Reader<'_>,
        byte_count: u32,
        dst: &mut [u8],
        dw: u16,
        dh: u16,
        x0: u16,
        y0: u16,
    ) -> Result<(), String> {
        let end = (r.pos + byte_count as usize).min(r.d.len());
        while r.pos < end {
            let xstart = r.u16()?;
            let xend = r.u16()?;
            let ystart = r.u16()?;
            let yend = r.u16()?;
            let cb = r.u8()?;
            let cg = r.u8()?;
            let cr = r.u8()?;
            if xend < xstart || yend < ystart {
                return Err("band range".into());
            }
            let vbar_count = (xend - xstart) as u32 + 1;
            let vbar_height = (yend - ystart) as u32 + 1;
            if vbar_height > 52 {
                return Err(format!("vBarHeight {vbar_height}"));
            }

            for i in 0..vbar_count {
                let header = r.u16()?;
                let mut update = false;
                let mut von = 0u32;
                let mut short_count = 0u32;
                let mut short_idx: Option<usize> = None;
                let mut full_idx: usize = 0;

                if header & 0xC000 == 0x4000 {
                    // SHORT_VBAR_CACHE_HIT
                    let idx = (header & 0x3FFF) as usize;
                    von = r.u8()? as u32;
                    short_count = self.short_vbar[idx].count;
                    short_idx = Some(idx);
                    update = true;
                } else if header & 0xC000 == 0x0000 {
                    // SHORT_VBAR_CACHE_MISS
                    von = (header & 0xFF) as u32;
                    let voff = ((header >> 8) & 0x3F) as u32;
                    if voff < von {
                        return Err("vBarYOff < vBarYOn".into());
                    }
                    short_count = voff - von;
                    if short_count > 52 {
                        return Err(format!("shortPixelCount {short_count}"));
                    }
                    let mut px = vec![0u8; short_count as usize * 4];
                    for k in 0..short_count as usize {
                        let b = r.u8()?;
                        let g = r.u8()?;
                        let rr = r.u8()?;
                        px[k * 4] = rr;
                        px[k * 4 + 1] = g;
                        px[k * 4 + 2] = b;
                        px[k * 4 + 3] = 255;
                    }
                    let cur = self.short_cursor;
                    self.short_vbar[cur] = VBar {
                        px,
                        count: short_count,
                    };
                    short_idx = Some(cur);
                    self.short_cursor = (cur + 1) % SHORT_VBAR_SIZE;
                    update = true;
                } else if header & 0x8000 == 0x8000 {
                    // VBAR_CACHE_HIT (full)
                    let idx = (header & 0x7FFF) as usize;
                    if idx >= VBAR_SIZE {
                        return Err(format!("vBar idx {idx}"));
                    }
                    if self.vbar[idx].count == 0 {
                        // Desync: the server referenced a full vBar we don't have
                        // cached. Fall back to the band BACKGROUND colour rather
                        // than fabricating BLACK (which showed as stark black
                        // blocks over UI elements). warn so the divergence is
                        // visible in logs.
                        warn!("ClearCodec: empty full-vBar idx={idx} h={vbar_height} — filling bg");
                        let mut px = vec![0u8; vbar_height as usize * 4];
                        for yy in 0..vbar_height as usize {
                            let o = yy * 4;
                            px[o] = cr;
                            px[o + 1] = cg;
                            px[o + 2] = cb;
                            px[o + 3] = 255;
                        }
                        self.vbar[idx] = VBar {
                            px,
                            count: vbar_height,
                        };
                    }
                    full_idx = idx;
                } else {
                    return Err(format!("vBarHeader {header:#06x}"));
                }

                if update {
                    // Build a full vBar: bkg above, short pixels, bkg below.
                    let mut px = vec![0u8; vbar_height as usize * 4];
                    let top = von.min(vbar_height);
                    for yy in 0..top {
                        let o = yy as usize * 4;
                        px[o] = cr;
                        px[o + 1] = cg;
                        px[o + 2] = cb;
                        px[o + 3] = 255;
                    }
                    let mid_end = (von + short_count).min(vbar_height);
                    if let Some(si) = short_idx {
                        let se = &self.short_vbar[si];
                        let mut k = 0usize;
                        for yy in von..mid_end {
                            let o = yy as usize * 4;
                            let so = k * 4;
                            if so + 4 <= se.px.len() {
                                px[o] = se.px[so];
                                px[o + 1] = se.px[so + 1];
                                px[o + 2] = se.px[so + 2];
                                px[o + 3] = 255;
                            }
                            k += 1;
                        }
                    }
                    for yy in mid_end..vbar_height {
                        let o = yy as usize * 4;
                        px[o] = cr;
                        px[o + 1] = cg;
                        px[o + 2] = cb;
                        px[o + 3] = 255;
                    }
                    let cur = self.vbar_cursor;
                    self.vbar[cur] = VBar {
                        px,
                        count: vbar_height,
                    };
                    self.vbar_cursor = (cur + 1) % VBAR_SIZE;
                    full_idx = cur;
                }

                // Compose the column onto the destination.
                let fe = &self.vbar[full_idx];
                let count = fe.count.min(vbar_height);
                let dx = x0 as u32 + xstart as u32 + i;
                for yy in 0..count {
                    let o = yy as usize * 4;
                    if o + 4 > fe.px.len() {
                        break;
                    }
                    put(
                        dst,
                        dw,
                        dh,
                        dx,
                        y0 as u32 + ystart as u32 + yy,
                        fe.px[o],
                        fe.px[o + 1],
                        fe.px[o + 2],
                    );
                }
            }
        }
        Ok(())
    }
}
