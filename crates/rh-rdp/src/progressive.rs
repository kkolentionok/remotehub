//! RemoteFX Progressive (WireToSurface2, codec2) decoder.
//!
//! TILE_FIRST/SIMPLE: RLGR1 → per-subband dequant → reduce-extrapolate inverse
//! DWT → YCbCr→RGB. TILE_UPGRADE: SRL/RAW bit-plane refinement of the retained
//! per-tile coefficients (+ sign), then re-run the inverse DWT. Per-tile state
//! (current coeffs + sign + bitPos) persists channel-wide across frames.
//!
//! Reference: MS-RDPEGFX + FreeRDP libfreerdp/codec/progressive.c (Apache-2.0);
//! reimplemented in Rust. RLGR + YCbCr→RGB reuse ironrdp-graphics.

use std::collections::HashMap;

use ironrdp::graphics::color_conversion::{ycbcr_to_rgba, YCbCrBuffer};
use ironrdp::graphics::rlgr;
use ironrdp::pdu::codecs::rfx::EntropyAlgorithm;
use tracing::info;

const WBT_SYNC: u16 = 0xCCC0;
const WBT_FRAME_BEGIN: u16 = 0xCCC1;
const WBT_FRAME_END: u16 = 0xCCC2;
const WBT_CONTEXT: u16 = 0xCCC3;
const WBT_REGION: u16 = 0xCCC4;
const WBT_TILE_SIMPLE: u16 = 0xCCC5;
const WBT_TILE_FIRST: u16 = 0xCCC6;
const WBT_TILE_UPGRADE: u16 = 0xCCC7;

const RFX_TILE_DIFFERENCE: u8 = 0x01;

pub struct DecodedTile {
    pub x: u16,
    pub y: u16,
    pub rgba: Vec<u8>, // 64*64*4
}

/// Per-band values (10 subbands), used for quant / shift / bitPos / numBits.
#[derive(Clone, Copy, Default)]
struct Quant {
    hl1: i32,
    lh1: i32,
    hh1: i32,
    hl2: i32,
    lh2: i32,
    hh2: i32,
    hl3: i32,
    lh3: i32,
    hh3: i32,
    ll3: i32,
}

impl Quant {
    fn map2(&self, p: &Quant, f: impl Fn(i32, i32) -> i32) -> Quant {
        Quant {
            hl1: f(self.hl1, p.hl1),
            lh1: f(self.lh1, p.lh1),
            hh1: f(self.hh1, p.hh1),
            hl2: f(self.hl2, p.hl2),
            lh2: f(self.lh2, p.lh2),
            hh2: f(self.hh2, p.hh2),
            hl3: f(self.hl3, p.hl3),
            lh3: f(self.lh3, p.lh3),
            hh3: f(self.hh3, p.hh3),
            ll3: f(self.ll3, p.ll3),
        }
    }
    /// bitPos = quant + progQuant
    fn add(&self, p: &Quant) -> Quant {
        self.map2(p, |a, b| a + b)
    }
    /// shift = quant + progQuant - 1 (clamped >= 0)
    fn shift(&self, p: &Quant) -> Quant {
        self.map2(p, |a, b| (a + b - 1).max(0))
    }
    /// numBits = oldBitPos - newBitPos (clamped >= 0)
    fn sub_clamp(&self, p: &Quant) -> Quant {
        self.map2(p, |a, b| (a - b).max(0))
    }
}

/// Persistent per-component state across frames.
struct Comp {
    current: Vec<i16>, // 4096 post-dequant coefficients
    sign: Vec<i16>,    // 4096 sign/value tracking for upgrade
    bitpos: Quant,
    seen_first: bool,
}
impl Comp {
    fn new() -> Self {
        Self {
            current: vec![0; 4096],
            sign: vec![0; 4096],
            bitpos: Quant::default(),
            seen_first: false,
        }
    }
}

