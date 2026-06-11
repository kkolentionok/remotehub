//! Stage 4 — GFX (MS-RDPEGFX) graphics pipeline, **Slice 2a**.
//!
//! Hosts the `Microsoft::Windows::RDS::Graphics` DVC, advertises GFX caps so
//! the server drives the graphics pipeline, then decodes the server's surface
//! commands into an RGBA framebuffer (`GfxState`) shared with the worker loop.
//! The loop ships the framebuffer's changed rectangles through the SAME
//! region-encode/`FrameBatch` transport as the legacy path — so the frontend
//! is unchanged for the non-AVC codecs.
//!
//! Codec coverage so far:
//!   - Uncompressed (0x0)  ✅  raw BGRA → surface
//!   - SolidFill           ✅  fill rects
//!   - CreateSurface / DeleteSurface / MapSurfaceToOutput / ResetGraphics ✅
//!   - ClearCodec / Planar / Progressive / AVC  ⬜  counted + skipped (next slices)
//!
//! The decoded picture is therefore only partial until ClearCodec lands
//! (Slice 3) — Windows uses ClearCodec for most UI. This slice proves the
//! surface model + framebuffer + transport wiring end to end.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ironrdp::core::{impl_as_any, Decode as _, Encode, EncodeResult, ReadCursor, WriteCursor};
use ironrdp::dvc::{DvcClientProcessor, DvcEncode, DvcMessage, DvcProcessor};
use ironrdp::graphics::zgfx::Decompressor;
use ironrdp::pdu::rdp::vc::dvc::gfx::{
    Avc420BitmapStream, Avc444BitmapStream, CapabilitiesAdvertisePdu, CapabilitiesV103Flags,
    CapabilitiesV104Flags, CapabilitiesV10Flags, CapabilitiesV81Flags, CapabilitiesV8Flags,
    CapabilitySet, ClientPdu, Codec1Type, Codec2Type, Encoding, FrameAcknowledgePdu, QueueDepth,
    ServerPdu,
};
use ironrdp::pdu::PduResult;
use tracing::{info, warn};

const CHANNEL_NAME: &str = "Microsoft::Windows::RDS::Graphics";

// ---------------------------------------------------------------------------
// Shared framebuffer state (worker loop ships from it; the DVC processor — run
// from inside active_stage.process — decodes into it).
// ---------------------------------------------------------------------------

struct Surface {
    w: u16,
    h: u16,
    px: Vec<u8>, // RGBA
}

pub struct GfxState {
    pub w: u16,
    pub h: u16,
    /// Composited screen, RGBA, `w*h*4` (matches DecodedImage RgbA32 so the
    /// existing `make_region` extractor works unchanged).
    pub fb: Vec<u8>,
    /// Screen-space changed rectangles since the loop last drained them.
    pub dirty: Vec<(u16, u16, u16, u16)>,
    surfaces: HashMap<u16, Surface>,
    origin: HashMap<u16, (u32, u32)>,
    /// GFX bitmap cache: cache_slot -> cached tile (RGBA). Filled by
    /// SurfaceToCache, stamped by CacheToSurface. The server uses this to fill
    /// large/repeating areas (e.g. the desktop background) cheaply.
    cache: HashMap<u16, CachedTile>,
}

struct CachedTile {
    w: u16,
    h: u16,
    px: Vec<u8>, // RGBA
}