struct TileState {
    y: Comp,
    cb: Comp,
    cr: Comp,
}
impl TileState {
    fn new() -> Self {
        Self {
            y: Comp::new(),
            cb: Comp::new(),
            cr: Comp::new(),
        }
    }
}

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
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        if self.pos + 2 > self.d.len() {
            return None;
        }
        let v = u16::from_le_bytes([self.d[self.pos], self.d[self.pos + 1]]);
        self.pos += 2;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        if self.pos + 4 > self.d.len() {
            return None;
        }
        let v = u32::from_le_bytes([
            self.d[self.pos],
            self.d[self.pos + 1],
            self.d[self.pos + 2],
            self.d[self.pos + 3],
        ]);
        self.pos += 4;
        Some(v)
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.pos + n > self.d.len() {
            return None;
        }
        let s = &self.d[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        if self.pos + n > self.d.len() {
            return None;
        }
        self.pos += n;
        Some(())
    }
}

/// MSB-first bit reader over a byte slice (mirrors FreeRDP wBitStream reads).
struct BitReader<'a> {
    d: &'a [u8],
    pos: usize, // bit position
}
impl<'a> BitReader<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    #[inline]
    fn bit(&mut self) -> u32 {
        let byte = self.d.get(self.pos >> 3).copied().unwrap_or(0);
        let b = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        b as u32
    }
    #[inline]
    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }
}

/// SRL (simplified run-length) bit-plane decoder state.
struct SrlState {
    kp: i32,
    nz: i32,
    mode: u32,
}
impl SrlState {
    fn new() -> Self {
        Self {
            kp: 8,
            nz: 0,
            mode: 0,
        }
    }
    fn read(&mut self, srl: &mut BitReader<'_>, num_bits: u32) -> i16 {
        if self.nz > 0 {
            self.nz -= 1;
            return 0;
        }
        let k = (self.kp / 8) as u32;
        if self.mode == 0 {
            let bit = srl.bit();
            if bit == 0 {
                self.nz = 1i32 << k;
                self.kp += 4;
                if self.kp > 80 {
                    self.kp = 80;
                }
                self.nz -= 1;
                return 0;
            } else {
                self.nz = 0;
                self.mode = 1;
                if k > 0 {
                    self.nz = srl.bits(k) as i32;
                }
                if self.nz > 0 {
                    self.nz -= 1;
                    return 0;
                }
            }
        }
        self.mode = 0;
        let sign = srl.bit();
        if self.kp < 6 {
            self.kp = 0;
        } else {
            self.kp -= 6;
        }
        if num_bits == 1 {
            return if sign == 1 { -1 } else { 1 };
        }
        let mut mag = 1u32;
        let max = (1u32 << num_bits) - 1;
        while mag < max {
            if srl.bit() == 1 {
                break;
            }
            mag += 1;
        }
        let mag = mag.min(32767) as i16;
        if sign == 1 {
            -mag
        } else {
            mag
        }
    }
}

fn read_quant(r: &mut Reader<'_>) -> Option<Quant> {
    let b0 = r.u8()? as i32;
    let b1 = r.u8()? as i32;
    let b2 = r.u8()? as i32;
    let b3 = r.u8()? as i32;
    let b4 = r.u8()? as i32;
    Some(Quant {
        ll3: b0 & 0x0F,
        hl3: b0 >> 4,
        lh3: b1 & 0x0F,
        hh3: b1 >> 4,
        hl2: b2 & 0x0F,
        lh2: b2 >> 4,
        hh2: b3 & 0x0F,
        hl1: b3 >> 4,
        lh1: b4 & 0x0F,
        hh1: b4 >> 4,
    })
}

#[inline]
fn clamp16(v: i32) -> i16 {
    v.clamp(-32768, 32767) as i16
}
#[inline]
fn lshift(b: &mut [i16], s: i32) {
    if s > 0 {
        let s = s as u32;
        for x in b.iter_mut() {
            *x = ((*x as i32).wrapping_shl(s)) as i16;
        }
    }
}
#[inline]
fn diff_decode(b: &mut [i16]) {
    for i in 1..b.len() {
        b[i] = b[i].wrapping_add(b[i - 1]);
    }
}
fn band_l(level: usize) -> usize {
    (64 >> level) + 1
}
fn band_h(level: usize) -> usize {
    if level == 1 {
        31
    } else {
        (64 + (1usize << (level - 1))) >> level
    }
}

// (offset, len) of each subband in the reduce-extrapolate packing.
const BANDS: [(usize, usize); 10] = [
    (0, 1023),    // HL1
    (1023, 1023), // LH1
    (2046, 961),  // HH1
    (3007, 272),  // HL2
    (3279, 272),  // LH2
    (3551, 256),  // HH2
    (3807, 72),   // HL3
    (3879, 72),   // LH3
    (3951, 64),   // HH3
    (4015, 81),   // LL3
];

fn band_shifts(q: &Quant) -> [i32; 10] {
    [
        q.hl1, q.lh1, q.hh1, q.hl2, q.lh2, q.hh2, q.hl3, q.lh3, q.hh3, q.ll3,
    ]
}

pub struct ProgressiveDecoder {
    tiles: HashMap<u32, TileState>,
    logged: u64,
}

impl ProgressiveDecoder {
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
            logged: 0,
        }
    }

    /// Drop all persistent per-tile state. Retained for a future context-aware
    /// reset; not currently called (baseline tracking makes a blanket reset
    /// unnecessary and it caused gray tiles on differential frames).
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.tiles.clear();
    }

    pub fn decode(&mut self, data: &[u8]) -> Vec<DecodedTile> {
        let mut out = Vec::new();
        let mut r = Reader::new(data);
        let mut summary = String::new();

        while r.remaining() >= 6 {
            let start = r.pos;
            let Some(block_type) = r.u16() else { break };
            let Some(block_len) = r.u32() else { break };
            if block_len < 6 {
                break;
            }
            let block_end = start + block_len as usize;
            if block_end > data.len() {
                break;
            }
            match block_type {
                WBT_SYNC | WBT_FRAME_BEGIN | WBT_FRAME_END | WBT_CONTEXT => {}
                WBT_REGION => self.decode_region(&mut r, block_end, &mut out, &mut summary),
                _ => {}
            }
            r.pos = block_end;
        }

        if self.logged < 4 {
            self.logged += 1;
            info!(
                "GFX Progressive: {}B -> {} tiles decoded {}",
                data.len(),
                out.len(),
                summary
            );
        }
        out
    }

    fn decode_region(
        &mut self,
        r: &mut Reader<'_>,
        region_end: usize,
        out: &mut Vec<DecodedTile>,
        summary: &mut String,
    ) {
        let Some(_tile_size) = r.u8() else { return };
        let Some(num_rects) = r.u16() else { return };
        let Some(num_quant) = r.u8() else { return };
        let Some(num_prog_quant) = r.u8() else { return };
        let Some(flags) = r.u8() else { return };
        let Some(_num_tiles) = r.u16() else { return };
        let Some(_tile_data_size) = r.u32() else { return };
        if flags & 0x01 == 0 {
            return; // only reduce-extrapolate supported
        }
        if r.skip(num_rects as usize * 8).is_none() {
            return;
        }
        let mut quants = Vec::with_capacity(num_quant as usize);
        for _ in 0..num_quant {
            match read_quant(r) {
                Some(q) => quants.push(q),
                None => return,
            }
        }
        let mut prog = Vec::with_capacity(num_prog_quant as usize);
        for _ in 0..num_prog_quant {
            let Some(_quality) = r.u8() else { return };
            let (Some(y), Some(cb), Some(cr)) = (read_quant(r), read_quant(r), read_quant(r)) else {
                return;
            };
            prog.push((y, cb, cr));
        }
        let full = Quant::default();

        let (mut n_first, mut n_up) = (0u32, 0u32);
        while r.pos + 6 <= region_end && r.remaining() >= 6 {
            let start = r.pos;
            let Some(bt) = r.u16() else { break };
            let Some(bl) = r.u32() else { break };
            if bl < 6 {
                break;
            }
            let tend = start + bl as usize;
            if tend > region_end {
                break;
            }
            match bt {
                WBT_TILE_SIMPLE | WBT_TILE_FIRST => {
                    if self.tile_first(bt, r, &quants, &prog, &full, out) {
                        n_first += 1;
                    }
                }
                WBT_TILE_UPGRADE => {
                    if self.tile_upgrade(r, &quants, &prog, &full, out) {
                        n_up += 1;
                    }
                }
                _ => {}
            }
            r.pos = tend;
        }
        if summary.is_empty() {
            summary.push_str(&format!(
                "region(q={} pq={} first={} upgrade={})",
                num_quant, num_prog_quant, n_first, n_up
            ));
        }
    }

    fn prog_for(
        prog: &[(Quant, Quant, Quant)],
        full: &Quant,
        quality: u8,
    ) -> (Quant, Quant, Quant) {
        if quality == 0xFF {
            (*full, *full, *full)
        } else {
            prog.get(quality as usize)
                .copied()
                .unwrap_or((*full, *full, *full))
        }
    }

    fn tile_first(
        &mut self,
        block_type: u16,
        r: &mut Reader<'_>,
        quants: &[Quant],
        prog: &[(Quant, Quant, Quant)],
        full: &Quant,
        out: &mut Vec<DecodedTile>,
    ) -> bool {
        let simple = block_type == WBT_TILE_SIMPLE;
        let (Some(qiy), Some(qicb), Some(qicr)) = (r.u8(), r.u8(), r.u8()) else {
            return false;
        };
        let (Some(x_idx), Some(y_idx)) = (r.u16(), r.u16()) else {
            return false;
        };
        let Some(flags) = r.u8() else { return false };
        let quality = if simple {
            0xFF
        } else {
            match r.u8() {
                Some(q) => q,
                None => return false,
            }
        };
        let (Some(yl), Some(cbl), Some(crl), Some(_tl)) = (r.u16(), r.u16(), r.u16(), r.u16()) else {
            return false;
        };
        let (Some(yd), Some(cbd), Some(crd)) =
            (r.bytes(yl as usize), r.bytes(cbl as usize), r.bytes(crl as usize))
        else {
            return false;
        };
        let (Some(&qy), Some(&qcb), Some(&qcr)) = (
            quants.get(qiy as usize),
            quants.get(qicb as usize),
            quants.get(qicr as usize),
        ) else {
            return false;
        };
        let (py, pcb, pcr) = Self::prog_for(prog, full, quality);
        let diff = flags & RFX_TILE_DIFFERENCE != 0;

        let key = ((y_idx as u32) << 16) | x_idx as u32;
        let ts = self.tiles.entry(key).or_insert_with(TileState::new);

        let mut sy = [0i16; 4096];
        let mut scb = [0i16; 4096];
        let mut scr = [0i16; 4096];
        let ok = first_component(yd, &qy.shift(&py), &mut ts.y, diff, &mut sy)
            && first_component(cbd, &qcb.shift(&pcb), &mut ts.cb, diff, &mut scb)
            && first_component(crd, &qcr.shift(&pcr), &mut ts.cr, diff, &mut scr);
        if !ok {
            return false;
        }
        ts.y.bitpos = qy.add(&py);
        ts.cb.bitpos = qcb.add(&pcb);
        ts.cr.bitpos = qcr.add(&pcr);

        emit_tile(x_idx, y_idx, &sy, &scb, &scr, out)
    }

    fn tile_upgrade(
        &mut self,
        r: &mut Reader<'_>,
        quants: &[Quant],
        prog: &[(Quant, Quant, Quant)],
        full: &Quant,
        out: &mut Vec<DecodedTile>,
    ) -> bool {
        let (Some(qiy), Some(qicb), Some(qicr)) = (r.u8(), r.u8(), r.u8()) else {
            return false;
        };
        let (Some(x_idx), Some(y_idx)) = (r.u16(), r.u16()) else {
            return false;
        };
        let Some(quality) = r.u8() else { return false };
        let (Some(ysl), Some(yrl), Some(cbsl), Some(cbrl), Some(crsl), Some(crrl)) =
            (r.u16(), r.u16(), r.u16(), r.u16(), r.u16(), r.u16())
        else {
            return false;
        };
        let (Some(ys), Some(yr), Some(cbs), Some(cbr), Some(crs), Some(crr)) = (
            r.bytes(ysl as usize),
            r.bytes(yrl as usize),
            r.bytes(cbsl as usize),
            r.bytes(cbrl as usize),
            r.bytes(crsl as usize),
            r.bytes(crrl as usize),
        ) else {
            return false;
        };
        let (Some(&qy), Some(&qcb), Some(&qcr)) = (
            quants.get(qiy as usize),
            quants.get(qicb as usize),
            quants.get(qicr as usize),
        ) else {
            return false;
        };
        let (py, pcb, pcr) = Self::prog_for(prog, full, quality);

        let key = ((y_idx as u32) << 16) | x_idx as u32;
        let Some(ts) = self.tiles.get_mut(&key) else {
            return false; // no FIRST seen for this tile yet
        };
        if !ts.y.seen_first {
            return false;
        }

        let mut sy = [0i16; 4096];
        let mut scb = [0i16; 4096];
        let mut scr = [0i16; 4096];
        upgrade_component(&qy, &py, &mut ts.y, ys, yr, &mut sy);
        upgrade_component(&qcb, &pcb, &mut ts.cb, cbs, cbr, &mut scb);
        upgrade_component(&qcr, &pcr, &mut ts.cr, crs, crr, &mut scr);

        emit_tile(x_idx, y_idx, &sy, &scb, &scr, out)
    }
}