impl GfxState {
    pub fn new() -> Self {
        Self {
            w: 0,
            h: 0,
            fb: Vec::new(),
            dirty: Vec::new(),
            surfaces: HashMap::new(),
            origin: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    pub fn ready(&self) -> bool {
        self.w as usize * self.h as usize > 0 && !self.fb.is_empty()
    }

    fn set_output(&mut self, w: u16, h: u16) {
        self.w = w;
        self.h = h;
        self.fb = vec![0u8; w as usize * h as usize * 4];
        self.dirty.clear();
        self.dirty.push((0, 0, w, h));
    }

    fn create_surface(&mut self, id: u16, w: u16, h: u16) {
        // If the server re-announces an existing surface with the same size,
        // keep its pixels. Re-zeroing here black-holes any area that the server
        // then expects to restore from the bitmap cache but doesn't re-stamp,
        // and causes a visible flash. Only allocate fresh on new id / new dims.
        if let Some(existing) = self.surfaces.get(&id) {
            if existing.w == w && existing.h == h {
                return;
            }
        }
        self.surfaces.insert(
            id,
            Surface {
                w,
                h,
                px: vec![0u8; w as usize * h as usize * 4],
            },
        );
    }

    /// Mirror a sub-rectangle of a surface onto the composited framebuffer (if
    /// the surface is output-mapped) and record the screen-space dirty rect.
    /// `rx,ry,rw,rh` are in surface space.
    fn propagate(&mut self, id: u16, rx: u16, ry: u16, rw: u16, rh: u16) {
        let Some((ox, oy)) = self.origin.get(&id).copied() else {
            return; // not mapped to the output yet
        };
        let Some(s) = self.surfaces.get(&id) else {
            return;
        };
        if self.fb.is_empty() {
            return;
        }
        let sx0 = (ox as i64 + rx as i64).max(0) as usize; // screen x of rect origin
        let sy0 = (oy as i64 + ry as i64).max(0) as usize;
        let fbw = self.w as usize;
        let fbh = self.h as usize;
        let sw = s.w as usize;
        let mut copied_w = 0usize;
        let mut copied_h = 0usize;
        for row in 0..rh as usize {
            let dy = sy0 + row;
            if dy >= fbh {
                break;
            }
            let src_y = ry as usize + row;
            if src_y >= s.h as usize {
                break;
            }
            let max_w = (fbw.saturating_sub(sx0)).min(rw as usize);
            let max_w = max_w.min(sw.saturating_sub(rx as usize));
            if max_w == 0 {
                continue;
            }
            let src_off = (src_y * sw + rx as usize) * 4;
            let dst_off = (dy * fbw + sx0) * 4;
            self.fb[dst_off..dst_off + max_w * 4]
                .copy_from_slice(&s.px[src_off..src_off + max_w * 4]);
            copied_w = copied_w.max(max_w);
            copied_h = row + 1;
        }
        if copied_w > 0 && copied_h > 0 {
            self.dirty
                .push((sx0 as u16, sy0 as u16, copied_w as u16, copied_h as u16));
        }
    }

    /// Blit a 64x64 RGBA tile onto a surface at (dx, dy), clamped. Returns the
    /// written rect for propagation.
    fn blit_rgba_tile(&mut self, id: u16, dx: u16, dy: u16, rgba: &[u8]) -> Option<(u16, u16, u16, u16)> {
        let s = self.surfaces.get_mut(&id)?;
        let (sw, sh) = (s.w as usize, s.h as usize);
        let mut cw = 0usize;
        let mut ch = 0usize;
        for row in 0..64usize {
            let py = dy as usize + row;
            if py >= sh {
                break;
            }
            let copy = 64usize.min(sw.saturating_sub(dx as usize));
            if copy == 0 {
                continue;
            }
            let src = row * 64 * 4;
            let dst = (py * sw + dx as usize) * 4;
            if src + copy * 4 <= rgba.len() && dst + copy * 4 <= s.px.len() {
                s.px[dst..dst + copy * 4].copy_from_slice(&rgba[src..src + copy * 4]);
                cw = cw.max(copy);
                ch = row + 1;
            }
        }
        if cw > 0 {
            Some((dx, dy, cw as u16, ch as u16))
        } else {
            None
        }
    }

    /// Copy a rect of a surface into the bitmap cache slot.
    fn surface_to_cache(&mut self, id: u16, slot: u16, rx: u16, ry: u16, rw: u16, rh: u16) {
        let Some(s) = self.surfaces.get(&id) else {
            return;
        };
        let (sw, sh) = (s.w as usize, s.h as usize);
        let (rw, rh) = (rw as usize, rh as usize);
        let mut px = vec![0u8; rw * rh * 4];
        for row in 0..rh {
            let sy = ry as usize + row;
            if sy >= sh {
                break;
            }
            let copy_w = rw.min(sw.saturating_sub(rx as usize));
            if copy_w == 0 {
                continue;
            }
            let src = (sy * sw + rx as usize) * 4;
            let dst = row * rw * 4;
            px[dst..dst + copy_w * 4].copy_from_slice(&s.px[src..src + copy_w * 4]);
        }
        self.cache.insert(
            slot,
            CachedTile {
                w: rw as u16,
                h: rh as u16,
                px,
            },
        );
    }

    /// Center-pixel RGB of a cached tile — trace only.
    fn cache_center_rgb(&self, slot: u16) -> Option<(u8, u8, u8)> {
        let t = self.cache.get(&slot)?;
        if t.w == 0 || t.h == 0 {
            return None;
        }
        let i = ((t.h as usize / 2) * t.w as usize + t.w as usize / 2) * 4;
        Some((*t.px.get(i)?, *t.px.get(i + 1)?, *t.px.get(i + 2)?))
    }

    /// Stamp a cached tile onto a surface at (dx, dy). Returns the written rect
    /// (for propagation) on success.
    fn cache_to_surface(&mut self, slot: u16, id: u16, dx: u16, dy: u16) -> Option<(u16, u16, u16, u16)> {
        let tile = self.cache.get(&slot)?;
        let (tw, th) = (tile.w as usize, tile.h as usize);
        let s = self.surfaces.get_mut(&id)?;
        let (sw, sh) = (s.w as usize, s.h as usize);
        for row in 0..th {
            let py = dy as usize + row;
            if py >= sh {
                break;
            }
            let copy_w = tw.min(sw.saturating_sub(dx as usize));
            if copy_w == 0 {
                continue;
            }
            let src = row * tw * 4;
            let dst = (py * sw + dx as usize) * 4;
            s.px[dst..dst + copy_w * 4].copy_from_slice(&tile.px[src..src + copy_w * 4]);
        }
        Some((dx, dy, tile.w, tile.h))
    }

    /// Copy a source rect of one surface to (dx, dy) on another (or the same)
    /// surface. Used for scrolling. Goes via a temp buffer so overlapping
    /// same-surface copies don't corrupt.
    fn surface_to_surface(
        &mut self,
        src_id: u16,
        dst_id: u16,
        rx: u16,
        ry: u16,
        rw: u16,
        rh: u16,
        dx: u16,
        dy: u16,
    ) {
        let Some(src) = self.surfaces.get(&src_id) else {
            return;
        };
        let (ssw, ssh) = (src.w as usize, src.h as usize);
        let (rw, rh) = (rw as usize, rh as usize);
        let mut tmp = vec![0u8; rw * rh * 4];
        for row in 0..rh {
            let sy = ry as usize + row;
            if sy >= ssh {
                break;
            }
            let copy_w = rw.min(ssw.saturating_sub(rx as usize));
            if copy_w == 0 {
                continue;
            }
            let so = (sy * ssw + rx as usize) * 4;
            let to = row * rw * 4;
            tmp[to..to + copy_w * 4].copy_from_slice(&src.px[so..so + copy_w * 4]);
        }
        let Some(dst) = self.surfaces.get_mut(&dst_id) else {
            return;
        };
        let (dsw, dsh) = (dst.w as usize, dst.h as usize);
        for row in 0..rh {
            let py = dy as usize + row;
            if py >= dsh {
                break;
            }
            let copy_w = rw.min(dsw.saturating_sub(dx as usize));
            if copy_w == 0 {
                continue;
            }
            let to = row * rw * 4;
            let doff = (py * dsw + dx as usize) * 4;
            dst.px[doff..doff + copy_w * 4].copy_from_slice(&tmp[to..to + copy_w * 4]);
        }
    }

    /// Write an uncompressed BGRA(/X) rectangle into a surface.
    fn write_uncompressed(&mut self, id: u16, rx: u16, ry: u16, rw: u16, rh: u16, data: &[u8]) {
        let Some(s) = self.surfaces.get_mut(&id) else {
            return;
        };
        let sw = s.w as usize;
        let need = rw as usize * rh as usize * 4;
        if data.len() < need {
            warn!(
                "GFX: uncompressed short ({} < {}) surf={id}",
                data.len(),
                need
            );
            return;
        }
        for row in 0..rh as usize {
            let dy = ry as usize + row;
            if dy >= s.h as usize {
                break;
            }
            for col in 0..rw as usize {
                let dx = rx as usize + col;
                if dx >= sw {
                    break;
                }
                let si = (row * rw as usize + col) * 4;
                let di = (dy * sw + dx) * 4;
                // wire is BGRA → store RGBA
                s.px[di] = data[si + 2];
                s.px[di + 1] = data[si + 1];
                s.px[di + 2] = data[si];
                s.px[di + 3] = 255;
            }
        }
    }

    fn solid_fill(&mut self, id: u16, r: u8, g: u8, b: u8, rx: u16, ry: u16, rw: u16, rh: u16) {
        let Some(s) = self.surfaces.get_mut(&id) else {
            return;
        };
        let sw = s.w as usize;
        for row in 0..rh as usize {
            let dy = ry as usize + row;
            if dy >= s.h as usize {
                break;
            }
            for col in 0..rw as usize {
                let dx = rx as usize + col;
                if dx >= sw {
                    break;
                }
                let di = (dy * sw + dx) * 4;
                s.px[di] = r;
                s.px[di + 1] = g;
                s.px[di + 2] = b;
                s.px[di + 3] = 255;
            }
        }
    }
}

/// RDPGFX_RECT16 → (x, y, w, h). GFX rectangles are **half-open**: `right`
/// and `bottom` are one past the last pixel (despite IronRDP naming the type
/// `InclusiveRectangle`). So width = right - left, height = bottom - top — no
/// +1, or every rect would be 1px too wide/tall and raster layers would shear.
fn rect_xywh(left: u16, top: u16, right: u16, bottom: u16) -> (u16, u16, u16, u16) {
    (
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

/// Decode one AVC420 bitmap stream into a surface: feed the H.264 packet to
/// the per-surface decoder, then blit each metadata region rect of the decoded
/// picture onto the surface (and propagate to the output framebuffer).
///
/// Free function (not a method) on purpose: the caller holds the `GfxState`
/// mutex guard (an immutable borrow of `self.state`), so it can only lend out
/// *disjoint* `self` fields (`self.avc`, `self.avc_warns`) — a `&mut self`
/// method would conflict with the live guard.
fn apply_avc420(
    decoders: &mut HashMap<u16, openh264::decoder::Decoder>,
    st: &mut GfxState,
    surface_id: u16,
    stream: &Avc420BitmapStream<'_>,
    warns: &mut u64,
) {
    use openh264::formats::YUVSource as _;
    use std::collections::hash_map::Entry;

    let dec = match decoders.entry(surface_id) {
        Entry::Occupied(e) => e.into_mut(),
        Entry::Vacant(v) => match openh264::decoder::Decoder::new() {
            Ok(d) => v.insert(d),
            Err(e) => {
                *warns += 1;
                if *warns <= 3 {
                    warn!("GFX: openh264 decoder init failed: {e}");
                }
                return;
            }
        },
    };

    match dec.decode(stream.data) {
        Ok(Some(yuv)) => {
            let (fw, fh) = yuv.dimensions();
            let (ys, us, vs) = yuv.strides();
            let (yp, up, vp) = (yuv.y(), yuv.u(), yuv.v());
            // Blit ONLY the regionRects the server flagged as changed. Blitting
            // the whole decoded picture is wrong: the H.264 reference frame
            // keeps the last-encoded content for regions the server later
            // repaints via a *different* codec (ClearCodec/Cache2S, e.g. the
            // wallpaper revealed when a window closes). A full blit would stamp
            // that stale reference back over the correct non-AVC pixels — which
            // re-creates the very "ghost" of a closed window we were chasing.
            for r in &stream.rectangles {
                let (x, y, w, h) = rect_xywh(r.left, r.top, r.right, r.bottom);
                blit_avc_rect(st, surface_id, yp, up, vp, ys, us, vs, fw, fh, x, y, w, h);
                st.propagate(surface_id, x, y, w, h);
            }
        }
        // Parameter sets / partial data only — no displayable picture yet.
        Ok(None) => {}
        Err(e) => {
            *warns += 1;
            if *warns <= 5 || *warns % 240 == 0 {
                warn!("GFX: AVC decode failed ({} total): {e}", *warns);
            }
        }
    }
}

/// Copy one region rect of a decoded YUV420 picture onto a surface as RGBA.
/// Conversion is BT.709 full-range with the same fixed-point coefficients
/// FreeRDP uses for RDP AVC streams. Clamped to both the picture (`fw`/`fh`,
/// which are 16-aligned and may exceed the surface) and the surface bounds.
#[allow(clippy::too_many_arguments)]
fn blit_avc_rect(
    st: &mut GfxState,
    id: u16,
    yp: &[u8],
    up: &[u8],
    vp: &[u8],
    ys: usize,
    us: usize,
    vs: usize,
    fw: usize,
    fh: usize,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
) {
    let Some(s) = st.surfaces.get_mut(&id) else {
        return;
    };
    let (sw, sh) = (s.w as usize, s.h as usize);
    let x0 = x as usize;
    if x0 >= sw || x0 >= fw {
        return;
    }
    for row in 0..h as usize {
        let py = y as usize + row;
        if py >= sh || py >= fh {
            break;
        }
        let max_w = (w as usize)
            .min(sw.saturating_sub(x0))
            .min(fw.saturating_sub(x0));
        if max_w == 0 {
            continue;
        }
        let yrow = &yp[py * ys..];
        let urow = &up[(py / 2) * us..];
        let vrow = &vp[(py / 2) * vs..];
        let dst = &mut s.px[(py * sw + x0) * 4..(py * sw + x0 + max_w) * 4];
        for col in 0..max_w {
            let px = x0 + col;
            let yv = i32::from(yrow[px]);
            let u = i32::from(urow[px / 2]) - 128;
            let v = i32::from(vrow[px / 2]) - 128;
            let r = (256 * yv + 403 * v + 128) >> 8;
            let g = (256 * yv - 48 * u - 120 * v + 128) >> 8;
            let b = (256 * yv + 475 * u + 128) >> 8;
            let o = col * 4;
            dst[o] = r.clamp(0, 255) as u8;
            dst[o + 1] = g.clamp(0, 255) as u8;
            dst[o + 2] = b.clamp(0, 255) as u8;
            dst[o + 3] = 255;
        }
    }
}

// ---------------------------------------------------------------------------
// DVC processor
// ---------------------------------------------------------------------------

/// Wrapper so we can satisfy the `DvcEncode` marker for the foreign
/// `gfx::ClientPdu` (orphan rule).
struct GfxClientMsg(ClientPdu);
impl Encode for GfxClientMsg {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        self.0.encode(dst)
    }
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn size(&self) -> usize {
        self.0.size()
    }
}
impl DvcEncode for GfxClientMsg {}
fn boxed(pdu: ClientPdu) -> DvcMessage {
    Box::new(GfxClientMsg(pdu))
}

pub struct GraphicsPipeline {
    state: Arc<Mutex<GfxState>>,
    zgfx: Decompressor,
    clear: crate::clearcodec::ClearDecoder,
    prog: crate::progressive::ProgressiveDecoder,
    frames: u32,
    skipped_codec: u64,
    dbg_n: u64,
    dbg_pdu: u64,
    dbg_cache: u64,
    op_counts: std::collections::BTreeMap<&'static str, u32>,
    last_op_log: std::time::Instant,
    trace: bool,
    /// Per-surface H.264 decoders for the AVC420/AVC444 GFX path. Each surface
    /// carries its own independent H.264 bitstream; created lazily on the first
    /// AVC PDU for a surface, dropped on DeleteSurface / CreateSurface (a fresh
    /// surface starts a fresh stream with a new IDR).
    avc: HashMap<u16, openh264::decoder::Decoder>,
    avc_warns: u64,
    avc_started: bool,
}

impl GraphicsPipeline {
    pub fn new(state: Arc<Mutex<GfxState>>) -> Self {
        Self {
            state,
            zgfx: Decompressor::new(),
            clear: crate::clearcodec::ClearDecoder::new(),
            prog: crate::progressive::ProgressiveDecoder::new(),
            frames: 0,
            skipped_codec: 0,
            dbg_n: 0,
            dbg_pdu: 0,
            dbg_cache: 0,
            op_counts: std::collections::BTreeMap::new(),
            last_op_log: std::time::Instant::now(),
            // RDP_GFX_TRACE=1 -> unbounded geometry trace of the ops involved in
            // window-move disocclusion (S2S / CacheToSurface / SolidFill /
            // uncompressed). Off by default.
            trace: std::env::var("RDP_GFX_TRACE").is_ok(),
            avc: HashMap::new(),
            avc_warns: 0,
            avc_started: false,
        }
    }

    /// Short tag for a ServerPdu variant — diagnostic op-count logging.
    fn pdu_name(p: &ServerPdu) -> &'static str {
        match p {
            ServerPdu::WireToSurface1(_) => "Wts1",
            ServerPdu::WireToSurface2(_) => "Wts2(prog)",
            ServerPdu::DeleteEncodingContext(_) => "DelEncCtx",
            ServerPdu::SolidFill(_) => "SolidFill",
            ServerPdu::SurfaceToSurface(_) => "S2S",
            ServerPdu::SurfaceToCache(_) => "S2Cache",
            ServerPdu::CacheToSurface(_) => "Cache2S",
            ServerPdu::EvictCacheEntry(_) => "Evict",
            ServerPdu::CreateSurface(_) => "Create",
            ServerPdu::DeleteSurface(_) => "Delete",
            ServerPdu::StartFrame(_) => "Start",
            ServerPdu::EndFrame(_) => "End",
            ServerPdu::ResetGraphics(_) => "Reset",
            ServerPdu::MapSurfaceToOutput(_) => "Map",
            ServerPdu::CapabilitiesConfirm(_) => "Caps",
            ServerPdu::CacheImportReply(_) => "ImportReply",
            ServerPdu::MapSurfaceToScaledOutput(_) => "MapScaledOut",
            ServerPdu::MapSurfaceToScaledWindow(_) => "MapScaledWin",
            #[allow(unreachable_patterns)]
            _ => "other",
        }
    }
}

impl_as_any!(GraphicsPipeline);

impl DvcProcessor for GraphicsPipeline {
    fn channel_name(&self) -> &str {
        CHANNEL_NAME
    }

    fn start(&mut self, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        let caps = vec![
            CapabilitySet::V8 {
                flags: CapabilitiesV8Flags::empty(),
            },
            CapabilitySet::V8_1 {
                flags: CapabilitiesV81Flags::AVC420_ENABLED,
            },
            CapabilitySet::V10 {
                flags: CapabilitiesV10Flags::empty(),
            },
            CapabilitySet::V10_2 {
                flags: CapabilitiesV10Flags::empty(),
            },
            CapabilitySet::V10_3 {
                flags: CapabilitiesV103Flags::empty(),
            },
            CapabilitySet::V10_4 {
                flags: CapabilitiesV104Flags::empty(),
            },
        ];
        info!("GFX: advertising {} capability sets", caps.len());
        Ok(vec![boxed(ClientPdu::CapabilitiesAdvertise(
            CapabilitiesAdvertisePdu(caps),
        ))])
    }

    fn process(&mut self, _channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        let mut decompressed = Vec::new();
        if let Err(e) = self.zgfx.decompress(payload, &mut decompressed) {
            warn!("GFX: zgfx decompress failed: {e}");
            return Ok(Vec::new());
        }

        let mut acks: Vec<DvcMessage> = Vec::new();
        let mut cur = ReadCursor::new(&decompressed);
        let mut st = self.state.lock().unwrap();

        while !cur.remaining().is_empty() {
            let pdu = match ServerPdu::decode(&mut cur) {
                Ok(p) => p,
                Err(e) => {
                    warn!("GFX: server PDU decode stopped: {e}");
                    break;
                }
            };

            *self.op_counts.entry(Self::pdu_name(&pdu)).or_insert(0) += 1;
            if self.last_op_log.elapsed() >= std::time::Duration::from_secs(1) {
                if self.trace {
                    let summary: Vec<String> =
                        self.op_counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    info!("GFX ops/sec: {}", summary.join(" "));
                }
                self.op_counts.clear();
                self.last_op_log = std::time::Instant::now();
            }

            match pdu {
                ServerPdu::CapabilitiesConfirm(c) => {
                    info!("GFX: server CONFIRMED caps {:?} — pipeline ACTIVE", c.0);
                }
                ServerPdu::ResetGraphics(r) => {
                    let w = r.width.min(u16::MAX as u32) as u16;
                    let h = r.height.min(u16::MAX as u32) as u16;
                    info!("GFX: ResetGraphics {w}x{h}");
                    st.set_output(w, h);
                }
                ServerPdu::CreateSurface(s) => {
                    info!(
                        "GFX: CreateSurface id={} {}x{} fmt={:?}",
                        s.surface_id, s.width, s.height, s.pixel_format
                    );
                    st.create_surface(s.surface_id, s.width, s.height);
                    // A (re)created surface starts a fresh H.264 stream (the
                    // server opens with a new IDR) — drop any old decoder state.
                    self.avc.remove(&s.surface_id);
                }
                ServerPdu::DeleteSurface(d) => {
                    st.surfaces.remove(&d.surface_id);
                    st.origin.remove(&d.surface_id);
                    self.avc.remove(&d.surface_id);
                }
                ServerPdu::MapSurfaceToOutput(m) => {
                    info!(
                        "GFX: MapSurfaceToOutput id={} -> origin ({},{})",
                        m.surface_id, m.output_origin_x, m.output_origin_y
                    );
                    st.origin
                        .insert(m.surface_id, (m.output_origin_x, m.output_origin_y));
                    // Blit the whole surface onto the output at its new origin.
                    let dims = st.surfaces.get(&m.surface_id).map(|s| (s.w, s.h));
                    if let Some((sw, sh)) = dims {
                        st.propagate(m.surface_id, 0, 0, sw, sh);
                    }
                }
                ServerPdu::SolidFill(f) => {
                    let (r, g, b) = (f.fill_pixel.r, f.fill_pixel.g, f.fill_pixel.b);
                    if self.trace {
                        let rs: Vec<String> = f
                            .rectangles
                            .iter()
                            .map(|rc| {
                                let (x, y, w, h) = rect_xywh(rc.left, rc.top, rc.right, rc.bottom);
                                format!("({x},{y},{w},{h})")
                            })
                            .collect();
                        info!("TRACE SolidFill surf={} rgb=({r},{g},{b}) rects={}", f.surface_id, rs.join(","));
                    }
                    for rect in &f.rectangles {
                        let (x, y, w, h) = rect_xywh(rect.left, rect.top, rect.right, rect.bottom);
                        st.solid_fill(f.surface_id, r, g, b, x, y, w, h);
                        st.propagate(f.surface_id, x, y, w, h);
                    }
                }
                ServerPdu::WireToSurface1(w) => {
                    let (x, y, rw, rh) = rect_xywh(
                        w.destination_rectangle.left,
                        w.destination_rectangle.top,
                        w.destination_rectangle.right,
                        w.destination_rectangle.bottom,
                    );
                    if self.trace {
                        info!(
                            "TRACE Wts1 surf={} codec={:?} dst=({x},{y},{rw},{rh})",
                            w.surface_id, w.codec_id
                        );
                    }
                    match w.codec_id {
                        Codec1Type::Uncompressed => {
                            st.write_uncompressed(w.surface_id, x, y, rw, rh, &w.bitmap_data);
                            st.propagate(w.surface_id, x, y, rw, rh);
                        }
                        Codec1Type::ClearCodec => {
                            let dims = st.surfaces.get(&w.surface_id).map(|s| (s.w, s.h));
                            if let Some((sw, sh)) = dims {
                                if self.trace && self.dbg_n < 8 {
                                    self.dbg_n += 1;
                                    let org = st.origin.get(&w.surface_id).copied();
                                    info!(
                                        "GFX: Wts1 ClearCodec surf={} rawrect[l={} t={} r={} b={}] -> xywh=({},{},{},{}) surf={}x{} origin={:?} fb={}x{}",
                                        w.surface_id,
                                        w.destination_rectangle.left,
                                        w.destination_rectangle.top,
                                        w.destination_rectangle.right,
                                        w.destination_rectangle.bottom,
                                        x, y, rw, rh,
                                        sw, sh, org, st.w, st.h
                                    );
                                }
                                if let Some(surf) = st.surfaces.get_mut(&w.surface_id) {
                                    if let Err(e) = self.clear.decode(
                                        &w.bitmap_data,
                                        rw,
                                        rh,
                                        &mut surf.px,
                                        sw,
                                        sh,
                                        x,
                                        y,
                                    ) {
                                        warn!("GFX: ClearCodec decode failed: {e}");
                                    }
                                }
                                st.propagate(w.surface_id, x, y, rw, rh);
                            }
                        }
                        Codec1Type::Avc420 => {
                            let mut acur = ReadCursor::new(&w.bitmap_data);
                            match Avc420BitmapStream::decode(&mut acur) {
                                Ok(s) => {
                                    if !self.avc_started {
                                        self.avc_started = true;
                                        info!("GFX: AVC stream ACTIVE (Avc420)");
                                    }
                                    if self.trace {
                                        let rs: Vec<String> = s
                                            .rectangles
                                            .iter()
                                            .take(6)
                                            .map(|r| {
                                                format!(
                                                    "({},{},{},{})",
                                                    r.left,
                                                    r.top,
                                                    r.right - r.left,
                                                    r.bottom - r.top
                                                )
                                            })
                                            .collect();
                                        info!(
                                            "TRACE Avc420 surf={} rects={} bytes={} rs=[{}]",
                                            w.surface_id,
                                            s.rectangles.len(),
                                            s.data.len(),
                                            rs.join(" ")
                                        );
                                    }
                                    apply_avc420(
                                        &mut self.avc,
                                        &mut st,
                                        w.surface_id,
                                        &s,
                                        &mut self.avc_warns,
                                    );
                                }
                                Err(e) => warn!("GFX: Avc420 meta decode failed: {e}"),
                            }
                        }
                        Codec1Type::Avc444 | Codec1Type::Avc444v2 => {
                            let mut acur = ReadCursor::new(&w.bitmap_data);
                            match Avc444BitmapStream::decode(&mut acur) {
                                Ok(s) => {
                                    if !self.avc_started {
                                        self.avc_started = true;
                                        info!(
                                            "GFX: AVC stream ACTIVE ({:?}, lc={:?})",
                                            w.codec_id, s.encoding
                                        );
                                    }
                                    if self.trace {
                                        info!(
                                            "TRACE Avc444 surf={} lc={:?} rects={} bytes1={} has2={}",
                                            w.surface_id,
                                            s.encoding,
                                            s.stream1.rectangles.len(),
                                            s.stream1.data.len(),
                                            s.stream2.is_some()
                                        );
                                    }
                                    // MVP: decode only the main (luma) view — it is a
                                    // complete 4:2:0 picture on its own. The auxiliary
                                    // chroma-upgrade stream is ignored for now, so
                                    // chroma stays 4:2:0 (visually: slightly softer
                                    // colour edges, text/luma fully sharp). LC==CHROMA
                                    // PDUs carry only the aux stream — skip them.
                                    if s.encoding != Encoding::CHROMA {
                                        apply_avc420(
                                            &mut self.avc,
                                            &mut st,
                                            w.surface_id,
                                            &s.stream1,
                                            &mut self.avc_warns,
                                        );
                                    }
                                }
                                Err(e) => warn!("GFX: Avc444 meta decode failed: {e}"),
                            }
                        }
                        _ => {
                            // Planar / RemoteFx — next slices.
                            self.skipped_codec = self.skipped_codec.wrapping_add(1);
                            if self.skipped_codec <= 3 || self.skipped_codec % 240 == 0 {
                                info!(
                                    "GFX: skip codec {:?} surf={} bytes={} (not yet implemented)",
                                    w.codec_id,
                                    w.surface_id,
                                    w.bitmap_data.len()
                                );
                            }
                        }
                    }
                }
                ServerPdu::StartFrame(_) => {}
                ServerPdu::DeleteEncodingContext(_) => {
                    // Intentionally a no-op now. We previously reset the
                    // Progressive decoder here, but that zeroed the per-tile
                    // coefficient baseline; a subsequent differential FIRST then
                    // reconstructed against zero -> neutral (128,128,128) gray
                    // tiles. With correct baseline tracking in first_component,
                    // stale state no longer accumulates, so no reset is needed.
                }
                ServerPdu::EndFrame(end) => {
                    self.frames = self.frames.wrapping_add(1);
                    acks.push(boxed(ClientPdu::FrameAcknowledge(FrameAcknowledgePdu {
                        queue_depth: QueueDepth::Unavailable,
                        frame_id: end.frame_id,
                        total_frames_decoded: self.frames,
                    })));
                }
                ServerPdu::SurfaceToCache(s) => {
                    let (rx, ry, rw, rh) = rect_xywh(
                        s.source_rectangle.left,
                        s.source_rectangle.top,
                        s.source_rectangle.right,
                        s.source_rectangle.bottom,
                    );
                    if self.trace && self.dbg_cache < 40 {
                        self.dbg_cache += 1;
                        info!(
                            "GFX: S2Cache slot={} key={} src[l={} t={} r={} b={}] -> xywh=({},{},{},{})",
                            s.cache_slot,
                            s.cache_key,
                            s.source_rectangle.left,
                            s.source_rectangle.top,
                            s.source_rectangle.right,
                            s.source_rectangle.bottom,
                            rx, ry, rw, rh
                        );
                    }
                    st.surface_to_cache(s.surface_id, s.cache_slot, rx, ry, rw, rh);
                }
                ServerPdu::CacheToSurface(c) => {
                    if self.trace {
                        let pts: Vec<String> =
                            c.destination_points.iter().map(|p| format!("({},{})", p.x, p.y)).collect();
                        let col = st
                            .cache_center_rgb(c.cache_slot)
                            .map(|(r, g, b)| format!("({r},{g},{b})"))
                            .unwrap_or_else(|| "none".into());
                        info!(
                            "TRACE Cache2S slot={} surf={} center_rgb={} dst_pts={}",
                            c.cache_slot, c.surface_id, col, pts.join(",")
                        );
                    }
                    for p in &c.destination_points {
                        match st.cache_to_surface(c.cache_slot, c.surface_id, p.x, p.y) {
                            Some((x, y, w, h)) => st.propagate(c.surface_id, x, y, w, h),
                            None => {
                                *self.op_counts.entry("Cache2S_MISS").or_insert(0) += 1;
                            }
                        }
                    }
                }
                ServerPdu::SurfaceToSurface(s) => {
                    let (rx, ry, rw, rh) = rect_xywh(
                        s.source_rectangle.left,
                        s.source_rectangle.top,
                        s.source_rectangle.right,
                        s.source_rectangle.bottom,
                    );
                    if self.trace {
                        let pts: Vec<String> =
                            s.destination_points.iter().map(|p| format!("({},{})", p.x, p.y)).collect();
                        info!(
                            "TRACE S2S src={}->dst={} srcrect=({},{},{},{}) dst_pts={}",
                            s.source_surface_id, s.destination_surface_id, rx, ry, rw, rh, pts.join(",")
                        );
                    }
                    for p in &s.destination_points {
                        st.surface_to_surface(
                            s.source_surface_id,
                            s.destination_surface_id,
                            rx,
                            ry,
                            rw,
                            rh,
                            p.x,
                            p.y,
                        );
                        st.propagate(s.destination_surface_id, p.x, p.y, rw, rh);
                    }
                }
                ServerPdu::WireToSurface2(w) => {
                    if w.codec_id == Codec2Type::RemoteFxProgressive {
                        let tiles = self.prog.decode(&w.bitmap_data);
                        if self.trace && !tiles.is_empty() {
                            let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
                            // Per-row coverage (y -> min..max x). The bbox alone
                            // hid whether Progressive actually repaints a given
                            // row of a vacated band; this lists the exact rows
                            // and their x-extent so a disocclusion gap is visible.
                            let mut rows: std::collections::BTreeMap<u16, (u16, u16)> =
                                std::collections::BTreeMap::new();
                            for t in &tiles {
                                x0 = x0.min(t.x);
                                y0 = y0.min(t.y);
                                x1 = x1.max(t.x + 64);
                                y1 = y1.max(t.y + 64);
                                let e = rows.entry(t.y).or_insert((u16::MAX, 0));
                                e.0 = e.0.min(t.x);
                                e.1 = e.1.max(t.x + 64);
                            }
                            let rowstr: Vec<String> = rows
                                .iter()
                                .map(|(y, (a, b))| format!("{y}:{a}-{b}"))
                                .collect();
                            info!(
                                "TRACE Wts2 prog surf={} tiles={} bbox=({x0},{y0},{},{}) rows=[{}]",
                                w.surface_id,
                                tiles.len(),
                                x1.saturating_sub(x0),
                                y1.saturating_sub(y0),
                                rowstr.join(" ")
                            );
                        }
                        for t in &tiles {
                            if let Some((x, y, tw, th)) =
                                st.blit_rgba_tile(w.surface_id, t.x, t.y, &t.rgba)
                            {
                                st.propagate(w.surface_id, x, y, tw, th);
                            }
                        }
                    } else if self.dbg_pdu < 20 {
                        self.dbg_pdu += 1;
                        info!(
                            "GFX: WireToSurface2 surf={} codec={:?} bytes={} — NOT handled",
                            w.surface_id,
                            w.codec_id,
                            w.bitmap_data.len()
                        );
                    }
                }
                ServerPdu::MapSurfaceToScaledOutput(m) => {
                    // Like MapSurfaceToOutput but the server also gives a target
                    // size for scaling. We render at native resolution and let
                    // the UI canvas scale, so the target size is irrelevant —
                    // map at the origin and blit the surface. Ignoring this (the
                    // old catch-all) left the surface unmapped → an unpainted /
                    // stale region (the "stuck strip").
                    st.origin
                        .insert(m.surface_id, (m.output_origin_x, m.output_origin_y));
                    let dims = st.surfaces.get(&m.surface_id).map(|s| (s.w, s.h));
                    if let Some((sw, sh)) = dims {
                        st.propagate(m.surface_id, 0, 0, sw, sh);
                    }
                }
                ServerPdu::EvictCacheEntry(e) => {
                    // Drop the cached tile so a later reuse of this slot can't
                    // stamp stale pixels (keeps our cache in lockstep with the
                    // server's eviction).
                    st.cache.remove(&e.cache_slot);
                }
                _ => {
                    // MapSurfaceToScaledWindow / CacheImportReply /
                    // DeleteEncodingContext — none repaint surface pixels, so
                    // ignoring them can't cause ghosting. Warn once if one ever
                    // shows up so we can revisit if a future server relies on it.
                    use std::sync::atomic::{AtomicBool, Ordering};
                    static WARNED: AtomicBool = AtomicBool::new(false);
                    if !WARNED.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            "GFX: an unhandled ServerPdu reached the catch-all (cache/scale/context op) — ignored"
                        );
                    }
                }
            }
        }

        Ok(acks)
    }
}

impl DvcClientProcessor for GraphicsPipeline {}