fn emit_tile(
    x_idx: u16,
    y_idx: u16,
    sy: &[i16],
    scb: &[i16],
    scr: &[i16],
    out: &mut Vec<DecodedTile>,
) -> bool {
    let mut rgba = vec![0u8; 64 * 64 * 4];
    let buf = YCbCrBuffer {
        y: sy,
        cb: scb,
        cr: scr,
    };
    if ycbcr_to_rgba(buf, &mut rgba).is_err() {
        return false;
    }
    out.push(DecodedTile {
        x: x_idx.saturating_mul(64),
        y: y_idx.saturating_mul(64),
        rgba,
    });
    true
}

/// TILE_FIRST component: RLGR1 → dequant → inverse DWT. Records sign + current.
fn first_component(data: &[u8], shift: &Quant, comp: &mut Comp, diff: bool, out: &mut [i16]) -> bool {
    let mut buf = [0i16; 4096];
    if rlgr::decode(EntropyAlgorithm::Rlgr1, data, &mut buf).is_err() {
        return false;
    }

    let shifts = band_shifts(shift);
    for (i, &(off, len)) in BANDS.iter().enumerate() {
        if i == 9 {
            diff_decode(&mut buf[off..off + len]); // LL3 differential
        }
        lshift(&mut buf[off..off + len], shifts[i]);
    }

    if diff {
        // RFX_TILE_DIFFERENCE: this tile's coefficients are a delta from the
        // previous frame's. Reconstruct the full coefficients...
        for i in 0..4096 {
            buf[i] = buf[i].wrapping_add(comp.current[i]);
        }
    }
    // ...and ALWAYS store the reconstructed coefficients as the new baseline,
    // both for the next differential FIRST and for UPGRADE refinement. Updating
    // only on the non-diff path (the previous bug) made each diff frame stack on
    // a stale baseline -> drift -> white/garbage blocks.
    comp.current[..4096].copy_from_slice(&buf[..4096]);
    // `sign` carries the SRL significance state for upgrades (upgrade_block only
    // reads its sign: >0 / <0 / ==0). It must reflect the ACCUMULATED
    // coefficients, not this frame's raw delta — otherwise differential tiles
    // refine against the wrong significance map and leave thin edge artifacts.
    comp.sign[..4096].copy_from_slice(&buf[..4096]);

    let mut temp = [0i16; 4096];
    dwt_block(&mut buf, 3807, &mut temp, 3);
    dwt_block(&mut buf, 3007, &mut temp, 2);
    dwt_block(&mut buf, 0, &mut temp, 1);
    out[..4096].copy_from_slice(&buf[..4096]);
    comp.seen_first = true;
    true
}

/// TILE_UPGRADE component: SRL/RAW bit-plane refine of `comp.current` (+sign),
/// then inverse DWT into `out`.
fn upgrade_component(q: &Quant, p: &Quant, comp: &mut Comp, srl_data: &[u8], raw_data: &[u8], out: &mut [i16]) {
    let new_bitpos = q.add(p);
    let num_bits = comp.bitpos.sub_clamp(&new_bitpos);
    let shift = q.shift(p);
    let nb = band_shifts(&num_bits);
    let sh = band_shifts(&shift);

    let mut srl = BitReader::new(srl_data);
    let mut raw = BitReader::new(raw_data);
    let mut st = SrlState::new();

    for (i, &(off, len)) in BANDS.iter().enumerate() {
        let non_ll = i != 9;
        upgrade_block(
            &mut comp.current[off..off + len],
            &mut comp.sign[off..off + len],
            sh[i],
            nb[i] as u32,
            non_ll,
            &mut st,
            &mut srl,
            &mut raw,
        );
    }
    comp.bitpos = new_bitpos;

    // reverse=TRUE: buffer = current, then inverse DWT.
    let mut buf = [0i16; 4096];
    buf.copy_from_slice(&comp.current[..4096]);
    let mut temp = [0i16; 4096];
    dwt_block(&mut buf, 3807, &mut temp, 3);
    dwt_block(&mut buf, 3007, &mut temp, 2);
    dwt_block(&mut buf, 0, &mut temp, 1);
    out[..4096].copy_from_slice(&buf[..4096]);
}

#[allow(clippy::too_many_arguments)]
fn upgrade_block(
    buf: &mut [i16],
    sign: &mut [i16],
    shift: i32,
    num_bits: u32,
    non_ll: bool,
    st: &mut SrlState,
    srl: &mut BitReader<'_>,
    raw: &mut BitReader<'_>,
) {
    if num_bits < 1 {
        return;
    }
    if !non_ll {
        for x in buf.iter_mut() {
            let input = raw.bits(num_bits) as i32;
            *x = (*x as i32).wrapping_add(input.wrapping_shl(shift as u32)) as i16;
        }
        return;
    }
    for i in 0..buf.len() {
        let input: i32 = if sign[i] > 0 {
            raw.bits(num_bits) as i32
        } else if sign[i] < 0 {
            -(raw.bits(num_bits) as i32)
        } else {
            let v = st.read(srl, num_bits) as i32;
            sign[i] = clamp16(v);
            v
        };
        buf[i] = (buf[i] as i32).wrapping_add(input.wrapping_shl(shift as u32)) as i16;
    }
}

fn dwt_block(buf: &mut [i16], base: usize, temp: &mut [i16], level: usize) {
    let bl = band_l(level);
    let bh = band_h(level);
    let step = bl + bh;
    let hl = base;
    let lh = base + bh * bl;
    let hh = base + bh * bl + bl * bh;
    let ll = base + bh * bl + bl * bh + bh * bh;
    let l_off = 0usize;
    let h_off = bl * step;
    idwt_x(buf, ll, bl, buf, hl, bh, temp, l_off, step, bl, bh, bl);
    idwt_x(buf, lh, bl, buf, hh, bh, temp, h_off, step, bl, bh, bh);
    idwt_y(temp, l_off, step, temp, h_off, step, buf, base, step, bl, bh, bl + bh);
}

#[allow(clippy::too_many_arguments)]
fn idwt_x(
    low: &[i16],
    low_off: usize,
    low_step: usize,
    high: &[i16],
    high_off: usize,
    high_step: usize,
    dst: &mut [i16],
    dst_off: usize,
    dst_step: usize,
    lc: usize,
    hc: usize,
    dc: usize,
) {
    for i in 0..dc {
        let mut lp = low_off + i * low_step;
        let mut hp = high_off + i * high_step;
        let mut xp = dst_off + i * dst_step;
        let mut h0 = high[hp] as i32;
        hp += 1;
        let mut l0 = low[lp] as i32;
        lp += 1;
        let mut x0 = clamp16(l0 - h0) as i32;
        let mut x2 = clamp16(l0 - h0) as i32;
        for _ in 0..(hc - 1) {
            let h1 = high[hp] as i32;
            hp += 1;
            l0 = low[lp] as i32;
            lp += 1;
            x2 = clamp16(l0 - ((h0 + h1) / 2)) as i32;
            let x1 = clamp16((x0 + x2) / 2 + 2 * h0) as i32;
            dst[xp] = x0 as i16;
            dst[xp + 1] = x1 as i16;
            xp += 2;
            x0 = x2;
            h0 = h1;
        }
        if lc <= hc + 1 {
            if lc <= hc {
                dst[xp] = x2 as i16;
                dst[xp + 1] = clamp16(x2 + 2 * h0);
            } else {
                l0 = low[lp] as i32;
                lp += 1;
                x0 = clamp16(l0 - h0) as i32;
                dst[xp] = x2 as i16;
                dst[xp + 1] = clamp16((x0 + x2) / 2 + 2 * h0);
                dst[xp + 2] = x0 as i16;
            }
        } else {
            l0 = low[lp] as i32;
            lp += 1;
            x0 = clamp16(l0 - (h0 / 2)) as i32;
            dst[xp] = x2 as i16;
            dst[xp + 1] = clamp16((x0 + x2) / 2 + 2 * h0);
            dst[xp + 2] = x0 as i16;
            l0 = low[lp] as i32;
            dst[xp + 3] = clamp16((x0 + l0) / 2);
        }
        let _ = (lp, hp);
    }
}

#[allow(clippy::too_many_arguments)]
fn idwt_y(
    low: &[i16],
    low_off: usize,
    low_step: usize,
    high: &[i16],
    high_off: usize,
    high_step: usize,
    dst: &mut [i16],
    dst_off: usize,
    dst_step: usize,
    lc: usize,
    hc: usize,
    dc: usize,
) {
    for i in 0..dc {
        let mut lp = low_off + i;
        let mut hp = high_off + i;
        let mut xp = dst_off + i;
        let mut h0 = high[hp] as i32;
        hp += high_step;
        let mut l0 = low[lp] as i32;
        lp += low_step;
        let mut x0 = clamp16(l0 - h0) as i32;
        let mut x2 = clamp16(l0 - h0) as i32;
        for _ in 0..(hc - 1) {
            let h1 = high[hp] as i32;
            hp += high_step;
            l0 = low[lp] as i32;
            lp += low_step;
            x2 = clamp16(l0 - ((h0 + h1) / 2)) as i32;
            let x1 = clamp16((x0 + x2) / 2 + 2 * h0) as i32;
            dst[xp] = x0 as i16;
            xp += dst_step;
            dst[xp] = x1 as i16;
            xp += dst_step;
            x0 = x2;
            h0 = h1;
        }
        if lc <= hc + 1 {
            if lc <= hc {
                dst[xp] = x2 as i16;
                xp += dst_step;
                dst[xp] = clamp16(x2 + 2 * h0);
            } else {
                l0 = low[lp] as i32;
                x0 = clamp16(l0 - h0) as i32;
                dst[xp] = x2 as i16;
                xp += dst_step;
                dst[xp] = clamp16((x0 + x2) / 2 + 2 * h0);
                xp += dst_step;
                dst[xp] = x0 as i16;
            }
        } else {
            l0 = low[lp] as i32;
            lp += low_step;
            x0 = clamp16(l0 - (h0 / 2)) as i32;
            dst[xp] = x2 as i16;
            xp += dst_step;
            dst[xp] = clamp16((x0 + x2) / 2 + 2 * h0);
            xp += dst_step;
            dst[xp] = x0 as i16;
            xp += dst_step;
            l0 = low[lp] as i32;
            dst[xp] = clamp16((x0 + l0) / 2);
        }
        let _ = (lp, hp, xp);
    }
}
