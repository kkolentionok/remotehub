# RemoteHub — Project State & Handoff

**Last updated:** NSCodec decoder works — log clean, UI/colors correct, fps up to 20. One remaining GFX artifact: persistent band at the TOP of a window after maximize->restore (disocclusion; predates NSCodec) — awaiting a focused RDP_GFX_TRACE=1 capture of a single maximize->restore. Added a comprehensive docs/CHEATSHEET.md (full project briefing).

## Latest — NSCodec landed clean; chasing the maximize/restore top-of-window disocclusion

NSCodec decoder verified working first try: zero `subcodec skipped` / `NSCodec decode failed` / `empty full-vBar` warns, UI lines/borders/blocks render, colors correct, smooth (fps up to 20, encode ~6ms). The black blocks and UI-line artifacts are gone.

Remaining single artifact: maximize a window to fullscreen then restore -> a PERSISTENT band at the top of the window. This is the long-standing disocclusion class (vacated region keeps stale content in st.fb; the server never corrects it as it believes that area already shows background). Predates NSCodec.

To pinpoint without a blind fix, extended `RDP_GFX_TRACE` to also log Progressive: `TRACE Wts2 prog surf=.. tiles=.. bbox=(x,y,w,h)`. Now S2S / Cache2S / SolidFill / Wts1 (ClearCodec/Uncompressed) / Wts2 (Progressive) are all traced. Next: capture a single maximize->restore with RDP_GFX_TRACE=1 and inspect which op (if any) repaints the vacated top band; if none covers it, the divergence is upstream (our S2S/decode leaving stale that the server assumes is background). Fix follows from the capture.

## Latest — NSCodec decoder (the real cause of GFX UI artifacts)

User (rightly) refused to call GFX done while artifacts remain. The warn build revealed the truth: `ClearCodec: subcodec sid=1 skipped` floods the log during any UI activity. sid=1 = NSCodec (MS-RDPNSC). This Windows server uses NSCodec as its PRIMARY ClearCodec subcodec for UI (window edges, separators, buttons, gridlines — the thin strips AND the blocks). We skipped it, so those regions stayed black/stale, and the ClearCodec glyph cache (which reads back the surface) replayed the black on every HIT -> persistent black blocks + line artifacts. The `empty full-vBar` warn never fired, so the vbar cache is sound (its bg-fill change stays as a safety net).

Implemented NSCodec from scratch (`crates/rh-rdp/src/nscodec.rs`, ported from FreeRDP `nsc.c`, Apache-2.0):
- Header: PlaneByteCount[4]×u32 LE (Y/Co/Cg/A), ColorLossLevel u8 (1..=7), ChromaSubsamplingLevel u8, 2 reserved.
- Per-plane NSC RLE (`rle_decode`): literal / run ([v][v][len:u8+2] or [v][v][0xFF][len:u32]) / last 4 bytes raw; planeByteCount==0 -> 0xFF fill; planeByteCount>=orig -> raw copy.
- Plane sizing: tempW=round8(w), tempH=round2(h); subsampled chroma planes are (tempW/2)×(tempH/2), Y is tempW×h.
- Decode: shift=CLL-1; co/cg = sign_extend8((c<<shift)&0xFF); R=Y+Co-Cg, G=Y+Cg, B=Y-Co-Cg, clamp; A from plane 3; chroma supersampled by (x>>1),(y>>1) at stride tempW/2. Output RGBA (our surface order).
- Wired into `clearcodec::subcodecs` sid=1 (was a skip+warn); unknown sids still warn+skip.

Risk: ported codec, user compiles. If colors look swapped/shifted, suspect the co/cg sign-extend or the BGR vs RGB order. Perf: per-rect allocations, hundreds/sec — optimize later (reusable buffers) if it adds latency.

## Latest — ClearCodec black blocks: empty full-vBar -> band bg (not black)

User pushed back (rightly): GFX isn't done while artifacts remain (mstsc has none). New screenshot shows BLACK rectangles at UI elements (View menu, TASKS button) plus faint right-side move-trails.

Trace ruled out cache corruption — every `Cache2S center_rgb` is a plausible colour (blue/white/dark), so the `current`+`sign` fixes hold. The black is ClearCodec-specific:
- `bands`: a full `VBAR_CACHE_HIT` referencing a slot we have empty (`count==0`) fabricated a BLACK vBar (`vec![0u8;…]`) and painted it. Cache sizes are correct (32768/16384) so the ring wraps in lockstep — an empty slot means a real desync somewhere, but fabricating black is the worst possible fallback.
- `subcodecs`: sid=1 (NSCodec) is skipped (not decoded) -> region stays black.
- The glyph cache reads back the decoded rect from the surface, so any black region above is cached and replayed black on every glyph HIT -> persistent black blocks at fixed UI positions.

Fix this turn:
- Empty full-vBar -> fill with the band BACKGROUND colour (cr,cg,cb) instead of black, + `warn!` (bounds-check idx<VBAR_SIZE too). Filling bg blends into surroundings and propagates cleanly through the glyph cache.
- `warn!` on skipped NSCodec subcodec (rare; confirms if any UI traces back to it).

Next: user reports whether black blocks are gone and whether `empty full-vBar` warns fire (frequent warns => a real vBar-cursor desync to chase; none => black was NSCodec). Then tackle the move-trail disocclusion strips.

## Latest — RDP_GFX_TRACE geometry trace for persistent window-move strips

Confirmed (user): the strips PERSIST after the drag stops, on the side the window moved AWAY from — classic disocclusion-not-repainted. fb-diff ships st.fb, so the strips ARE in st.fb and the server never corrects them => our surface diverged from the server model there.

To find the exact culprit op without guessing (and without regressing the now-clean image), added `trace: bool` to GraphicsPipeline (env `RDP_GFX_TRACE=1`, off by default). When on it logs unbounded:
- `TRACE S2S src->dst srcrect dst_pts`
- `TRACE Cache2S slot surf center_rgb dst_pts`  (center_rgb tells if a white/window-edge tile is being stamped where wallpaper belongs)
- `TRACE SolidFill surf rgb rects`
- `TRACE Wts1 surf codec dst`
Re-added `cache_center_rgb` helper for the Cache2S trace.

Next: user runs with BOTH `$env:RDP_GFX=1; $env:RDP_GFX_TRACE=1`, moves a window to leave strips, sends the TRACE slice. Two outcomes: (a) a Cache2S stamps a white-center tile into the strip x -> cache-content divergence (cache captured our momentarily-wrong surface); (b) NO op covers the strip region -> earlier divergence / missing disocclusion handling. Fix follows from which.

## Latest — Progressive `sign`/SRL significance from accumulated coeffs (residual edge artifacts)

After the `comp.current` baseline fix the gray squares are essentially gone and the desktop is clean; only thin edge artifacts remained on dynamic (differential) content. `upgrade_block` reads `comp.sign` only for significance (>0/<0/==0) to drive SRL refinement of non-LL bands. `first_component` was capturing `sign` from the raw per-frame RLGR output (the delta) BEFORE dequant/accumulation, so on RFX_TILE_DIFFERENCE tiles the significance map reflected the delta, not the accumulated coefficients -> wrong SRL state -> thin edge errors.

Fix: capture `comp.sign` from the accumulated `buf` (post-dequant, post-LL3-diff, post-add), right next to `comp.current`. Equivalent for non-diff tiles (lshift preserves sign), correct for diff tiles. LL3 band unaffected (its upgrade uses the RAW codepath and never reads `sign`).

Remaining diagnostics: 1/s op counter + bounded first-40 S2Cache/Cache2S geometry logs (quiet after startup). Can strip fully when finalizing GFX.

## Latest — Progressive differential baseline fix (kills white drift AND gray squares)

User: "overall smooth now, but gray squares ruin it." Cache center-RGB logging showed the squares are tiles cached as exactly (128,128,128) = RemoteFX neutral (zero coefficients after level shift).

Root cause (one bug explaining both the earlier white/garbage blocks and the gray squares):
`progressive.rs::first_component` only wrote `comp.current` (the per-tile post-dequant coefficient baseline) on the **non-differential** path:
```
if diff { buf[i] += comp.current[i] }      // output = delta + old
else    { comp.current = buf }             // baseline updated ONLY here  <-- bug
```
- Without reset: every RFX_TILE_DIFFERENCE FIRST reconstructed delta+old for display but left `comp.current` stale, so the next diff stacked on an out-of-date baseline -> cumulative drift -> white/garbage blocks ("много кривизны").
- With the DelEncCtx `prog.reset()` band-aid (prev build): state was zeroed, so a differential FIRST did `buf = delta + 0` = just the small delta -> ~zero coefficients -> inverse DWT ~0 -> YCbCr(0,0,0) -> RGB (128,128,128) GRAY. Those gray tiles were then SurfaceToCache'd and stamped around -> gray squares.

Fix:
- `first_component`: after the optional diff add, ALWAYS `comp.current = buf` (store reconstructed coefficients pre-DWT) in both branches. Correct baseline -> diff frames track exactly -> no drift, no gray.
- `gfx.rs`: removed `DeleteEncodingContext -> prog.reset()` (now a documented no-op); the baseline fix makes a blanket reset unnecessary, and the reset was what produced gray. `reset()` kept as `#[allow(dead_code)]` for a possible future context-aware reset.
- Removed the flooding cache center-RGB + SolidFill diagnostic logs (the `<=40` gate never closed). Kept the 1/s op counter and the bounded (first-40) S2Cache/Cache2S geometry logs.

Expectation: smooth AND clean (no white drift, no gray squares). If a few gray remain, next suspect is the `sign`/SRL baseline not accumulating across differential firsts (upgrade refinement), but current is the dominant term.

## Latest — DeleteEncodingContext resets Progressive state (interactive corruption fix attempt)

Decision: not abandoning GFX. Decoders are pixel-correct (static is sharp); the defect is interactive. Cache log proved geometry clean (slot 2 = (0,0) 64x64 tiled screen-wide; uniform tiles; aligned; zero misses). Every individual op re-verified correct.

Strongest remaining lead: during interaction the server emits `DeleteEncodingContext` 1-10/s, which we ignored. Our `ProgressiveDecoder` keeps persistent per-tile state (`current`/`sign`/`bitpos`/`seen_first`, keyed by tile position). When the server tears down/reuses a codec context (new window, content change) and we keep stale state, the next FIRST/UPGRADE for that position decodes on top of stale bit-planes -> garbage tile -> SurfaceToCache captures it -> CacheToSurface replicates it across the screen. This matches "static clean / interactive corrupt" exactly (Progressive ops are few during interaction, but one bad tile is amplified by the cache).

Changes:
- `progressive.rs`: added `ProgressiveDecoder::reset()` (clears `tiles`).
- `gfx.rs`: explicit `DeleteEncodingContext` arm -> `self.prog.reset()` (no longer falls through to the catch-all). Safe: an UPGRADE arriving after a reset with no state is skipped (tile keeps its current rendered value), not corrupted.
- Diagnostics: after each `SurfaceToCache`, log the cached tile's center RGB (`cached slot=.. center_rgb=..`); log `SolidFill rgb=.. nrects=.. rects` — to confirm cached/background colours are sane (not black/white).

Next: user opens/moves windows, reports if artifacts shrank, and sends the `center_rgb` + `SolidFill` lines + whether the one-shot `unhandled ServerPdu` warn still fires (now only EvictCacheEntry/CacheImportReply/MapScaled remain in the catch-all — none paint).

## Latest — cache geometry logging (Cache2S_MISS=0, so pixels/coords are wrong)

User log (this server): no `Cache2S_MISS` ever -> all stamped slots are present, so we are NOT missing cache content; the cached pixels or the stamp/capture geometry are wrong. Artifact (screens): a vertical strip of a foreground window not restored (background Server Manager shows through Edge) + misplaced colour blocks -> coordinate/size defect, not a decode defect.

Verified: SurfaceToCache/CacheToSurface/SurfaceToSurface pixel copies correct; IronRDP field reads correct; rects via `rect_xywh` (half-open). An off-by-one on the InclusiveRectangle would only give 1px seams, not the wide strip seen — so the unknown is the actual tile sizes/coords the server sends.

Added (diagnostic, bounded to first 40 via `dbg_cache`):
- `GFX: S2Cache slot=.. key=.. src[l,t,r,b] -> xywh=(..)` — shows cached tile size + source.
- `GFX: Cache2S slot=.. surf=.. npts=.. first_pts=[..]` — shows stamp slot + destination points.

The initial connect burst (S2Cache~447, Cache2S~1975 in 1s) logs 40 samples immediately, so user only needs to connect + open a window. Correlate slot sizes (S2Cache) with stamp coords (Cache2S): if tiles are uniform 64x64 at aligned coords, geometry is right and corruption is upstream (a bad paint cached + replicated); if sizes/coords are irregular, that's the bug. Decide fix from the numbers.

## Latest — desktop is cache-composited; fix CreateSurface wipe + add cache-miss counter

Drag-time `GFX ops/sec` (this server): `Cache2S` 138-1973, `S2Cache` 10-447, `S2S` 0-16, `Wts1` 16-73, `Wts2(prog)` 5-18. The desktop/window content is restored and composited overwhelmingly via the **bitmap cache** (SurfaceToCache -> CacheToSurface), not by re-sending Progressive tiles. So the black rectangles = gaps/wrong pixels in our cache path, NOT a Progressive decode issue (static frames are sharp).

Findings + fixes:
- The log has a **2nd `CreateSurface id=0` (same 1828x1080)** shortly after the first. Our `create_surface` re-zeroed the surface to black each time; any region the server then expected to restore from cache but didn't re-stamp stayed black. **Fix:** `create_surface` now preserves pixels when the id already exists at the same dims (only allocates fresh on new id / new dims).
- Added `Cache2S_MISS` to the op counter: `cache_to_surface` returning None (slot not present) is counted. If this is high during a drag, the server is stamping slots we never populated (persistent-cache assumption / key mismatch / eviction-reuse bug); if it's ~0, the slots exist but the cached *pixels* are wrong (capture/stamp bug) and we go pixel-level next.
- `surface_to_cache`/`cache_to_surface` pixel copies and `surface_to_surface` (overlap-safe) re-verified correct; rects are half-open via `rect_xywh`.

Caps: we advertise V8/8.1(AVC420)/10/10.2/10.3/10.4; server confirms V10_4 and uses Progressive+cache (AVC unused). We send no CacheImportOffer, so the server shouldn't assume a persistent disk cache — `Cache2S_MISS` will confirm.

Next: user drags a window, sends `GFX ops/sec` (watch `Cache2S_MISS`) + says whether the black blocks shrank. Miss>0 -> handle persistent cache / fix slot keying. Miss~0 -> add pixel-level cache logging.

## Latest — diagnostic: per-second GFX op counter (chasing the residual move-trail)

fb-diff (prev turn) removed a lot of ghosting, but dragging/closing a window still leaves faint window fragments on the right of the screen. Since the emit is now a self-correcting fb diff, the duplicate must be IN `st.fb` — i.e. the vacated region is never repainted into our surface. Confirmed not a code bug in `surface_to_surface` (overlap-safe temp-buffer copy) or `propagate` (correct surface->fb rect copy). The unhandled ServerPdu variants (DeleteEncodingContext/EvictCacheEntry/CacheImportReply/MapSurfaceToScaled*) don't paint pixels.

So the background-restore mechanism is one of: `CacheToSurface` (stamp cached desktop back — possible bug in `cache_to_surface`), inter-surface `SurfaceToSurface` from an offscreen surface we never populated, or `Wts2`/`Wts1` repaints we decode wrong for the vacated tiles (stale per-tile Progressive state at a position that changed content).

Added (gfx.rs, diagnostic only):
- `GraphicsPipeline.op_counts: BTreeMap<&str,u32>` + `last_op_log: Instant`; `pdu_name()` tags each ServerPdu; once/sec logs `GFX ops/sec: Wts1=.. Wts2(prog)=.. S2S=.. Cache2S=.. SolidFill=.. ...` then resets.
- (prev) one-shot warn if any unhandled ServerPdu reaches the catch-all.

Next: user drags a window for a few seconds, sends the `GFX ops/sec` lines. Decide the fix from the dominant op:
- heavy `Cache2S` -> audit `cache_to_surface` stamping (rect/size/origin).
- heavy `S2S` with src_id != 0 -> we're not populating/creating that offscreen surface.
- heavy `Wts2`/`Wts1` over the vacated region but still ghosting -> stale Progressive per-tile state; reset tile state when a position gets a fresh TILE_FIRST.

## Latest — GFX emit: per-op dirty list -> self-correcting fb diff (ghosting fix)

User report: dynamic scenes ghost — dragging/opening a window leaves the old copy on screen (two overlapping Save-As dialogs), "artifacts everywhere", first paint slow, perceived quality poor. Static pages render sharp (verified prior turn), so the decode is fine; the defect is in how changes reach the canvas.

Root cause: the GFX emit path shipped `GfxState.dirty` (per-op rects recorded by `propagate`). That list is NOT self-correcting — if any fb change's dirty rect fails to reach the canvas, the stale area persists (ghost). All handlers do call `propagate`, and the only unhandled ServerPdu variants (DeleteEncodingContext, EvictCacheEntry, CacheImportReply, MapSurfaceToScaled*) don't repaint pixels, so the gap was in the dirty-list shipping itself, not a dropped op.

Fix (actor.rs):
- New `compute_regions_raw(data,w,h,last_frame)` — the same band diff as `compute_regions` but over a raw RGBA buffer.
- GFX emit branch now diffs `st.fb` vs `last_frame` (exactly like the proven legacy `image` path), advances `last_frame` from `st.fb` on send, clears `st.dirty`. Any fb pixel that differs from the last shipped frame is shipped regardless of dirty bookkeeping -> vacated areas re-sync -> no ghosting. Also coalesces first+upgrade+upgrade landing within one tick into one ship.
- `force_repaint` simplified: `last_frame.clear()` alone now forces a full frame (len mismatch path), dropped the GFX-specific dirty push.
- gfx.rs catch-all: one-shot `tracing::warn!` if any unhandled ServerPdu shows up (diagnostic only).

`GfxState.dirty` is now vestigial (still written by `propagate`, no longer read) — harmless; can remove later. Frontend untouched.

Next: user re-extracts + rebuilds, opens/drags a window. Expect the ghost gone. If it persists, fb itself holds the duplicate (server move-via-S2S without a vacated repaint) — would then need explicit vacated-area handling; the new warn also tells us if any unhandled op fires during the drag.

## Latest — GFX Progressive verified live (full quality)

4c works. Live log: `first=455` then `upgrade=455` + `upgrade=455` (the two heavy passes now decode), and the screen renders at full quality — Windows wallpaper, Edge browser + Microsoft new-tab page, desktop icons, taskbar, all sharp with correct colours, no shear, no SRL desync, no black blocks. (The "built 4b / WBT_TILE_UPGRADE unused" last round was a stale extraction — the shipped archive already had 4c.)

**GFX rendering stack is now functionally complete:**
- ClearCodec (residual / bands-vBar + caches / subcodec RLEX/RAW / glyph cache)
- Bitmap cache (SurfaceToCache + CacheToSurface) and SurfaceToSurface copy
- RemoteFX Progressive (reduce-extrapolate inverse DWT; TILE_FIRST RLGR1+dequant; TILE_UPGRADE SRL/RAW bit-plane refinement with persistent per-tile current+sign+bitPos)
- Half-open RDPGFX rects; native-res surface + framebuffer transport reusing the legacy region encoder.

Minor leftovers (not blocking usability):
- A few small dark squares occasionally (likely transient mid-refinement / surface-recreate before repaint) — investigate only if they persist.
- Diagnostic logging is bounded (first N) but could be quieted for release.
- Surface re-create (`CreateSurface` same id/dims) currently re-allocs; could preserve pixels to avoid a flash.

Remaining GFX backlog (optional): AVC420/444 → WebCodecs (only if a server negotiates H.264; this server uses Progressive), dynamic resize re-enable, non-extrapolate DWT fallback. Otherwise GFX is done — next priorities can return to product features.

## Latest — RemoteFX Progressive: TILE_UPGRADE (Slice 4c)

4b rendered FIRST tiles correctly (wallpaper, Edge window, text all sharp), but the server streams progressively: a coarse FIRST then 2 large UPGRADE passes (~61KB+54KB vs 78KB FIRST) that carry most of the image detail. We were dropping UPGRADE → coarse-only + black/stale blocks where later updates were upgrade-only.

Implemented the upgrade path in `progressive.rs`:
- Per-tile state extended to **sign** (= raw RLGR output captured during FIRST, before dequant) and **bitPos** (= quant+progQuant per band), kept channel-wide alongside `current`.
- `BitReader` (MSB-first, mirrors FreeRDP wBitStream) + `SrlState` (the kp/nz/mode simplified-run-length bit-plane decoder) + RAW bit reader.
- `upgrade_block`: for non-LL bands, per coefficient — known sign(>0/<0) reads `numBits` RAW bits; sign==0 reads SRL (and records the new sign); LL band reads RAW unconditionally. Adds `input << shift` to the stored coefficient.
- `upgrade_component`: numBits = oldBitPos − newBitPos (per band), shift = newBitPos − 1; refines all 10 subbands, updates bitPos, then re-runs the inverse extrapolate DWT (reverse path: buffer = current → IDWT) and emits the tile.
- Tiles seen only via UPGRADE without a prior FIRST are skipped (can't refine nothing).

gfx unchanged — upgrade tiles flow through the same decode→blit→propagate path. The dead-code warning (WBT_TILE_UPGRADE) is resolved.

Expect: Progressive regions now reach full quality (photos/thumbnails sharpen across the 2-3 passes), and the black/stale blocks fill in. This is the last big rendering piece — if it works, GFX is functionally complete (ClearCodec + cache + SurfaceToSurface + Progressive first/upgrade). ~865-line module, still untested DSP; watch for: SRL desync (garbage that grows over a region = bit-reader misalignment), or residual coarse areas (numBits/bitPos math).

Remaining GFX backlog: AVC420/444 → WebCodecs (only if a server negotiates H.264; this one uses Progressive), dynamic resize re-enable, non-extrapolate fallback.

## Latest — RemoteFX Progressive: TILE_FIRST decoder (Slice 4b)

Implemented the FIRST/SIMPLE tile decode in `crates/rh-rdp/src/progressive.rs` (the parser from 4a extended to a full decoder). Pipeline per tile (extrapolate path, which is what the server uses — flags=0x01):
1. **RLGR1** entropy decode (`ironrdp::graphics::rlgr::decode`) → 4096 i16 coeffs.
2. **Dequant**: left-shift each subband by `shift = quant + progQuant - 1` (per band). Subband offsets/lengths for reduce-extrapolate: HL1@0(1023) LH1@1023(1023) HH1@2046(961) HL2@3007(272) LH2@3279(272) HH2@3551(256) HL3@3807(72) LH3@3879(72) HH3@3951(64) LL3@4015(81). LL3 gets a differential (prefix-sum) decode first.
3. **Inverse reduce-extrapolate DWT**: `dwt_block` levels 3→2→1, each = idwt_x(LL+HL→L), idwt_x(LH+HH→H), idwt_y(L+H→out). Ported faithfully (lifting scheme, clamp to i16, integer /2). band counts: L=(64>>lvl)+1, H= lvl==1?31:(64+(1<<(lvl-1)))>>lvl.
4. **YCbCr→RGB** (`ironrdp::graphics::color_conversion::ycbcr_to_rgba`, RFX transform) → 64x64 RGBA.
5. Blit to surface at (xIdx*64, yIdx*64) (clamped at right/bottom edge tiles) + propagate.

Per-tile coefficient buffers kept in a HashMap keyed by grid index (for RFX_TILE_DIFFERENCE accumulation now, and TILE_UPGRADE later).

**TILE_UPGRADE is parsed but NOT applied this slice** — upgraded tiles keep their FIRST pixels. So expect: areas covered by a detailed FIRST render sharp; areas that relied on coarse-FIRST + upgrade passes look soft/blocky. That's expected; 4c (SRL upgrade + sign/current) sharpens them.

This is ~620 lines of untested integer-wavelet DSP. Likely needs 1-2 fixes. Watch for: wrong colors (YCbCr scaling / R-B swap), blocky-but-placed (DWT edge bug), or per-tile garbage (subband offset/length). Log: "GFX Progressive: NB -> M tiles decoded region(...)". If a tile decode fails it's silently skipped (stays blank).

Next: 4c = TILE_UPGRADE (progressive_rfx_srl_read bit-plane + upgrade_block) using the retained sign/current buffers; then dynamic-resize re-enable.

## Latest — RemoteFX Progressive: bitstream parser (Slice 4a)

The cache slice made the desktop render well; the leftover artifacts (blue blocks where web images/photos go, ghost-trails when dragging a window) are ALL `WireToSurface2` codec2 = `RemoteFxProgressive` (log: 78400/61190/54472… byte blocks, "NOT handled"). So Progressive is the final rendering codec.

Confirmed (reading FreeRDP progressive.c) it is a large self-contained codec — NOT reusable from IronRDP's classic-RFX primitives:
- reduce-extrapolate inverse DWT (`RFX_DWT_REDUCE_EXTRAPOLATE`), subband diffing, progressive quantization, per-component RLGR, SRL bit-plane UPGRADE passes, and persistent per-tile sign/current buffers across frames.
IronRDP ships only the pieces (`dwt`,`rlgr`,`quantization`,`subband_reconstruction`,`color_conversion` in ironrdp-graphics 0.7) — the assembly + the progressive-specific transform must be ported.

Building in slices. **Slice 4a = `crates/rh-rdp/src/progressive.rs` bitstream parser** (no DSP): walks blocks SYNC(0xCCC0)/FRAME_BEGIN/FRAME_END/CONTEXT/REGION(0xCCC4)/TILE_SIMPLE(0xCCC5)/FIRST/UPGRADE; parses the region header (tileSize/numRects/numQuant/numProgQuant/flags/numTiles/tileDataSize), skips rect+quant+progQuant tables, walks tile headers (quantIdx Y/Cb/Cr, xIdx/yIdx, flags, quality, y/cb/cr/tail lengths; upgrade = srl/raw lengths). Logs a per-frame structure summary ("region tiles[simple/first/upgrade] numQuant… extrapolate… ; tile0[…]"). Wired into the WireToSurface2 arm.

Goal of 4a: verify we read Progressive correctly and learn from the live server how many tiles, simple vs first vs upgrade, numQuant/numProgQuant, and whether extrapolate is set — to ground the DSP (4b). No visible change expected this build; need the log.

Next: 4b = TILE_SIMPLE/FIRST DSP (RLGR via ironrdp-graphics + ported progressive dequant + reduce-extrapolate inverse DWT + YCbCr→RGB → write 64x64 tiles), then 4c = TILE_UPGRADE (SRL) + sign/current persistence + RFX_TILE_DIFFERENCE.

## Latest — GFX bitmap cache + surface-to-surface copy

Diagnostic log answered it: the black background was NOT a missing codec. The server fills large/repeating areas via the **bitmap cache** — `SurfaceToCache` (copy a surface rect into a cache slot) then a flood of `CacheToSurface slot=N -> surf pts=1` (stamp the cached tile at many destination points). Scrolling uses `SurfaceToSurface` (copy a surface rect to new points) — that was the blue vertical streaks on the 2nd screenshot.

Implemented in `GfxState` (no codec math — pure pixel copies):
- `cache: HashMap<u16, CachedTile{w,h,px RGBA}>`.
- `surface_to_cache(id, slot, rect)` — extract the rect from the surface into the slot.
- `cache_to_surface(slot, id, dx, dy)` — stamp the cached tile onto the surface at each destination point, returns the written rect for propagation.
- `surface_to_surface(src,dst,rect,dx,dy)` — copy via a temp buffer so overlapping same-surface scroll copies don't corrupt.
Handlers wired in `process()`; each propagates the written rect to the framebuffer → shipped via the existing transport. All GFX rects use the half-open `rect_xywh`.

State after Slice 3 + shear fix: ClearCodec renders crisp (menus, taskbar, Edge window, text all correct). With the cache ops the desktop background + repeated UI should now fill in and scrolling should stop streaking.

Still unhandled: `WireToSurface2` (codec2 = RemoteFX Progressive) — not seen carrying much yet; logging left in. If large regions still go black/stale, Progressive is the next decoder. Otherwise GFX is largely functional for everyday use.

## Latest — shear confirmed fixed; hunting the wallpaper carrier

The half-open rect fix worked: context menus + taskbar now render crisp and square (log confirms 448x128 / 36x64). ClearCodec path is solid.

Remaining: most of the screen is still black. No `skip codec` lines fire → the big/background areas are NOT WireToSurface1 (any codec1). They must arrive via the previously-silent catch-all. Added one-shot logging (first 40) for WireToSurface2 (codec2 = RemoteFX Progressive), SurfaceToSurface, SurfaceToCache, CacheToSurface — to learn what carries the desktop bulk before building the next decoder. Next slice targets whatever shows up (expected: WireToSurface2 / Progressive, or Cache tiles).

## Latest — GFX shear fix: half-open RDPGFX rects

Slice 3 rendered ClearCodec but everything was sheared diagonally. Added a one-shot log of the raw `destination_rectangle` (l/t/r/b) + surface/fb dims. Decisive data:
- `l=1792 r=1828` on a 1828-wide screen → half-open width 36 fits exactly (inclusive +1 → 37 → 1829 > 1828, overflow).
- `l=704 r=1152 t=512 b=640` → 448x128 (powers of 64), my inclusive math gave 449x129.

Root cause: `RDPGFX_RECT16` is **half-open** (right/bottom = one past last pixel), but IronRDP models it as `InclusiveRectangle`, and `rect_xywh` added +1. Every GFX rect was 1px too wide & tall → the raster-order layers (residual, glyph store/blit) drifted 1px/row → diagonal shear; also 1px overflow past the surface edge.

Fix: `rect_xywh` now `width = right - left`, `height = bottom - top` (no +1). Affects WireToSurface1 and SolidFill (both GFX rects). ClearCodec-internal band/vBar coordinates are unchanged (those are inclusive within the codec stream, per spec — only the GFX wrapper rect was wrong). Surface=fb=1828x1080, origin (0,0) — confirmed consistent, so this single fix should make the desktop render correctly. Diagnostic logging left in (low volume) for the next codec slices.

## Latest — GFX Slice 3: ClearCodec decoder

Probe (Slice 2a) showed the server's ClearCodec uses mainly the **bands/vBar** layer (big frame: bands=8480, residual=414, subcodec=0; glyphs via bands + GLYPH_INDEX caching; tiny 35B updates = subcodec RLEX). So the decoder had to cover vBar bands fully — the hardest layer. No shortcut.

Implemented `crates/rh-rdp/src/clearcodec.rs` (`ClearDecoder`), ported from FreeRDP `libfreerdp/codec/clear.c` (Apache-2.0 — algorithm reimplemented in Rust, pixels handled directly as RGBA):
- Header: glyphFlags + seqNumber; CACHE_RESET resets vBar cursors.
- **Glyph cache** (4000 entries): GLYPH_HIT → blit cached bitmap to dst; GLYPH_INDEX (no hit) → decode then store the decoded rect into the cache.
- Composite layers in order residual → bands → subcodec:
  - **residual**: RLE of BGR + run (u8→u16→u32 escapes), fills the rect raster.
  - **bands/vBar**: per band (xStart/xEnd/yStart/yEnd + bkg color), per column a vBar via SHORT_VBAR_CACHE_HIT / SHORT_VBAR_CACHE_MISS / VBAR_CACHE_HIT; two caches (ShortVBarStorage 16384, VBarStorage 32768, wrapping cursors); full vBar = bkg above + short pixels + bkg below; composed column-by-column into dst.
  - **subcodec**: records (xStart,yStart,w,h,byteCount,id); id 0 = RAW BGR24, id 2 = RLEX (palette + run/suite encoding), id 1 = NSCodec (skipped).
- Single decoder per GFX session (caches + seq are channel-wide, persist across ResetGraphics).

`gfx.rs`: `GraphicsPipeline` gained a `ClearDecoder`; the ClearCodec arm decodes into the target surface then `propagate`s the rect to the framebuffer → shipped via the existing region transport. Borrow-safe (self.clear vs self.state are disjoint fields; surface dims read into locals before the &mut decode).

Expect on test (`RDP_GFX=1`): **the desktop should actually render now** (ClearCodec is most of the picture). Watch for decode warnings ("GFX: ClearCodec decode failed: ...") and any visual artifacts (wrong colors = BGR/RGB swap somewhere; garbage columns = vBar bug). Next: Planar + RemoteFX-Progressive (Slice 4), then AVC→WebCodecs (Slice 5). Compile risk: ~545 lines of untested decoder — iterate on report.

## Latest — GFX Slice 2a: surface model, framebuffer, transport feed

Connector patch (1b) worked: server confirms GFX (V10_4) and streams — but the default codec is **ClearCodec** (not AVC; AVC needs a per-server GPO). User chose the **full GFX** path (universal, no per-server config; H.264 deferred as a later bonus layer). Finding: IronRDP provides ZGFX + GFX PDU parsing + a Planar decoder, but **no ClearCodec and no assembled RemoteFX-Progressive decoder** — those we write.

Architecture: GFX codecs decode into an RGBA framebuffer (`GfxState`, shared `Arc<Mutex>`), and the worker loop ships that framebuffer's dirty rects through the **existing** region-encode/`FrameBatch` transport — so the frontend is unchanged for all non-AVC codecs. (AVC→WebCodecs will be the only frontend change, last.)

Slice 2a (this turn, Rust-only):
- `gfx.rs`: `GfxState { w,h, fb: RGBA, dirty, surfaces, origin }` + `GraphicsPipeline` (DvcProcessor). Handles ResetGraphics (alloc fb), CreateSurface/DeleteSurface, MapSurfaceToOutput (blit surface→fb), SolidFill, WireToSurface1 **Uncompressed** (BGRA→RGBA into surface→fb); ClearCodec/Planar/RemoteFx/Avc* counted + skipped with periodic log; StartFrame/EndFrame → FrameAck. `propagate()` mirrors surface rects to the composited fb + records screen-space dirty rects.
- `actor.rs`: `connect()` gains `gfx_state: Option<Arc<Mutex<GfxState>>>`; `blocking_session` builds it from `RDP_GFX` and passes a clone. Emit tick: when GFX active, ship `GfxState.dirty` rects (extracted from `fb` via existing `make_region`) through `tx_enc` instead of `compute_regions(&image)`. force_repaint marks whole screen dirty for GFX. Without `RDP_GFX` → None → legacy path byte-identical.

Expected on test (`RDP_GFX=1`): logs "advertising caps" → "CONFIRMED V10_4" → CreateSurface → "skip codec ClearCodec ... (not yet implemented)" + frame acks. **Screen mostly BLACK** (ClearCodec skipped) — possibly some SolidFill/Uncompressed regions paint. This proves surfaces+framebuffer+transport end-to-end. Real picture arrives with ClearCodec = Slice 3.

Compile risk (user compiles): borrow-checker edge cases in `propagate`/`process` (disjoint field borrows), IronRDP field-name assumptions (verified against pdu 0.7 source). Next: ClearCodec decoder.

## Latest — GFX Slice 1b: patch connector to actually request GFX

Live test of Slice 1 (`RDP_GFX=1`): connection succeeded, server advertised `DYNVC_GFX_PROTOCOL_SUPPORTED`, but the **GFX DVC never opened** — the picture rendered fine via the legacy fast-path (visible `rdp frame stats fps=4..` from our encoder) and **none** of the GFX diag logs appeared (no "advertising capability sets"). Root cause found in IronRDP sources: the connector (`ironrdp-connector/src/connection.rs`) hardcodes `ClientEarlyCapabilityFlags` to VALID_CONNECTION_TYPE|SUPPORT_ERR_INFO_PDU|STRONG_ASYMMETRIC_KEYS|SUPPORT_SKIP_CHANNELJOIN[|WANT_32_BPP] and never sets `SUPPORT_DYN_VC_GFX_PROTOCOL` (0x0100). No `Config` option exposes it; `ClientConnector` only has `new`+`with_static_channel`; connector 0.9 is the same (and would force a pdu0.8/core0.2 stack bump). So upstream IronRDP simply never requests the graphics pipeline → the server doesn't open the channel.

Fix: **vendored `ironrdp-connector` 0.8.0** into `vendor/ironrdp-connector/` (exact published source, removed nested Cargo.lock/.orig), added the one flag `| ClientEarlyCapabilityFlags::SUPPORT_DYN_VC_GFX_PROTOCOL` to the early-cap construction, and added to the workspace `Cargo.toml`:
```
[patch.crates-io]
ironrdp-connector = { path = "vendor/ironrdp-connector" }
```
Vendored Cargo.toml is self-contained (concrete `[lints]`, registry deps pin core0.1/pdu0.7/svc0.6 = our stack → versions unify, no duplicates). The patch replaces the connector everywhere in the graph, including the `ironrdp` meta crate's transitive use.

Re-test (still `RDP_GFX=1`): now expect "advertising 6 capability sets" → "server CONFIRMED caps {...}" → growing AVC wire-pdu counts. Screen still blank (Slice 1 only logs). If AVC flows, Slice 2 (extract H.264 → WebCodecs draw).

Maintenance note: re-apply the connector patch on any IronRDP version bump.
Compile/resolve risk (user compiles): the `[patch]` + vendored path must resolve cleanly (version unification, picky `=7.0.0-rc.20` pin). If cargo complains, report.

## Latest — GFX/H.264 Slice 1: negotiation diagnostic

Direction decided: GFX picture decoded in the **WebView via WebCodecs** (GPU H.264), not openh264 in Rust — best quality/smoothness + no C-dependency. Frontend spike (shipped `webcodecs_h264_spike.html`) came back fully green on the user's machine: hardware H.264 decode for all profiles, ~825 fps round-trip. Decode risk retired.

Backend findings (from IronRDP 0.14 / pdu 0.7 / graphics 0.7 sources): no `ironrdp-egfx` crate, but the building blocks exist — `ironrdp::pdu::rdp::vc::dvc::gfx` has the full GFX message set incl. `Avc420BitmapStream`/`Avc444BitmapStream`, client `CapabilitiesAdvertisePdu`/`FrameAcknowledgePdu`, server `ServerPdu` enum; `ironrdp::graphics::zgfx::Decompressor` does RDP8 bulk decompress. `ironrdp-session` 0.8 does NOT route GFX (only fast-path bitmap/RemoteFX) — so surface model + AVC forwarding are ours to write.

**Slice 1 shipped (Rust, env-gated):** `rh-rdp/src/gfx.rs` — `GraphicsPipelineDiag` implements `DvcProcessor`+`DvcClientProcessor` for `Microsoft::Windows::RDS::Graphics`. `start()` advertises caps (V8, V8.1 AVC420_ENABLED, V10/V10.2/V10.3/V10.4 with AVC_DISABLED clear) so the server switches to the pipeline. `process()` ZGFX-decompresses, loops `ServerPdu::decode`, logs CapabilitiesConfirm (chosen version) + WireToSurface1 (codec/bytes, counts AVC) + counts other PDUs, and acks every EndFrame so the server keeps streaming. `GfxClientMsg` newtype wraps the foreign `ClientPdu` to satisfy `DvcEncode` (orphan rule). Wired into `actor::connect` behind `std::env::var("RDP_GFX")` — when set, also hosts the GFX DVC; **screen blank by design** (we only listen+log, no drawing yet). Unset → legacy path untouched.

Test: run with `RDP_GFX=1` set, connect RDP. Expect: console logs "advertising N capability sets" → "server CONFIRMED caps version=..." → periodic "N frames | M wire-pdus (K AVC)". Screen blank is expected. If AVC count climbs, GFX+H.264 is confirmed live → Slice 2 (extract H.264 → WebCodecs draw).

Compile risk (user compiles): IronRDP meta-crate import paths (`ironrdp::core::impl_as_any`, `ironrdp::graphics::zgfx`, `ironrdp::pdu::rdp::vc::dvc::gfx::*`, `ironrdp::dvc::DvcEncode`), the `impl_as_any!` macro through the renamed `core` module, and the caps flag variant names — flagged; quick fixes if any differ.

## Latest — RDP: keepalive so a dropped session doesn't just freeze

Reported: connecting to the same host via mstsc kicked the RemoteHub RDP session, but RemoteHub showed no disconnect — the picture just froze (mstsc shows the standard "another user connected" 0x3/0x5 error). A clean MCS Disconnect Ultimatum is handled (`ActiveStageOutput::Terminate` → `Closed`), so the freeze means none arrived — the server stopped/half-closed silently and our 16 ms-poll reads just kept returning `TimedOut` (→ continue) forever.

Fix (`rh-rdp/actor.rs`, `connect()`): enable **TCP keepalive** on the socket right after connect (`socket2::TcpKeepalive`, time 15 s / interval 5 s; new direct dep `socket2 = "0.6"`, already in the tree). The OS now probes the link; a dead/half-open connection surfaces as a read error (`ConnectionReset`/`ConnectionAborted`, **not** `TimedOut`) within ~1 min, which hits the existing `Err(_)` arm → `Closed { ServerDisconnected }` → the disconnect screen. No false positives: a live-but-idle desktop stays up because keepalive probes succeed. Detection isn't instant (~40-65 s) but beats an indefinite freeze.

Rust-only; user compiles. (The "mstsc feels smoother" observation is the known server-repaint-rate ceiling → GFX/H.264 backlog item, not addressed here.)

## Latest — connecting overlay cancel + failed-state redesign

- **Cancel button:** the connecting overlay’s button is renamed Close → **Cancel** (`common.cancel`) and `.connecting` got `z-index: 20` so it sits above the terminal layer (the likely reason clicks weren’t registering). Calls `close(session.key)` — removes the tab immediately, tears the actor down in the background.
- **Failed state:** replaced the long raw OS error with a short **headline** + red dot. Category from **locale-independent** markers (Winsock 10060/10061/11001/10051 + English io::ErrorKind phrases — never the localized OS prose): timeout→“Host not responding”, refused, dns→“Host not found”, network, generic. Collapsible **Details** (`<details>`) shows a synthesized **step log** (👤 start → ⚙️ resolve → ⚙️ connect → 😨 failure, i18n with host/port) + the raw OS error in mono. Log built on the frontend from the category (shows *where* it failed) — no backend trace events. Auth failures keep the re-auth UI; graceful “closed” keeps the plain EmptyState.
- **i18n:** added `common.details`, `session.fail.*`, `session.log.*` to en + ru.

All frontend; no Rust change. Cancel-during-connect still lets the backend handshake run its ~20s timeout (UI tab is gone); a real cancellation token is a later nicety.

## Latest — host inspector fills full height

The right-pane inspector sat at content height, leaving dead space below the «Удалить» footer (user report). Root cause: `.dockInner` used `max-height: 100%` (shrink-to-content) instead of `height: 100%`. The height chain is already definite (`.home flex-col` → `.listrow flex:1` → `.dock align-self:stretch`) and the form is already a flex column (`header`/`connectRow` = `flex:0 0 auto`, `body` = `flex:1; min-height:0; overflow-y:auto`, `footer` = `flex:0 0 auto`). Switching `.dockInner` to `height:100%` lets the body expand+scroll and pins the footer to the bottom — no magic `calc(100vh - offset)` needed since the parents already provide the height. CSS-only; `tsc`/`vite build` green.

## Latest — Key selector → filtering combobox (✕ in-field)

Per request, reworked the host "Key" field:
- It's now a real combobox: clicking the field opens the dropdown and turns it into a text input that **filters the saved keys by name** (case-insensitive substring).
- **"Add new key" and "Use SSH agent" are pinned** at the bottom of the dropdown (own section, top border, never filtered), above a scrollable key list (`savedPickerScroll` max-height 200px). Empty filter result shows "No matching keys".
- Trailing in-field icon: **✕ (clear)** when a key/agent is linked — replaces the chevron the user disliked — clearing unlinks it; **chevron** when nothing is linked (opens the list). The separate outside ✕ button is gone.
- Outside-click close now lives on the wrapper (`comboRef` covers field + dropdown), so typing/clicking inside doesn't self-close; Esc also closes.

Implementation: `CredentialPanel` key row rebuilt (`keyCombo`/`keyComboInput`/`keyComboBtn` + `keyFilter` state + outside-click effect); `SavedCredentialPicker` now takes a `filter` prop and renders `savedPickerScroll` + pinned `savedPickerPinned`. i18n: `keyClear` repurposed as the clear tooltip ("Очистить ключ"), new `keyNoMatch`. Frontend-only; `tsc`+`vite build` green.

## Latest — Fix + UX: single-slot SSH key selector

Reported: changing the host's key from `ed-2.ppk` to `ED.ppk` did nothing; then "No key" removed one and left the other — i.e. multiple key creds were stacking on the host.

Cause: `linkCredential` / `onUseAgent` called `linkHost` without unlinking the previously-linked key. The host ended up with several `ssh_key`/`ssh_key_agent` links; `linkedKeyCred = linkedCreds.find(kind==="ssh_key")` returned the *first*, so the UI never reflected the new pick.

Fixes (frontend only, `ui/src/components/host/HostDetail.tsx` + `.module.css`):
- New `dropLinkedKeyAuth(exceptId)` helper unlinks every currently-linked `ssh_key`/`ssh_key_agent` (except the one being set). Called before linking in both `linkCredential` and `onUseAgent`; both now `set_as_default: true`. → exactly one key/agent linked at a time; switching replaces.
- UX: removed the "No key" row from `SavedCredentialPicker`; added a small ✕ `keyClearBtn` to the right of the key field (`authTriggerWrap` is now a flex row, `keySelect` flexes, ✕ hovers to danger). Clears the linked key/agent.

No Rust change — `tsc`+`vite build` green.

## Latest — Fix: spurious "host not found" when deleting a host

Symptom: delete a host → long spinner → red "host not found" inside the confirm dialog (though the host *was* deleted). Three causes, three fixes:

1. **`ConfirmDialog` re-entrancy** (`ui/src/components/dialog/ConfirmDialog.tsx`). `disabled={submitting}` only takes effect after a re-render, so a fast double-click fired `onConfirm` (→ `host_delete`) twice. Added a synchronous `useRef(false)` guard checked at the top of `run()`. The 2nd call was racing the 1st on the SQLite write lock — that wait was the "long spin"; the 2nd then found 0 rows.
2. **`host_delete` masked the real error** (`rh-app/api/hosts.rs`). `.map_err(|_| ApiError::not_found("host"))` relabelled *any* failure as "host not found". Now propagates the real `StorageError` via `?` (there's a `From<StorageError> for ApiError`).
3. **Non-idempotent store delete** (`rh-storage/host_store.rs`). `delete` returned `Backend("host … not found")` when `rows_affected == 0`. Now idempotent: a host already gone satisfies the goal → `Ok(())` (mirrors the keychain delete philosophy). Linked rows clear via FK cascade.

Net: a double-fire can't happen; if it somehow does, the second call is a no-op success; and any *genuine* delete failure now shows its real cause instead of a misleading "not found". (Rust compiles on the user's machine; frontend `tsc`+`vite build` green.)

## Latest — SSH: jump-connect timeout + agent-forwarding serving side

Two SSH gaps closed (Rust; compiles on the user's machine — verified the russh 0.45 API against the downloaded crate source, not built here).

**(A) Bastion/jump connect timeout.** `drive_target_connect`'s `CONNECT_TIMEOUT` (20 s) only bounded the *target* connect; the bastion `russh::client::connect(jump…)` itself was unbounded → a dead jump host hung on the OS TCP timeout (minutes on Windows). Now wrapped in `tokio::time::timeout(CONNECT_TIMEOUT, …)` → clean `SshError::Network(TimedOut, "jump host connection timed out")`, same as the direct path.

**(B) Agent-forwarding serving side** (`rh-ssh/actor.rs`). Previously we only *advertised* acceptance (`channel.agent_forward(false)`); the back-channels were ignored. Now bridged end-to-end:
- `ClientHandler` gained `agent_forward: bool` (true only for the target when `params.agent_forwarding`; false for the bastion) + `agent_bridges: HashMap<ChannelId, AgentBridge>`.
- New Handler callbacks (russh auto-`confirm()`s the channel before calling): `server_channel_open_agent_forward` opens the local OS agent and registers a bridge; `data` feeds server bytes into the bridge and writes framed agent replies back via `session.data`; `channel_eof`/`channel_close` drop the bridge. The PTY channel is untouched (its data is consumed via its own `Channel<Msg>`; the map-membership check ignores it even though russh fans data out to both).
- `AgentBridge` is a transparent length-prefixed byte relay (no agent-protocol parsing): each complete framed request → written verbatim to the agent → its framed reply → back on the channel. `AGENT_MSG_CAP` 256 KiB sanity bound. Transport: unix `$SSH_AUTH_SOCK` `UnixStream`; Windows `\.\pipe\openssh-ssh-agent` named pipe (same pipe the client-auth path uses).

**Verified from russh 0.45 source:** `server_channel_open_agent_forward(&mut self, ChannelId, &mut Session)`, `data(&mut self, ChannelId, &[u8], &mut Session)`, `Session::data/close`, `russh::CryptoVec::from_slice`, `russh::ChannelId`, `#[async_trait]`, and that `confirm()` runs before the callback. Compile risk is low; the one live-unproven bit is the OS-agent transport behaviour.

**Test (SSH server `89.23.99.57` is separate from the down RDP host):** enable agent forwarding on the host, ensure the local agent holds a key (`ssh-add -l` locally), connect, then on the remote run `ssh-add -l` — it should list the *local* key. Also confirm a dead/wrong jump host now fails at ~20 s with a timeout screen, not a multi-minute hang.

## Latest — RDP "open in a separate window" (pop-out)

Detach a live RDP session into its own OS window; the tab shows a placeholder. Single-sink handoff model: the backend forwards frames to exactly one webview, swapped on demand, with a forced full repaint so the fresh canvas paints completely (RDP streams deltas).

**Backend (compiles on the user's machine — not built here):**
- `rh-rdp/lib.rs`: new `RdpCommand::Repaint`.
- `rh-rdp/actor.rs`: `run()` owns `force_repaint: Arc<AtomicBool>` (cloned to the worker); `Repaint` sets it; the worker, before the diff tick, does `if force_repaint.swap(false) { last_frame.clear(); }` → `compute_regions` sees a length mismatch → emits the **full** image. `blocking_session` signature gained `force_repaint: &AtomicBool`.
- `rh-app/rdp_session.rs`: `RdpHandle.sink: Arc<Mutex<Channel<RdpSessionEvent>>>` (swappable); the forwarding task reads the sink per-event; new `reattach(id, on_event)` swaps the sink + sends `Repaint`. Break-on-send-error still ends the session when a webview dies *without* a reattach (so closing the popout window ends the session).
- `rh-app/api/rdp_sessions.rs`: `#[tauri::command] rdp_session_reattach(state, req: SessionIdRequest, on_event: Channel) -> ApiResult<bool>`; registered in `main.rs`.
- `capabilities/default.json`: `windows: ["main","rdp-*"]` + `core:webview:allow-create-webview-window`, `core:window:allow-set-title`.

**Frontend (tsc + vite green):**
- `ipc.ts`: `rdpSession.reattach(sessionId, onEvent)`.
- `store`: `poppedOut: Record<key,bool>`; `detachRdpToWindow(key)` (marks placeholder + `new WebviewWindow('rdp-<sid>', { url:'index.html#popout?…', decorations:true })`); `redockRdp(key)` (reattach to main **first**, clear placeholder, then close the popout window); `attachExternalRdp({sessionId,title,w,h})` (popout side: create a local tab + reattach + repaint). Module-level `redockGuard` + a one-time `rdp:popout-closed` listener: a user-closed popout ends the owning tab; a re-dock close is suppressed (guard, with a 1.5s backstop).
- `App.tsx`: `#popout` hash → renders `RdpPopoutApp` (new) instead of `AppShell`. `RdpPopoutApp` parses `sid/t/w/h`, attaches, renders a full-window `RdpViewport` (native window controls kept; our bar = title + fullscreen), and emits `rdp:popout-closed` on `onCloseRequested`.
- `RdpViewport`: new `onPopOut?` → pop-out button in the bar (PictureInPicture2), shown only when provided (so the popout window has none).
- `SessionView`: when `poppedOut[key]` → placeholder ("Сессия RDP открыта в отдельном окне" + "Вернуть во вкладку" → `redockRdp`) instead of the viewport; otherwise passes `onPopOut`.
- i18n: `session.popOut` / `session.poppedOutTitle` / `session.redock` (ru+en).

**Lifetime:** pop-out → tab placeholder + window owns the stream. Window X = session ends + tab closes. "Вернуть во вкладку" = reattach to tab, session continues, window closes.

**⚠️ Unproven / likely tweaks:** Rust not compiled here; multi-window untestable in sandbox. Watch (1) the `WebviewWindow` `url` format across dev/prod, (2) whether Tauri wants an extra window-create permission, (3) the full-frame repaint on reattach (same DVC/RemoteFX caveat noted for fullscreen resize, though a full frame is the normal first-frame path).

## Latest — RDP: fullscreen fill, auto-show bar, trimmed controls

Follow-ups to the pane-aspect change (which fixed windowed but left side bars in fullscreen, since the monitor is wider than the pane):

- **Fullscreen fills (no bars).** On the `fullscreenchange` transition we now fire a one-shot DisplayControl resize (`onResizeRef`): monitor size (`window.screen.*`) entering, the pane size (`wrap.clientW/H`) leaving, deferred a frame so the box has settled. The server re-negotiates → `Resized` → canvas backing matches the surface aspect → `object-fit: contain` fills edge-to-edge. ⚠️ **Unproven / caveat:** this is the same DVC path that's gated for continuous resize (`ENABLE_DYNAMIC_RESIZE=false`) due to a RemoteFX post-resize repaint issue; a one-shot toggle should be fine (interaction repaints), but needs a live test. Fallback if it misbehaves: connect at monitor aspect instead (fullscreen native, windowed letterboxed).
- **Auto-show bar.** New `connected` prop (`session.state === "ready"`); when it flips true the connection bar reveals for ~3.2s then auto-hides, so the fullscreen control is discoverable right after connect.
- **Trimmed bar.** Removed the minimize and close buttons (and the `onClose` prop / `Minus`,`X` imports / `minimize` cb); the bar is now **title + fullscreen** only. Close is via the session tab; in fullscreen exit first (Esc / Ctrl+Alt+Enter).

**Not done — "open in separate window" (pop-out):** assessed as a real slice, deferred. Needs (1) backend `rdp_session_reattach` — swap the frame sink to a new Channel **and force a full-frame repaint** (RDP has no ring/reattach today, unlike SSH/local), (2) a Tauri `WebviewWindow` (+ window-create capability), (3) a frontend popout route rendering just that session + the main tab showing a "session opened in another window" placeholder + cross-window open/close coordination (single-sink handoff main↔popout). Not sandbox-verifiable (multi-window). Proposed as the next focused slice.

## Latest — RDP proportions fix (pane-aspect resolution + contain)

The RDP desktop was rendered at the **monitor** resolution (usually 16:9) and shown with `object-fit: fill`; in a non-16:9 session pane that meant either stretch distortion or a desktop sitting in the corner with black margins (user report). Fixes (frontend-only — backend already honours the requested width/height):

- `store/index.ts createSession`: request a resolution matching the **pane aspect** (`window.innerWidth × (innerHeight − ~44px tab strip)`) at near-monitor vertical resolution, capped 2560×1600, even dims. So the server desktop comes back at the viewport's shape and fills it.
- `RdpViewport.module.css`: `object-fit: fill → contain` — never distort; if the live pane aspect drifts from the connect-time request (e.g. a window resize), it letterboxes cleanly instead of stretching.
- `RdpViewport.tsx toCanvas`: map client→backing through the contain transform (uniform `scale` + centering `offX/offY`), so clicks stay aligned; points in the bars clamp to the nearest edge.

Trade-off (unchanged limitation): resolution is still fixed at connect — resizing the window afterwards re-letterboxes rather than reflowing; true live reflow is the gated DisplayControl feature. Fullscreen of a pane-aspect desktop letterboxes within the monitor (correct, sharp). `tsc`/`vite build` green.

## Latest — conn-states: timeout, reliable auth taxonomy, inline re-auth restored

Three fixes from the user's report + designer mockups:

1. **SSH connect hangs forever → fixed (BACKEND, recompile).** `crates/rh-ssh/src/actor.rs`: added `CONNECT_TIMEOUT = 20s` and a deadline arm in `drive_target_connect` (the `tokio::select!`). A dead host (no SYN-ACK) previously hung on the OS TCP timeout (minutes); now it fails at 20s with `io::ErrorKind::TimedOut → SshError::Network("connection timed out")` → `CloseReason::NetworkError` → the `timeout` screen. (Bastion/jump connect not wrapped — direct path only; note for later.)

2. **Wrong password/key showed the generic screen → fixed (FRONTEND).** Root cause: the nice `auth_failed` message (`Auth failed (password)`) was getting clobbered by a later `error`/`closed` event whose text (`authentication failed …` / `Authentication failed`) my `connCategory` didn't match. Fix: the store now **captures the method** from `auth_failed` into `SessionTab.authMethod`; `connCategory(state, message, hostKeyPending, authMethod?)` prefers it (`password → badpass`, else `auth`) and only falls back to (broadened) message markers (`auth failed` / `authentication failed` / `permission denied`). So badpass/auth are now reliable regardless of message order.

3. **Inline re-auth restored** per the mockups. New `ReauthPanel.tsx` (rendered via ConnState's new `reauthSlot`, between diagnosis and technical details, on `auth`/`badpass` only): lock-icon header "Ввести данные заново", **Доступ** segment (SSH-ключ / Пароль), password field with eye toggle **or** a key `<select>` (saved ssh_key creds), full-width primary **"Сохранить и подключиться"** (saves creds to the host — rotate/create password or link+default the chosen key, `link_host` upserts — then close+reopen → reconnect). Default method = the failed one (auth → key, badpass → password) with **autofocus** on that field. For auth/badpass the bottom row drops the duplicate Reconnect (the Save button covers it), keeping only **Изменить хост** (ghost) + an **attempt counter** (`authAttempts: Record<hostId, number>` in the store, bumped on `auth_failed`, reset on `ready`). The "Что проверить" checklist no longer shows on auth/badpass (replaced by the panel); timeout/refused/dns/network keep it. `badpass`/`auth` raw details render structured `AuthError { method, user, message }`.

i18n: added `conn.reauth.*` (ru+en). Key `<select>` is a styled native element (could become a Combobox later). Frontend `tsc --noEmit` + `vite build` green.

## Latest — conn-states: actions aligned to spec, inline re-auth removed

Per the prototype spec, `auth` (key rejected) and `badpass` (wrong password) now show the **same actions as other errors** — Reconnect (primary) + Edit host (ghost) — instead of the inline password/key re-auth that the previous pass had preserved. Consequences:

- Removed from `SessionView`: the inline re-auth UI and its handlers (`connectWithPassword`, `linkAndReconnect`, `addKeyAndReconnect`), the `pw`/`reauthBusy`/`keyPickerOpen`/`addKeyOpen` state, the `isAuthScreen` flag, and the now-unused imports (`useState`, `KeyRound`, `useCredentialsStore`, `credApi`/`hostsApi`/`encodeSecret` from ipc, `Input`, `SavedCredentialPicker`, `AddKeyModal`). To change credentials after an auth failure the user goes through **Edit host**.
- `badpass`/`auth` "Технические детали" now render a structured `AuthError { method: password|publickey, user: <real>, message: "<real session.message>" }` (real values, no faked codes). hostkey keeps `HostKeyError { algo, fingerprint, changed }`.
- The auth step label/diagnosis/fixes for `badpass` already matched the spec (step "Аутентификация · пароль — неверный пароль", headline "Неверный пароль", checklist раскладка/Caps Lock · верный логин · SSH-ключ); unchanged.
- Dead CSS left behind in `SessionView.module.css` (`.reauth*`, `.failBox*`, `.card*`, `.dead*`, `.hostKey*`) — harmless, can be swept later.

## Latest — connection-states screen (handshake log + taxonomy)

Replaced the raw error dump / `ConnectingOverlay` / host-key banner in `SessionView` with one presentational component, `ConnState` (`ConnState.tsx` + `.module.css`, adapted from the user prototype to tokens/CSS-modules/i18n).

- **Live handshake log**, 4 steps (Разрешение адреса → TCP-соединение → Аутентификация → Открытие сессии), driven by the **real** `session.state` (resolving/connecting/authenticating) — no fake timers. Step states: done (green check) / active (spinner) / pend (gray dot) / fail (red ✕ + reason). The failing step is derived from the category, so you see *where* it broke.
- **Error taxonomy** via `connCategory(state, message, hostKeyPending)`: `timeout` / `refused` / `dns` / `network` / `auth` (key rejected) / `badpass` (password rejected — distinct copy/fixes) / `hostkey` (amber warning) / `generic`. `auth` vs `badpass` split on the SSH `auth_failed (method)` message (`password` → badpass, else auth). hostkey = `session.hostKey` present.
- Each error shows: colored headline + status dot, plain-language **diagnosis** banner (interpolated `{addr}/{port}/{user}`), **"Что проверить"** checklist with real commands (`nc -vz {addr} {port}`, `systemctl status sshd`) for timeout/refused/dns/network/auth/badpass, and a collapsible **"Технические детали"** with the raw message + copy button.
- **Actions** composed by `SessionView` via `children`: connecting → Cancel; timeout/refused/dns/network/generic → Reconnect + Edit host; auth/badpass → inline re-auth (password + key picker + connect) + Edit host (re-auth handlers preserved); hostkey → Accept (amber) / Reject (reuses `acceptHostKey`/`rejectHostKey`). Connecting card overlays the mounted Terminal/RdpViewport (`.connecting` absolute) so the terminal stays mounted for reattach.
- ms timings omitted (no real per-phase backend source — honest over decorative).
- Removed the old `failCategory`/`failHeadlineKey`/`buildConnectLog` helpers and `ConnectingOverlay`; added ~50 `conn.*` i18n keys (ru+en).

## Latest — host inspector densification (inline labels)

Right-pane host editor was too tall vertically (label-above-control per field) → didn't fit without scrolling. Switched to **inline labels**: each field is now a `.frow` row `[label 92px][control flex]`, min-height 34px.

- **CSS (`HostDetail.module.css`):** new `.frow/.frow__l/.frow__c/.frow__port/.frow__sep`. Compacted metrics: `.body input` 32→34, body gap 11→9 + padding →13/14, header →11/14, title 13.5→14, connectRow →10/14, footer →8/14, credentialPanel gap →9.
- **JSX (`HostDetail.tsx`):** 7 rows reordered to Name / Address:Port / Protocol / Login / Access / Key|Password / Group. Address+Port merged into one row (`[Address ___] : [port 56px]`). Each field (main body + `CredentialPanel` login/auth/key + `passwordField`) wrapped in `.frow` + `.frow__c`; dropped the `<section>` wrapper around `CredentialPanel`. Advanced section keeps the stacked layout.
- **i18n:** `dialog.host.authLabel` "Аутентификация"→"Доступ" (EN "Authentication"→"Access") so it fits the 92px label column.

Live-save / draft→real focus continuity untouched (mechanism lives in `HostForm`/`promotedId`, not the field wrappers); the Address `<Input>` keeps its identity + `autoFocus` in draft mode. Body ~553px → ~318px.

## Latest — connection-failure UX (failed card triggers on connecting-drop)

The compact failure card + step log + `common.cancel`/immediate-close already existed, but a **connect timeout** (and other connecting-time drops) landed in state `closed` → the raw OS error showed in the plain `EmptyState`, never the nice `failBox`. Fix in `store/index.ts` `handleEvent`/`handleRdpEvent` `closed`: if the session never reached `ready` and it wasn't a `user_requested` close, set state **`failed`** (keeping the raw message so `failCategory` matches the OS error code, e.g. 10060 → "Хост не отвечает", and `failRaw` shows the full text under "Подробнее"). Normal mid-session drops (was `ready`) still show `closed`.

Connect-overlay button relabeled to a dedicated key **`session.cancelConnect`** (RU "Отменить" / EN "Cancel") instead of `common.cancel` ("Отмена"). `close()` already removes the tab immediately and tears the actor down in the background, so cancel is responsive even during a blocking handshake.

## Latest — RDP clipboard images: permission + RGBA fix

Two bugs kept clipboard **images** from working (text was fine):
1. **Permissions:** `crates/rh-app/capabilities/default.json` granted only `clipboard-manager:allow-{read,write}-text`. Added `allow-write-image` + `allow-read-image` — without them `readImage`/`writeImage` were silently denied (this blocked **both** directions; local→remote started working once added).
2. **Remote→local format:** we were emitting PNG and writing it via `Image.fromBytes`, which only decodes when tauri is built with the `image-png` feature (it isn't) → silent failure. Switched to **raw RGBA**: new `RdpSessionEvent::ClipboardImage { width, height, rgba_base64 }` (replaces the image-over-`Clipboard{mime}` path); `dib_to_rgba` replaces `dib_to_png_base64` (dropped the PNG encoder); the frontend builds `Image.new(rgba, w, h)` and writes it — no `image-png` feature needed. Added a `console.warn` on write failure for future diagnosis.

Both directions now confirmed: local→remote ✅ (user-verified); remote→local pending re-verify after this fix.

## Latest — RDP clipboard images (CF_DIB, both directions)

Extends the CLIPRDR bridge from text-only to text **+ images**, via `CF_DIB`. Files (FileContents stream) stay deferred (complex).

- **`crates/rh-rdp`:** `RdpCommand::SetClipboardImage{ width, height, rgba }`. The clipboard `local` state is now `LocalClip { text, image }` (`LocalImage` = top-down RGBA); the clip channel carries `LocalClipUpdate::{Text,Image}`. `ClipboardBridge` advertises `CF_UNICODETEXT` and/or `CF_DIB` per what's held; gained a `pending_format` field (the data response carries no format id, so we remember what we requested in `on_remote_copy`). New helpers: `dib_to_png_base64` (CF_DIB → PNG; handles 24/32 bpp, BI_RGB & BI_BITFIELDS, V3/V4/V5 headers, bottom-up/top-down; output opaque RGBA), `rgba_to_dib` (RGBA → 32-bpp BI_RGB bottom-up BGRA), `encode_rgba_png_base64`. `on_format_data_response` decodes per `pending_format`; `on_format_data_request` answers `CF_DIB` with `FormatDataResponse::new_data(dib)`. API verified against `ironrdp-cliprdr 0.5.0` (`CF_DIB`=8/`CF_DIBV5`=17, `new_data(impl Into<Cow<[u8]>>)`).
- **`crates/rh-app`:** `RdpSessionManager::set_clipboard_image`; `RdpClipboardImageRequest` DTO (`rgba_base64` to keep IPC light); `rdp_session_set_clipboard_image` command (base64-decodes, registered).
- **Frontend:** remote→local — `store.handleRdpEvent` `clipboard` case now also handles `image/png`: base64 → `TauriImage.fromBytes` → `writeImage` to the OS clipboard. local→remote — `RdpViewport.onFocus` reads the OS clipboard image (`readImage`→`rgba()`+`size()`), base64-encodes (chunked `bytesToBase64`), and pushes via `onLocalClipboardImage`→`rdpSession.setClipboardImage`; prefers image over text. `ipc.ts` wrapper added; `RdpSessionEvent`/types unchanged (reuses `clipboard{mime,data}`).

**Notes / risk (user compiles):** the focus image-read is guarded by a size key (`WxH`) to avoid re-transferring the same image on every refocus — so two **different** images of identical pixel dimensions copied back-to-back won't re-push (remote keeps the first); rare, acceptable for v1. Large images transfer raw RGBA base64 over IPC (heavy but one-shot). DIB parsing covers common Windows formats; exotic palettized DIBs are skipped (error response).

## Latest — RDP server cursor (client-rendered CSS cursor)

The remote cursor shape wasn't shown (`enable_server_pointer: false`) — you always saw the local arrow, never the remote I-beam/resize/hand. Now enabled with **client-side** rendering (not software-composited into the frame, which lags the server repaint): the cursor tracks the local mouse instantly.

- **`crates/rh-rdp`:** `build_config` → `enable_server_pointer: true`, `pointer_software_rendering: false` (so IronRDP emits `PointerBitmap` updates with **non-premultiplied** RGBA via the Accelerated target instead of compositing). New actor output arms for `ActiveStageOutput::{PointerBitmap, PointerHidden, PointerDefault}` → new `RdpSessionEvent::{PointerBitmap{ w,h,hotspot_x,hotspot_y,rgba_base64 }, PointerHidden, PointerDefault}` (RGBA base64-encoded, like frame tiles). `DecodedPointer` fields confirmed from `ironrdp-graphics 0.5.0`: `width/height/hotspot_x/hotspot_y/bitmap_data` (top-down RGBA; decode handles the bottom-up flip). Reactivation path already reads the pointer flags from `Finalized` → cursor survives a resize.
- **Frontend:** `RdpViewport.applyEvent` handles the three pointer events; `pointerToCss()` decodes base64 RGBA → offscreen canvas → PNG `data:` URL → `canvas.style.cursor = url(...) hotspotX hotspotY, auto`. Hidden → `none`, default → `default`. `store.handleRdpEvent` routes the new events through `pushRdpEvent`. Types added to `RdpSessionEvent`.

**Notes / risk (user compiles):** Chromium ignores cursor images > 128px → those fall back to the arrow (rare large pointers). XOR-inverted cursor pixels render transparent on the Accelerated target (IronRDP behavior). If a cursor looks upside-down, `bitmap_data` row order needs a flip — but the decoder already accounts for `flip_vertical`, so it shouldn't.

## Latest — RDP fullscreen keyboard capture + maximize→fullscreen strip fix

**Fullscreen strip (frontend):** entering fullscreen from an already-**maximized** window left a black strip (~taskbar height) at the bottom — Windows keeps the maximized client rect (= work area) so the surface grew to full screen but the content stayed work-area-tall. Fix in `RdpViewport.tsx` `toggleFs`: if the window `isMaximized()`, `unmaximize()` it before `requestFullscreen()`. Belt-and-suspenders: `.wrap:fullscreen { 100vw/100vh }` in CSS + inline `100vw/100vh` on the canvas when `isFs`.

**Keyboard capture (the mstsc feature — "Apply Windows key combinations: on the remote"):** in fullscreen, `e.preventDefault()` can't stop the OS Win key (Start opens locally) and WebView2's Keyboard Lock API is unreliable for Win. Proper fix = an OS-level low-level keyboard hook, the whole reason we own the input path.

- **`crates/rh-app/src/kbd_hook.rs` (new, `cfg(windows)`):** installs `WH_KEYBOARD_LL` on a dedicated thread (with a `GetMessageW` pump). While capture is armed (RDP fullscreen) **and** our window is foreground (safety: never hijacks another app), the hook swallows each key (`return 1`) and forwards its hardware scancode + extended flag to the active RDP session via a `tokio` forwarder task (`AppHandle::state::<AppState>().rdp_sessions.send_input`). Ctrl+Alt+Enter is recognized in the hook and relayed as `rdp:exit-fullscreen` (never forwarded) so the user can always escape. Uses `windows-sys` (Foundation/WindowsAndMessaging/Input.KeyboardAndMouse/System.LibraryLoader). Non-Windows: no-op stubs.
- **`crates/rh-rdp`:** new `RdpInputEvent::RawScancode { scancode: u8, extended: bool, pressed: bool }` → fed straight to `Scancode::from_u8` (bypasses `code_to_scancode`).
- **`crates/rh-app`:** `RdpKbdCaptureRequest` DTO + `rdp_session_kbd_capture` command (registered); `kbd_hook::init` called in `setup()` (records the main-window HWND).
- **Frontend:** `rdpSession.kbdCapture(sid, on)` in `ipc.ts`; `RdpViewport` arms/disarms capture on `fullscreenchange` (+ `release_all_modifiers` on exit) and listens for `rdp:exit-fullscreen`; `SessionView` wires `onKbdCapture`.

**Risk (unproven, user compiles):** first use of `windows-sys` here — `SetWindowsHookExW`/`KBDLLHOOKSTRUCT.flags`/`VK_*`/`HC_ACTION` integer types and the `windows-sys 0.59` vs Tauri's pin may need a tweak. The hook is gated to fullscreen + foreground so it can't lock the keyboard; Ctrl+Alt+Enter always exits.

## Latest — RDP clipboard (CLIPRDR), bidirectional text

Real clipboard over the `cliprdr` static virtual channel, text only (CF_UNICODETEXT). The OS clipboard itself lives in the **frontend** (Tauri `@tauri-apps/plugin-clipboard-manager`); `rh-rdp` only shuttles text.

**Backend (`crates/rh-rdp`):** enabled the `cliprdr` feature on `ironrdp`; new `RdpCommand::SetClipboard(String)`. `actor.rs`: a `ClipboardBridge` (impl `CliprdrBackend`, text-only) registered on the connector via `ClientConnector::with_static_channel(CliprdrClient::new(backend))`. Incoming channel PDUs auto-dispatch through `active_stage.process` → backend callbacks; the backend can't touch the stage, so it posts `ClipMsg`s that the worker loop drains and encodes via `get_svc_processor::<CliprdrClient>()` + `process_svc_processor_messages`. Remote→local: `on_remote_copy` (text offered) → `initiate_paste(CF_UNICODETEXT)` → `on_format_data_response` decodes UTF-16LE → `RdpSessionEvent::Clipboard{mime,data}`. Local→remote: UI pushes text (`SetClipboard`) → stored in `Arc<Mutex<Option<String>>>` + `initiate_copy([CF_UNICODETEXT])`; server's `on_format_data_request` answered with `FormatDataResponse::new_unicode_string(&text).into_owned()`.

**App (`crates/rh-app`):** `RdpClipboardRequest` DTO; `RdpSessionManager::set_clipboard`; `rdp_session_set_clipboard` command (registered in `main.rs`).

**Frontend:** `ipc.ts` `rdpSession.setClipboard`; store `handleRdpEvent` `clipboard` case writes remote text to the OS clipboard (`writeText`); `RdpViewport` reads the local clipboard on focus (`readText`) and hands it up via `onLocalClipboard`; `SessionView` forwards it to `setClipboard`.

**Risk (unproven API, user compiles):** the IronRDP CLIPRDR surface was verified against crates.io source (`ironrdp-cliprdr` 0.5.0, `ironrdp-session` 0.8.0, `ironrdp-connector` 0.8.0) but never compiled here — most likely fix-up point is the SVC send path (`get_svc_processor` borrow scoping, `process_svc_processor_messages` generics) or the `impl_as_any!` re-export. **Test:** copy text in the remote → it lands on the local clipboard; copy text locally → click into the RDP view (focus) → paste into the remote. Note `enable_clipboard: false` in `RdpOpenOptions` is unrelated (legacy flag) and left as-is.

## Latest — RDP keyboard + modifier-sync (Stage 4.2)

Wired keyboard input end-to-end in `rh-rdp` — the last big functional hole in RDP. The whole frontend path already existed (`RdpViewport` emits `key` / `sync_modifiers` / `release_all_modifiers` on keydown/keyup/focus/blur; types + IPC in place); the only gap was the actor dropping those three events. **Backend-only change** (`crates/rh-rdp/src/actor.rs`); no frontend/IPC/DTO churn.

- **Scancode map** — `code_to_scancode(&str) -> Option<(extended, u8)>` maps the browser `KeyboardEvent.code` to PS/2 **Set 1** scancodes: full letters/digits/punctuation/function row, nav cluster + arrows (extended `0xE0`), numpad (incl. extended `NumpadDivide`/`NumpadEnter`), L/R modifiers + Win/Menu, intl keys. Nav-cluster vs numpad code reuse (e.g. `Home` `(true,0x47)` vs `Numpad7` `(false,0x47)`) is disambiguated by the extended flag — the `Database` keys its held-state table on `(extended, code)`. Unmapped codes are logged at debug and ignored.
- **Key path** — `Scancode::from_u8(extended, code)` → `Operation::KeyPressed/Released` → `Database::apply` → `process_fastpath_input` (same pipe as mouse). The `Database` emits release-before-press on auto-repeat, so browser repeats are forwarded as-is.
- **Modifier-sync (the anti-stuck-modifier fix — the reason we own the input path):** on blur the UI sends `ReleaseAllModifiers` → `Database::release_all()` (blanket KeyUp of everything held — cures stuck Ctrl/Alt after Alt-Tab); on focus it sends `SyncModifiers` (OS modifier state) → `sync_mod` diffs against `is_key_pressed` and emits only deltas (no spurious repeat). Syncs Ctrl/Alt/Shift/Meta. `Ctrl+Alt+Enter` (fullscreen) is intercepted in the UI and never reaches the remote.
- **`Scancode` API now proven** (was the flagged unknown). `ironrdp-input` **0.5.0**: `Scancode::from_u8(extended: bool, code: u8)`, `Operation::{KeyPressed,KeyReleased}(Scancode)`, `Database::{apply, is_key_pressed, release_all}`. Confirmed against the crate source, so it should compile first try — but **user compiles** (no cargo in sandbox), so report any drift.
- **Deferred (keyboard polish, not blockers):** lock-LED sync (Caps/Num/Scroll) needs a `TS_SYNC_EVENT` (not exposed by ironrdp-input's fast-path `Database`); PrintScreen is best-effort (`E0 37` half), Pause omitted (E1 sequence). Doc updated: `docs/specs/rdp-pipeline.md` (status → 4.2, §7 rewritten).
- **Test next session (live, Windows, RDP `5.42.106.222`):** type into a remote session (letters/symbols/Enter/Backspace, arrows, Shift+chars), Alt-Tab away and back → no stuck Ctrl/Alt, Ctrl+Alt+Del / Ctrl+C etc. behave.



## Latest — vault/sync foundation (rh-vault)

New crate `crates/rh-vault` (registered in workspace; `rh-app → rh-vault → rh-core`). Backend-agnostic — **no** Tauri/SQLite/keychain/network. It is the part of "accounts + sync" that does **not** depend on the still-open A/B/C backend choice. Full spec: `docs/specs/sync.md`.

- **Crypto** — master password → 256-bit key via **Argon2id** (`argon2` crate; OWASP defaults 64 MiB / t=3 / p=1, params+salt stored cleartext in the envelope header). Payload sealed with **AES-256-GCM** via pure-Rust **RustCrypto `aes-gcm`**, fresh random 96-bit nonce per seal (RNG via `getrandom`). Cleartext header (`{format, kdf}`) bound as AAD so KDF-param downgrade or ciphertext tamper fails the tag. Wrong password == corrupt blob == one `Decrypt` error (no oracle). **(Switched off aws-lc-rs — its `aws-lc-sys` C lib needs NASM + C11 to build on Windows; `aes-gcm` is pure Rust, no native build.)**
- **Portable vault** — `VaultEnvelope { format, kdf, nonce, ciphertext }` serializes to a JSON export string; `seal_snapshot` / `open_envelope` / `to_export_string` / `from_export_string`. The bytes that leave the device are ciphertext; the master password never does. Credential secrets (kept in the OS keychain locally, never in SQLite) ride **inside** the encrypted blob via `SyncCredentialPayload`; a test asserts the secret never appears plain or base64 on the wire.
- **Sync model** — four `EntityKind`s replicate (Host/Group/Credential/Setting), each a `SyncRecord { kind, id, meta, data }` with **opaque `serde_json::Value` payloads** (new rh-core fields flow through automatically; machine-set fields like `detected_os`/`last_connected_at` ride along — acceptable v1, documented). `RecordMeta { rev: Hlc, origin: NodeId, deleted, field_revs }`.
- **Conflict resolution** — **record-level last-write-wins** keyed by `(kind, id)`, winner by greater `(rev, origin)` total order; **Hybrid Logical Clock** (`Hlc`/`HlcGenerator`) stamps writes monotonically (safe against skewed/backwards wall clocks; `observe()` folds in remote clocks). Tombstones are records, so delete-vs-edit resolves on the same order (later wins → stays-deleted or undelete). Merge is commutative/idempotent/convergent (tested). **Field-level LWW is the planned v2** — `field_revs` map already reserved (empty), so the upgrade is additive, no format break.
- **Transport seam** — `#[async_trait] SyncRemote { pull, push(expected) }` with **optimistic concurrency** (version token; stale push → `RemoteConflict` → pull/re-merge/retry). The one place A/B/C plugs in; the sync loop is written once against the trait. `MemoryRemote` test double included. Spec maps each backend (A server `If-Match`; B object-store ETag conditional PUT; C cloud-folder hash+mtime).
- **Verification** — unit tests in every module + two integration tests (`tests/end_to_end.rs`: two-device converge through encrypted remote with secret intact + not-on-wire; concurrent-push forces re-merge). **User runs `cargo test -p rh-vault`** (sandbox has no cargo — tests are the proof; frontend untouched this turn).
  - **Build note:** originally specified aws-lc-rs; `cargo test -p rh-vault` failed because `aws-lc-sys` needs NASM + a C11 MSVC toolchain (it was only a lockfile entry before — russh/rustls use `ring`, never compiled aws-lc-sys). Swapped the AEAD to RustCrypto `aes-gcm` (0.10) + `getrandom` (0.2) — both already in `Cargo.lock`, pure Rust, no native build. Argon2id (`argon2` 0.5) unchanged. To restore aws-lc-rs instead: install NASM + `AWS_LC_SYS_PREBUILT_NASM=1`.
- **OPEN — A/B/C backend choice (awaiting user).** Recommended order: **C** (cloud-sync folder file — ship fastest, zero infra, validates the whole pipeline) → **B** (object store — no desktop-client dependency, real conditional-PUT) → **A** (self-hosted server — only if hosted Team sync becomes a goal). Foundation is transport-agnostic, so the choice changes only one trait impl.
- **Deferred to next turn (after compile + choice):** IPC `vault_export`/`vault_import`/`vault_status`; frontend master-password UX + export/import UI; wiring the **Team** storage scope (`TabBar.tsx`, `storage.*`) to "sync configured"; the concrete `SyncRemote` impl + `rh-app` sync engine (snapshot build from storage+keychain, merged write-back).

**Follow-up 2 (region-diff — the real fix):** instrumentation revealed the smoking gun — full-frame JPEG encode was **~130ms** each (the RGBA→RGB copy ran in *unoptimized* rh-rdp; the dev profile only optimized dependencies, not our own crate), capping fps at ~7 and blocking the worker; and full ~130KB base64 frames congested the single webview IPC bridge, so input invokes (clicks) queued behind them and arrived 3-15s late. Fixes: (1) **region-diff** — compute the changed bounding box vs the last frame and JPEG only that rectangle (`FrameJpeg` now carries x,y); a click/keystroke touches a tiny area → tiny encode + tiny payload → no IPC congestion. Frame coalescing was *removed* (each region is a distinct rect; dropping one leaves a stale patch). (2) `[profile.dev.package.rh-rdp] opt-level = 3` so the hot pixel loops are optimized in dev too. Added `rdp frame stats` (fps / avg encode ms / payload KB) + per-click logging for diagnosis.

## Latest — tab-bar scroll, storage scope switcher, SFTP byte-resume

- **Tab-bar horizontal scroll** — session tabs live in a `.scroller` (flex, `overflow-x:auto`, hidden scrollbar); wheel scrolls horizontally; the active tab auto-scrolls into view. Vault/Tools + `+`/gear/window-controls stay pinned.
- **Storage scope switcher** — the Vault chevron opens a Personal/Team dropdown (`storage:scope`). **Personal** is active; **Team** is disabled ("needs sync") — the UI seam for the future sync feature. No backend yet.
- **SFTP byte-offset resume** — transfers now resume from the destination's current size instead of restarting. `download/upload/copy_stream` gained an `offset` param; `SftpConn::size()` stats the remote partial. `SftpTransferRequest.resume` (default false); the dock's ↻ retry sets `resume:true`. Offset=0 path is unchanged (normal transfers unaffected).
  - **Risk (unproven russh-sftp, user compiles):** resume branch uses `File` `AsyncSeek` (remote read seek) + `open_with_flags(WRITE|CREATE|APPEND)` / `russh_sftp::protocol::OpenFlags`. If these names differ, only resume is affected.
- **SSH agent-forward serving — deferred to a spike.** Needs the client-`Handler` forwarded-agent-channel hook + an OS-agent byte bridge (security-sensitive, unproven API). Spike first (like sftp/rdp), then implement.



- **App icons** — real `rhub` icon set generated into `crates/rh-app/icons/` (32 / 128 / 128@2x / `icon.ico` multi-res / `icon.icns` / `icon.png`) from the designer's 1024² PNG. Window + tray now show the brand icon.
- **Favorites** — new `Host.favorite: bool` (rh-core). Migration **v9** (`ALTER TABLE hosts ADD COLUMN favorite … DEFAULT 0`; `CURRENT_SCHEMA_VERSION = 9`; v1.sql fresh schema updated). Runtime SQL in `host_store` (INSERT/UPDATE/SELECT + row map — no sqlx-macro/offline-cache impact). DTOs: `favorite` on full/create/update; handlers set/patch it. Frontend: `favorite` on `HostDto` + create/update; a **star toggle** in the HostDetail header (live-saves immediately, like agent-forwarding). Tray gained a **Favorites** submenu (pinned hosts, by name), rebuilt on `hosts:changed`.



Added a system-tray icon (`rh-app/src/tray.rs`, `tray-icon` feature on the tauri dep). Right-click menu: **Open RemoteHub**, **Recent** (hosts by `last_connected_at`, newest 8), **Groups** (nested submenu per group), separator, **Quit**. Left-click shows/focuses the window. Selecting a host emits `tray:connect <host_id>`; `AppShell` listens and opens it via the normal `sessions.open` flow (connect logic stays in one place). Menu rebuilds on `hosts:changed` / `groups:changed`.

- **Favorites** submenu is NOT wired — `Host` has no favorite flag yet (only `tags`, `group_id`, `last_connected_at`). Needs a `favorite` bool (migration + star toggle in the editor) — small follow-up.
- **App icons** still the Tauri placeholders; tray reuses `default_window_icon()`. Drop a 1024² `rhub.png` and regenerate the `icons/` set (32 / 128 / 128@2x / icon.ico / icon.icns).
- **Risk (Tauri 2.1 menu/tray API, user compiles):** `show_menu_on_left_click` (was `menu_on_left_click` pre-2.1), `SubmenuBuilder`/`MenuItemBuilder` shapes, `tray_by_id`/`set_menu`. Spike-grade surface — report compile errors.



- **Resume/retry interrupted transfers** — failed/cancelled queue rows get a ↻ retry (fresh `transfer_id`, re-enqueued); dock header gains "Retry failed". Cancel now keys on the item's current `transfer_id` (survives retry). (True byte-offset resume is still a follow-up — retry restarts from 0.)
- **Editable path field** — clicking the breadcrumb's empty area turns it into a path input (`navigateTo`): Enter validates by listing — success navigates, failure shows a red border and keeps the current listing (no clobber). Crumb buttons still navigate per-segment.
- **Local-terminal restore-on-reload** — `LocalPtyManager` now mirrors the SSH hub: per-session 256 KiB output ring + swappable sink + `list()`/`reattach()`. New commands `local_session_list` / `local_session_reattach`; `restoreSessions` rebuilds local tabs and replays scrollback after a webview reload (was SSH-only). Local shells no longer vanish on reload.



Closed the SFTP backlog from the roadmap's item 2:
- **Streaming host↔host copy** — `rh_ssh::sftp::copy_stream(src_conn, dst_conn, …)` chunks A→B with real byte-progress + cancel (was buffered 0→full). run_transfer locks by session-id order (A==B → one lock; A≠B → ordered, no deadlock).
- **TOFU host-key pinning** — `SftpConn::connect` now takes `Arc<dyn KnownHostsStore>`; `SftpHostKey` handler does silent trust-on-first-use against the shared `known_hosts` store (matches the SSH path), **rejects a changed key**. Replaces trust-all. `fingerprint_sha256` is now `pub(crate)` in `actor.rs` and reused.
- **SSH-agent auth for SFTP** — `try_auth` handles `RevealedCredential::Agent` (Pageant / OpenSSH pipe), mirroring the shell actor's agent block.
- **chmod** — `SftpConn::chmod(path, mode)` (`set_metadata` w/ `FileAttributes.permissions`), `sftp_chmod` command, context-menu "Permissions…" (host only) → a 3×3 rwx grid dialog with live octal.

**New risk flags (unproven russh-sftp surface):** `set_metadata` + `russh_sftp::protocol::FileAttributes` path (chmod). Agent path mirrors the proven actor code; copy_stream reuses already-compiled open/create/read/write/shutdown.

## Latest — Navy default + Redpanda theme; search-field click target

Theme picker gained **Navy** (deep-blue surfaces) — now the **default** (`Theme::Navy` `#[default]`) — and **Redpanda** (near-black warm surfaces + coral-red accent `#f0552f`, the only theme that shifts the accent). Both are `:root[data-theme=…]` token blocks; applied via `AppShell` `data-theme`. The Storage search hero is now a `<label>` so the whole 54px frame focuses the input (no more narrow hit area).



A complete Termius/commander-style SFTP browser, built incrementally and live-verified. Spec/pipeline reference: **`docs/specs/sftp.md`**.

**Backend**
- `rh-ssh/src/sftp.rs` — `SftpConn`: connect (TrustAll host-key — TOFU is a follow-up) via russh `request_subsystem("sftp")` + `russh_sftp::client::SftpSession`; `list` (`SftpEntry{name,path,is_dir,size,modified:Option<i64> from mtime, perms:Option<String> via fmt_perms()}`, dirs-first), `read_file`/`put_in_dir`, `download`/`upload` (buffered), **`download_stream`/`upload_stream`** (256 KiB chunks, cancel flag `&AtomicBool`, `progress: &mut (dyn FnMut(u64)+Send)`), `rename`, `remove` (recursive, boxed async), `mkdir`. `read_dir`/`open`/`read_to_end`/`metadata` are spike-proven; `create`/`write_all`/`shutdown`/`rename`/`remove_file`/`remove_dir`/`create_dir`/`mtime`/`permissions` were unproven russh-sftp surface (compiled clean on Windows).
- `rh-app/src/sftp_session.rs` — `SftpManager`: `HashMap<SessionId, Arc<Mutex<SftpConn>>>` + a per-transfer cancel registry (`cancels: HashMap<String, Arc<AtomicBool>>`, `register/unregister/cancel_transfer`). `rh-app/src/api/sftp_sessions.rs` — commands `sftp_open` (host_id → `revealed_creds_for` → connect), `sftp_list`, `sftp_close`, `sftp_download`/`sftp_upload`/`sftp_copy` (legacy buffered, still registered), **`sftp_transfer`** (`Channel<u64>` byte-progress + `dst_name` override + cancel), `sftp_transfer_cancel`, `sftp_rename`, `sftp_remove`, `sftp_mkdir`.
- `rh-app/src/api/local_fs.rs` — the local side of the explorer: `fs_home`, `fs_drives` ("This PC" — enumerates Windows drive roots), `fs_list` (with `clean()` stripping the `\\?\` verbatim prefix so breadcrumbs are clean), `fs_rename`, `fs_remove` (`remove_dir_all` for dirs), `fs_mkdir`. `FsEntry`/`SftpEntry` share `{name,path,is_dir,size,modified,perms}`.
- tokio gained `io-util` in `rh-ssh` for the streamed copies.

**Frontend** (`ui/src/components/sftp/SftpView.tsx` + `.module.css`, ~1.2k lines)
- Two interchangeable panels ("точка А | точка Б"), each a `usePanel()` hook: source (local | host), session, listing, sort, multi-select, hidden-files toggle (hosts show dotfiles by default, local hides), filter, inline-rename, create-folder. Endpoint switcher with "This machine"/"Hosts" sections; clean breadcrumbs with a PC-icon root → drives.
- **Transfer matrix:** local↔host (download/upload), host↔host (copy through the app). Four ways to move: center-rail →/← (armed on the active pane's selection), double-click a file, drag-and-drop between panels (drop highlight + plate), context menu (ПКМ: send/open/rename F2/copy-path/delete Del).
- **Transfer queue dock** (bottom, collapsible): max 2 parallel (`useTransfers` orchestrator), per-row progress bar / speed / ETA / cancel, total speed, "clear finished"; streamed via `sftp.transfer` + `Channel<u64>`.
- **Name-conflict dialog** on collision: Replace / Keep both (auto `name (1).ext`, via `dst_name`) / Skip.
- File ops: search/filter, rename (inline), delete (confirm dialog, recursive), new folder.
- All `invoke` through `lib/ipc.ts` (`localFs.*`, `sftp.*`); every string i18n'd (`sftp.*`).

**Known follow-ups:** streaming host→host copy (currently buffered, progress 0→100); SFTP TOFU cert pinning (trust-all now); agent-auth for SFTP; transfer queue speed-smoothing; perms-edit (chmod) context item.

## Latest — Local terminal (real PTY)

`portable-pty` in `rh-app`; `rh-app/src/local_pty.rs` `LocalPtyManager` + `spawn_pty` worker (PTY → shell: PowerShell on Windows / `$SHELL`|bash unix, overridable). Reuses `SshSessionEvent`/`SessionCommand` so the existing `Terminal.tsx` works unchanged. Commands `local_session_open/close/input/resize` (`rh-app/src/api/local_sessions.rs`). Shell choice persisted via settings key `local.shell` (rh-core `Settings.local_shell`, `TerminalSection.tsx`). Resize-race fixed (re-send resize once `sessionId` set, else ConPTY sticks at 80×24). Restore-on-reload deferred.

## Latest — Tools credential manager fixes

In the Tools screen, the credential list now: reveals on row click (copy icon appears inline next to a revealed password; no edit-on-click — creds are edited in the host form), shows **only credentials still linked to ≥1 host** (orphans from `unlinkHost` hidden via a `useCredentialLinks` aggregation), and dropped the "+ Add" path (credentials are created in the host editor). Backend orphan-cleanup-on-unlink offered but deferred.



First report of real-world lag (vs native mstsc). Two fixes for the two causes:
- **Input latency** — the worker's idle read timeout was 400ms, so a click sent during an idle read waited up to 400ms before the worker drained + forwarded it. Dropped `READ_POLL` to **16ms** (read_pdu still returns immediately on data; this only bounds the idle wait). This is the dominant click-responsiveness fix.
- **Frame transport** — we were shipping the full 1280×800×4 = 4MB RGBA framebuffer as a serde-JSON number array (~14MB of text) every 100ms (the bottleneck flagged in `rdp-session.md` Open-Q #1). Now: JPEG-compress (quality 72) + base64 → new `RdpSessionEvent::FrameJpeg`, ~40-60KB/frame (~250× smaller), **only sent when the framebuffer actually changed** (raw compare against the last-sent buffer → zero traffic when idle), ~15fps cap. `image` + `base64` moved into `rh-rdp` deps. Frontend decodes the data URL via `Image`/`drawImage`.

Native mstsc will still edge it out (hardware codecs + protocol-level region diffing), but this closes the big gap. Region-diffed frames + an off-thread encoder are the next perf step if needed.

**Follow-up (input backlog + debug-build perf):** first real test showed >10s click latency. Root causes + fixes:
- **Mouse-move flood** — every DOM mousemove was one IPC call; the queue grew faster than the worker drained it, so a click sat behind a flood of moves. Fixed at both ends: frontend throttles `mouse_move` to ~25/s (clicks/wheel/keys still immediate; clicks carry their own coords so dropping intermediate moves is safe), and the worker **coalesces consecutive moves** when draining (only the latest is sent).
- **Debug-build codec cost** — `cargo tauri dev` is unoptimized, making JPEG encode + graphics decode 10-30× slower. Added `[profile.dev.package."*"] opt-level = 3` so dependencies are optimized even in dev (our crates stay unoptimized for fast iteration). First rebuild after this is slow (deps recompile once), then cached.

## Latest — RDP round 2b-2 (mouse input): interactive pointer in the app

Input API **validated by the spike first** (the project's rule for unproven IronRDP surface): extended `rdp_spike` to inject a right-click — it compiled clean (confirming `ironrdp-input 0.5`: `Database::new/apply`, `Operation::MouseMove/MouseButtonPressed/MouseButtonReleased`, `MousePosition`, `ActiveStage::process_fastpath_input` → `ResponseFrame`) and the context menu appeared in the captured PNG. Timing lesson baked in: pointer-move and click must be separated by a beat (in the live app this is natural — real user motion).

Ported the proven mouse path into the app:
- **Actor**: command bridge added — `run` forwards `RdpCommand::Input` to the worker via a std channel; the worker drains it each loop iteration and encodes via `send_input` (mouse move / button / wheel). Keyboard + modifier-sync events are accepted but ignored (next slice — they need the scancode map + `Scancode` API, still unproven).
- **rh-app**: `rdp_session_input` command + `RdpInputRequest` DTO → `RdpSessionManager::send_input` → `RdpCommand::Input`.
- **Frontend** (FE verified): `rdpSession.sendInput` ipc + `RdpInputRequest` type; `SessionView.handleRdpInput` now forwards viewport events to the actor (fire-and-forget). The `ironrdp` meta-crate needs the **`input`** feature (added to `rh-rdp/Cargo.toml`).

You can now **click and scroll** the live desktop. Keyboard is 2b-2b. (Known MVP: mouse-move fires one IPC per event — fine now, throttle/coalesce later with the transport work.)

## Latest — RDP round 2b-1: live desktop in the app (read-only) — spike PROVEN

**Spike (2a) succeeded against the real Windows test server** — `rdp_spike` connected (TLS + NTLM/CredSSP), decoded graphics and saved a full-colour 1280×800 desktop PNG. The IronRDP 0.14 wave + the exact `connector::Config` field set + the blocking connect sequence are confirmed end-to-end on Windows.

**2b-1 ports that proven path into the app as a read-only live viewer.** Because the validated code is *blocking*, the actor runs it on a dedicated OS thread and bridges to async: events out via the Tokio `UnboundedSender` (its `send` is sync), shutdown in via a shared `AtomicBool`. No async-IronRDP API guessing — it's the spike code, verbatim, behind a poll loop (400 ms read timeout so it notices shutdown; full framebuffer pushed at ~10 fps — MVP, region-diff + faster transport deferred per spec Open-Q #1).

- IronRDP moved `rh-rdp` **[dev-dependencies] → [dependencies]** (so the app build pulls it now; the `rdp_spike` example still builds via its remaining dev-deps). First `cargo tauri dev` compile will be slow and *may* surface minor API drift in `actor.rs`, but the connect code is identical to the proven spike, so risk is low.
- `rh-rdp/src/actor.rs` rewritten: `spawn_session` → worker thread (`blocking_session`) doing connect + ActiveStage loop, emitting `StateChanged`(Connecting→Authenticating→Ready)/`Frame`/`Closed`/`Error`. Input/Resize accepted but dropped (2b-2).
- `rh-app`: new `RdpSessionManager` (thin registry, no scrollback — a framebuffer isn't replayable) + `rdp_session_open`/`rdp_session_close` commands (resolve host+password cred, reveal, spawn, forward events→`Channel<RdpSessionEvent>`). Wired into `AppState` + `main.rs`.
- Frontend (FE verified — tsc + vite clean): `SessionOpenOptions` now a ssh|rdp union; `rdpSession` ipc namespace; store routes RDP events (frame-sink keyed by session, latest-frame buffer until the viewport mounts) and branches `createSession`/`teardownSession` on protocol; `RdpViewport` self-registers its frame sink via `registerSessionViewport`. Opening an RDP host now shows the **live remote desktop** (read-only, ~10 fps, no input yet).

Run: `cargo tauri dev`, then open the RDP host in RemoteHub → live desktop. **2b-2 next: input** (mouse/keyboard + modifier-sync) via IronRDP's `input` API — new/unproven, so it's a separate pass.

## Latest — RDP connectivity spike (round 2a): isolated IronRDP connect → PNG

Before wiring IronRDP into the actor/app, validate it in isolation. Added `crates/rh-rdp/examples/rdp_spike.rs` — a near-verbatim port of IronRDP's official blocking `screenshot.rs`: connects (TLS + CredSSP), decodes graphics until idle, saves the desktop to PNG. IronRDP lives in `rh-rdp` **[dev-dependencies]** only (`ironrdp 0.14` + `ironrdp-blocking` + `sspi` + `tokio-rustls` + `x509-cert` + `image`), so the normal app build is unaffected — only `cargo run -p rh-rdp --example rdp_spike` pulls it.

Run: `cargo run -p rh-rdp --example rdp_spike -- --host <IP> -u <USER> -p <PASS> [-d <DOMAIN>] -o shot.png`. If `shot.png` shows the desktop → IronRDP connects to this server and the exact 0.14 API/versions are confirmed. Round 2b then ports the validated connect into the async `rh-rdp` actor + rh-app session path + frontend viewport routing, and adds input. Expect possible version/feature drift on first compile (the spike's purpose is to surface it).

## Latest — RDP actor shell (round 1; compiles, no IronRDP yet)

`rh-rdp` now has the actor shell mirroring rh-ssh: `spawn_session` + `RdpCommand` channel + lifecycle events. No IronRDP deps yet — it reaches `Authenticating` then emits a graceful "not wired" close. Clean compile; round 2 fills `connect_and_pump` (TLS+CredSSP connect, ActiveStage graphics→Frame, input→fastpath) against the real `ironrdp-client` async source, plus the rh-app session wiring + frontend live-routing.

## Latest — RDP trusted-cert store (TOFU for RDP), frontend verified; Rust mirrors known_hosts

Spike-independent prep for RDP. Trusted RDP server certificates get a TOFU store — the exact analog of known_hosts: `RdpCertStore` trait + `TrustedCert`/`RdpCertEntry` (rh-core), `SqliteRdpCertStore` + `rdp_known_certs` table (migration **v7→v8**, additive, `CURRENT_SCHEMA_VERSION=8`), `rdp_certs_list`/`rdp_cert_forget` commands, wired into `AppState`. The actor will use `lookup`/`remember` for cert pinning when it lands. Surfaced as a third tab ("RDP certificates") in the Security dialog (empty until you connect, like Known Hosts was). All a mirror of the SSH known_hosts work — low risk.

## Latest — RDP foundation: contract types + viewport (frontend verified; RUST = types only)

First RDP slice. Deliberately **no IronRDP yet**: the spec mandates a connectivity/transport spike first (run `ironrdp-client` against a real RDP server; benchmark frame transport — Tauri Channel `Vec<u8>`→JSON is too slow at 1080p60, candidates are custom-protocol / localhost WS / SharedArrayBuffer; Open-Qs #1/#5). That spike needs real hardware (yours), so the connect/decode actor is the next slice.

**Contract types** (`crates/rh-rdp/src/lib.rs`, pure types, no IronRDP — compiles as a normal rh-app dep): `RdpInputEvent` (MouseMove/MouseButton/MouseWheel/Key + **SyncModifiers**/**ReleaseAllModifiers** for focus-sync), `RdpSessionEvent` (StateChanged/Frame{region,format,data}/PointerPosition/CertPrompt/Clipboard/Error/Closed), `RdpState`, `PixelFormat`, `FrameRegion`, `RdpCloseReason`, `ColorDepth`, `RdpOpenOptions`, `RevealedRdpCredential`, `RdpSpawnParams`, `ModifierState`, `RdpError(+into_close_reason)`. Layering like rh-ssh (events over mpsc; rh-app bridges to Tauri Channel — NOT tauri in rh-rdp). Mirrored in `ui/src/lib/types.ts`.

**`RdpViewport`** (`ui/src/components/session/RdpViewport.tsx`, verified): `<canvas>` + imperative `applyEvent(frame)` (builds `ImageData`, BGRA→RGBA swap when needed, `putImageData` at region) + full mouse/keyboard capture (display→backing coord mapping) + **focus/modifier sync** — the spec's required-in-foundation fix for stuck modifiers: `blur`→`release_all_modifiers`, `focus`→`sync_modifiers` from last-known physical state (tracked via `getModifierState` on every mouse/key event). Wired into `SessionView` (RDP protocol branch); `onInput` currently routes to a placeholder (`handleRdpInput`) — the backend input channel lands with the actor. RDP sessions still can't be created (session_open → not_implemented) so the branch is dormant but type-complete.

### Next RDP slice (needs your spike): IronRDP connect + decode actor
1. Spike: `ironrdp-client` vs a Windows VM / xrdp — confirm happy-path + pin exact crate versions.
2. Pick frame transport (bench the three options).
3. `rh-rdp` actor: connector + `ActiveStage` loop + cert store (analog of known_hosts) + input mapping (browser code→PS/2 scancode) + frame coalescing. `rh-app`: RDP path in `session_open`, `rdp_session_input` command, bridge events→Channel. Wire `RdpViewport` to live frames + `handleRdpInput` to the channel.



Last SSH item. `ssh -A`: forward the local agent so onward auth on the remote works.

**Data model:** `Host.agent_forwarding: bool`. Migration **v6→v7** `ALTER TABLE hosts ADD COLUMN agent_forwarding INTEGER NOT NULL DEFAULT 0` (`CURRENT_SCHEMA_VERSION=7`, v1.sql bumped + column). Wired through host_store (bool↔INTEGER, `i64::from` / `!= 0`), HostDto, Host{Create,Update}Request (plain `Option<bool>`).

**rh-ssh:** `SshSpawnParams.agent_forwarding`. The actor calls `channel.agent_forward(false)` after channel-open when enabled (confirmed russh API — advertises acceptance). ⚠️ **Serving side deferred:** russh 0.45's client callback is `server_channel_open_agent_forward(&mut self, channel: ChannelId, _)` — it hands a `ChannelId`, not a `Channel`, so back-channel bytes arrive via the `data()` callback and replies go through `session.handle().data(...)`. That stateful relay needs its own tested pass; for now we only advertise (request-only). My first attempt used a `Channel` arg → E0053; removed.

**UI:** "Forward SSH agent (ssh -A)" checkbox in the host form's Advanced section (edit mode only), saved immediately via `host_update` (same pattern as jump host). `.checkboxRow` style added.

### SSH hardening status: ✅ COMPLETE
TOFU/known_hosts + management UI, SSH-agent auth, restore-on-reload, env passthrough, keepalive, OS auto-detect (+ sidebar icon), last_connected, ProxyJump, agent forwarding. Next major: **Stage 4 — RDP via IronRDP** (see `docs/specs/rdp-session.md`, sticky-modifier focus-sync requirement), or Sync/master-password.



A host can now route through a **bastion** — another saved SSH host used as a jump. Agent-forwarding is the next (last) SSH item; kept separate (russh-heavy).

**Data model:** `Host.jump_host_id: Option<HostId>` (plain nullable TEXT, no FK — a deleted bastion is handled at connect time). Migration **v5→v6** `ALTER TABLE hosts ADD COLUMN jump_host_id TEXT` (chained runner; `CURRENT_SCHEMA_VERSION=6`, v1.sql bumped + column added). Wired through host_store INSERT/UPDATE/SELECT/row_to_host, HostDto/HostFullDto, Host{Create,Update}Request (update guards against self-reference).

**Connect flow (actor):** `SshSpawnParams.jump: Option<JumpParams{hostname,port,host_id,credentials}>`. When set, the actor connects the bastion (`russh::client::connect`, auth via `try_all_auth`), opens `channel_open_direct_tcpip(target,…)`, wraps it `into_stream()`, and runs the target transport over it via `connect_stream` — then proceeds identically (auth/PTY/shell/pump). The bastion `Handle` is kept alive (`_bastion_keepalive`) for the session. Refactor: `ClientHandler.auto_accept` (bastion pins its key silently — no double prompt; target keeps normal interactive TOFU); new helpers `ConnectOutcome` + `drive_target_connect` (drives either connect future while forwarding host-key decisions) + `try_all_auth`. ⚠️ russh-version-sensitive calls flagged: `channel_open_direct_tcpip`, `Channel::into_stream`, `connect_stream`.

**rh-app:** `session_open` resolves the jump host (must exist + be SSH), reveals its creds via new self-contained helper `revealed_creds_for` (mirrors target reveal: passwordless fallback, key→agent→password order). One level only (a bastion's own `jump_host_id` is ignored).

**UI:** "Jump host" combobox in the host form's **Advanced** section (edit mode only), listing other SSH hosts (excludes self); empty = direct. Reuses the `Combobox` (clearable). Saved **immediately** on change via `host_update` (not threaded through the debounced text-field autosave — lower risk). Spec appended to `docs/specs/ssh-session.md`.



Two of the four remaining SSH items (jump-host + agent-forwarding are the next, dedicated pass — they restructure the connect path and are the most russh-fragile, so they're kept separate).

**OS auto-detect (Stage 2.2):** the actor runs a best-effort probe on a *separate* exec channel right after Ready (doesn't touch the PTY): `uname -s; ___RH___; cat /etc/os-release`, parsed by `parse_os_slug` (5 unit tests) → "ubuntu"/"debian"/"macos"/"windows"/"linux"/… Emits a new `SshSessionEvent::DetectedOs { os }` which the `SessionManager` pump **consumes** (not forwarded to the UI) and persists via the new `HostStore::mark_detected_os` (targeted UPDATE, like `mark_connected`). The OS chip in the host header (already built) shows it on the next `host_get`. The **sidebar host icon** now switches to the OS logo too: `HostIcon` maps the slug → a Simple Icons glyph via `react-icons/si` (new dep; rendered MONOCHROME via currentColor — no brand colors, per the design language), fallback to the generic `Server`. To refresh the sidebar live after the first connect, the detect path emits `hosts:changed` (AppHandle plumbed into `SessionManager::register`), which the UI already reloads on. ⚠️ The exec probe (`Channel::exec`) is russh-version-sensitive — flagged in `detect_os`.

**Known Hosts management:** `KnownHostsStore::list()` + `KnownHostEntry` (rh-core), `known_hosts_list`/`known_host_forget` commands, `knownHosts` ipc namespace. Surfaced as a **second tab in the key/credentials dialog** (now titled "Security"/"Безопасность"): tab 1 = Credentials (unchanged), tab 2 = Known hosts — list of `hostname:port · key_type · SHA256:… · trusted-date` with a trash button to forget (next connect re-prompts TOFU). Per-host jump/agent-forward will be host-form fields, NOT here (deliberately kept the footer clean — confirmed UX call).



The ⓘ technical-info popover now shows Created / Updated / **Last connection** / Fingerprint. The opaque ULID `ID` row was removed (debug-only; kept the value out of the user's face). Fingerprint is copy-to-clipboard (`SHA256:<fp>` + key-type hint).

- **`last_connected_at: Option<DateTime<Utc>>` on `Host`** (rh-core), machine-set — never written through create/update. New `HostStore::mark_connected(id, when)` does a targeted `UPDATE hosts SET last_connected_at=?`. The `SessionManager` event pump stamps it once, on the first `Ready` event (so it means *connected*, not *attempted*); `Hub.connected_stamped` guards against repeats.
- **Migration v4 → v5**: additive `ALTER TABLE hosts ADD COLUMN last_connected_at TEXT`. `db.rs` `init_or_migrate` was refactored into a **chained runner** (`MIGRATIONS: &[(from, sql)]` + `has_migration_chain`): any DB with a contiguous path (v2→v3→v4→v5) migrates forward with no data loss; only a gap (pre-v2) drop-recreates. `host_store` INSERT/SELECT/`row_to_host` updated for the column; `v1.sql` carries it + version '5'.
- Exposed as `HostDto.last_connected_at` (RFC 3339 string | null). Frontend shows `formatDate(...)` or "Never"/"Ещё не подключались". Info-label column widened to 104px; RU "Подключение".



## Latest — SSH hardening: TOFU, agent, restore-on-reload, env (DONE, live-verified)

Closes the Stage-2 follow-ups before RDP. **Compiled & live-verified on Windows**: TOFU prompt (unknown/changed/reject + silent on known), SSH-agent auth, restore-on-reload (F5 brings sessions back with scrollback), env passthrough, last-connection stamp, copyable fingerprint. The agent block compiled against russh 0.45 as written.

**known_hosts / TOFU (rh-core + rh-storage + rh-ssh + rh-app):**
- New `KnownHostsStore` trait (`rh-core/store.rs`) + `KnownHostKey { key_type, fingerprint_sha256 }` (OpenSSH SHA256, base64 no-pad). `SqliteKnownHostsStore` (`rh-storage/known_hosts_store.rs`, upsert by `(hostname, port)`, 5 tests). New `known_hosts` table.
- **Migration v3→v4 is incremental, data-preserving** (`db.rs` `CURRENT_SCHEMA_VERSION = 4`): `Some(3)` → `CREATE TABLE known_hosts`; plus a chained `Some(2)` → v3 ALTER then v4 CREATE so a two-versions-behind DB isn't wiped. `v1.sql` updated for fresh installs (known_hosts table + version '4'). **Same rule as before: additive change = incremental ALTER/CREATE path, never a bare v1.sql bump.**
- Actor (`rh-ssh/actor.rs`): `check_server_key` now computes the SHA256 fingerprint, looks up the pin, and — on unknown (when `strict`) or **changed** (always) — emits `HostKeyPrompt { fingerprint_sha256, key_type, changed }`, sets state `host_key_pending`, and **blocks** on a decision. The decision arrives as `SessionCommand::HostKeyDecision(bool)`, forwarded into the handler from the command channel while the connect future is in flight (select loop in `connect_and_pump`). Accept → pin + `Ok(true)`; reject → `Ok(false)` → mapped to `CloseReason::HostKeyRejected` via a `rejected` flag + new `SshError::HostKeyRejected`.
- `session_accept_host_key`/`session_reject_host_key` now send `HostKeyDecision(true/false)` (no longer no-ops / hard close). Frontend prompt surface already existed; added a `changed` flag → red-accented warning banner + `session.hostKey.changedPrompt` string (EN/RU).
- `strict_host_key` comes from `Settings.ssh_known_hosts_strict` (default true). Non-strict auto-pins unknown keys silently but **still prompts on a changed key**.
- **Pinned fingerprint shown in the host technical-info panel** (the ⓘ popover): new `known_host_get` command (resolves host_id → hostname/port → `KnownHostsStore::lookup`) + `hosts.knownHostKey` ipc. The popover's ID and fingerprint rows are now copy-to-clipboard (`CopyableValue` in HostDetail.tsx; `SHA256:<fp>` + key-type hint). Shows "not pinned yet" until first trust.

- **SSH-agent auth (rh-ssh + rh-app):**
- `RevealedCredential::Agent { username }`; `CredentialKind::SshKeyAgent` now produces it (was skipped). Actor `try_auth_agent`: connect to agent (unix `$SSH_AUTH_SOCK` via `AgentClient::connect_env`; **windows** `\\.\pipe\openssh-ssh-agent` named pipe — covers OpenSSH agent and modern Pageant), `request_identities`, then `authenticate_future` per identity. **Best-effort & non-fatal** — any agent failure returns `Ok(false)` so other methods still run. ⚠️ **This is the most russh-version-fragile block** — if it doesn't compile, the fix is local to `try_auth_agent` (and possibly a russh `agent` feature flag); TOFU/restore/env are independent of it.
- Auth order is keys → agent → password.
- **Agent UI (HostDetail credential panel):** the "+ SSH-ключ" picker now has a **"Use SSH agent"** footer → creates/reuses an `ssh_key_agent` credential (no secret) and links it; shows as a `Server`-icon chip with ✕ to unlink. Key and agent share the one "method slot" (mutually exclusive in the UI; password is always its own field). This is the only way to create an agent credential — before, `ssh_key_agent` was reachable only via the IPC console.

**Restore-on-reload (rh-app `SessionManager` rewrite + frontend):**
- The Rust process survives a webview reload, so actors stay alive. `SessionManager` now holds a per-session `Hub` { tx_cmd, abort, meta, state, **256 KB output ring**, current `Channel` sink }. `register` absorbs the old event-bridge: it pumps actor events → records into the ring + forwards to the live channel. New `list()` (→ `session_list` returns real `SessionSummaryDto[]`) and `reattach(id, channel)` (→ new `session_reattach` command) which swaps the sink and replays buffered scrollback + current state.
- Frontend: `restoreSessions()` store action (called from `AppShell` mount) calls `session_list`, rebuilds one tab per live session, and `session_reattach`es a fresh channel; dead/closed sessions are skipped, and a reattach miss drops the stale tab. **Split layouts are NOT reconstructed** — each restored session comes back as its own tab (flat). **Edge:** a session reloaded mid host-key-prompt restores without the prompt object (fingerprint isn't buffered) — rare; user reconnects.

**Env + keepalive (rh-ssh + rh-app):**
- `SshSpawnParams.env_vars: Vec<(String,String)>` from `host.env_vars`; actor sends `channel.set_env(false, k, v)` before the shell (servers honor only their `AcceptEnv`; want_reply=false so an unaccepted var can't fail the channel).
- Keepalive interval now from `Settings.ssh_keepalive_interval_secs` (0 = disabled) instead of a hardcoded default.

**Build/run after pulling this:** `cargo tauri dev` (Rust + migration changed). The v3→v4 migration is additive — existing hosts/creds/settings are preserved; only a brand-new `known_hosts` table is added.



UX pass on top of the auth work below. All frontend; verified `tsc --noEmit` + `vite build`.

**Credential panel (HostDetail.tsx — the fragile file):**
- **Saved password is locked by default** (read-only, muted color `.pwMuted`). Eye 👁 reveals the live keychain secret read-only (selectable/copyable, 10s auto-hide). Pencil ✏️ reveals it into an **editable plaintext** field (via `onPasswordRevealed`, which seeds the value as the committed baseline so no save fires).
- **No ✕ on the password.** Removal is deliberate: reveal with the pencil, clear the text, save → `credential_unlink_host`. Guard: an empty field only deletes when it differs from a non-empty committed baseline (so merely opening a host and saving never nukes the password). `saveAction` password block: changed→ empty+pwCred = unlink, non-empty = rotate/create.
- **Re-lock on click-outside** the password row (`pwRowRef` + document mousedown) — and on linked-cred change. So the resting state is always "locked + muted"; clicking Connect, another field, etc. re-locks and hides the reveal.
- **Connect commits the field:** `handleConnect` clears the typed `inlinePassword/privateKey/passphrase` + committed refs after the flush-save. Without this, a locally-typed password lingered and the eye showed the stale typed value (not the live keychain secret) until you navigated away — the "password shows 111 after re-auth changed it to 222" bug. Now the field locks to the saved cred and the eye always reveals live.
- **SSH key add:** "+ SSH-ключ" opens a dropdown of existing `ssh_key` creds + an **"Add new key…"** footer → `AddKeyModal` (paste / import .ppk·PEM / passphrase). Key chip still uses pencil→✕ (keys aren't text, so the 2-step delete stays). `SavedCredentialPicker` and `AddKeyModal` are **exported** from HostDetail.tsx for reuse on the re-auth screen.

**Inline re-auth on auth failure (SessionView.tsx):**
- Detect: `isDead && message.toLowerCase().includes("auth")` — **note auth failure ends the session in state `closed`, not `failed`**, so don't gate on `failed`.
- Layout: password input + a stretched **SSH-key button** (green `--color-ssh` icon) on one row (`.reauthPwRow` is the dropdown anchor → full-width picker), full-width **"Подключиться и сохранить"** below. No hint text, no "Edit" button on the auth screen (Edit stays on the non-auth closed/failed EmptyState).
- Actions all **save to the host** then reconnect: `connectWithPassword` rotates the existing password cred or creates+links a new one; `linkAndReconnect(credId)` links a picked key; `addKeyAndReconnect` creates+links a pasted/imported key. Each fetches the full host (`hosts.get`) for `credential_ids`, mutates via `credApi`, then `close + open(fresh)`.

**Other:**
- **Passwordless fallback (sessions.rs):** if a host has **no linked credential but a username**, try a single empty-password attempt instead of erroring "host has no credential".
- **Split-tab label (TabBar.tsx):** a tab with >1 pane shows `t("tab.split")` ("Сплит") instead of one pane's title; the ⊞ count badge stays.



## Latest — SSH auth, multi-method credentials, per-host username (DONE, live-verified)

Big batch after Stage 2 part 2. All compiled on the user's machine and verified by real connections (key, .ppk, password, passwordless).

**SSH auth (rh-ssh):**
- Public-key auth: `russh::keys::decode_secret_key` for OpenSSH/PEM; `authenticate_publickey` (russh 0.45 `bool` API — note in `actor.rs` for 0.46+ `PrivateKeyWithHashAlg` + `.success()`).
- **Native PuTTY .ppk → OpenSSH** converter: `rh-ssh/src/ppk.rs` (pure Rust, PPK v2 HMAC-SHA1 + v3 Argon2id, aes256-cbc/none, rsa/dss/ecdsa/ed25519). Crypto crates added to `rh-ssh/Cargo.toml` (sha1/sha2/hmac/aes/cbc/cipher/argon2/base64) with a comment justifying the deviation from "aws-lc-rs only". `actor.rs` detects `is_ppk` → converts → decodes with no passphrase.
- Empty/passwordless: missing keychain secret → empty password; and (see sessions) a host with a username but **no** credential tries an empty password.

**Multi-method per host (the actor tries each, keys → password):**
- `SshSpawnParams.credential` → `credentials: Vec<RevealedCredential>`. `actor.rs` loops `try_auth` over them; a bad/undecodable key is skipped (`Ok(false)`) so a working password still gets in; auth fails only if all are rejected.
- `CredentialStore::credentials_for_host(host_id)` (new trait method + JOIN on `host_credentials`, default first).
- `api/sessions.rs`: with no `credential_id` override, gathers **all** linked creds (keys first); if none linked but host has a username → single empty-password attempt; else "host has no credential".
- Frontend `open()` sends `credential_id: null` (was the default id) so the backend offers every method — passing a specific id would restrict to one.

**Per-host username (data-model change, NON-DESTRUCTIVE migration):**
- `username` moved from the **credential** to the **host** (one key shared across hosts with different logins). `Host.username`, `HostFullDto.username`, host create/update DTOs.
- Session resolves `host.username` else falls back to `cred.username` (back-compat for pre-migration hosts).
- **Migration v2→v3 is incremental, data-preserving:** `db.rs` `CURRENT_SCHEMA_VERSION = 3`; `Some(2)` → `ALTER TABLE hosts ADD COLUMN username TEXT NOT NULL DEFAULT ''` + bump `schema_meta` (no drop). Other version mismatches still drop-recreate (alpha mode). `v1.sql` updated for fresh installs (hosts.username + version '3'). New `InitOutcome::Migrated`. **Do not bump the version with a plain edit to v1.sql for an additive change — add an incremental ALTER path or you wipe user data.**
- `HostFullDto.credential_ids: Vec<CredentialId>` (populated by `host_get` via `credentials_for_host`) so the UI can render all linked methods.
- Credential username validation relaxed: **empty username is allowed** for all kinds (login lives on the host now). Inline-created creds pass `username: ""`.

**Credential UX (HostDetail.tsx, the fragile file):**
- Password field is **always visible**; a linked SSH key shows as a chip; each linked method has a ✕ to **unlink** (`credential_unlink_host`). Password/key handled **independently** in save (create+link if absent, rotate if present, change-gated to avoid duplicate creates).
- **New add-key flow:** "+ SSH-ключ" → dropdown of existing keys + **"Add new key…"** footer → **modal** (`AddKeyModal`) to paste or import (.ppk/PEM) + passphrase; on confirm creates the cred in the keychain and applies immediately (edit → linkHost, draft → remembered, linked on promotion). Inline key textarea removed.
- **Connect flushes pending save first** (`handleConnect` cancels debounce, awaits `saveAction`, re-fetches, then opens) so a just-typed password/key is persisted before the session opens — fixes spurious "host has no credential".
- Compact form: Name + Group on one row; Tags / Startup command / Env vars / Notes under an **"Advanced/Дополнительно"** spoiler. Password field full-width with trailing controls (timer/eye/✕) overlaid right.
- Key creds named by imported **file name**; re-import renames. "Use existing" lists **keys only** (passwords stay private to their host).

**Session error screen:** "Edit/Изменить" button next to "Reconnect" → jumps to the Vault tab and selects the host (`setActiveTab(null)` + `selectHost`).



## UX overhaul — tabbed shell (part 1, DONE, verified tsc+vite)

Replaced the permanent left-sidebar shell with a Termius/Windows-Terminal-style tab bar.
- `layout/TabBar.tsx` — top bar: pinned **Vault** tab (`nav.vault`, host manager, not closable, `activeSessionKey === null`) + one tab per session + a "+" button (currently returns to Vault; dedicated launcher is the next step).
- `layout/HomeView.tsx` — Vault content = `CommandBar` + `Sidebar` + `HostDetail` (the former shell body, now the home tab).
- `layout/AppShell.tsx` — `TabBar` over a stage that keeps **every** tab mounted (HomeView + all SessionViews) and toggles visibility via inline `display`, so scrollback, form drafts, and focus all survive tab switches.
- Removed `layout/WorkArea.tsx` and `session/SessionTabs.tsx` (folded into AppShell/TabBar).

**UX overhaul follow-ups (user's list):** (a) "+" launcher — search + recent/host list to start a session (screen ref: Termius new-tab); (b) terminal appearance — bundle a default mono font + make font configurable; (c) terminal theme presets (Dracula/Nord/Solarized/Monokai/Pro…) with a full 16-color ANSI palette + picker + persistence. ANSI output colors already render (server-driven); no client-side keyword highlighting.

## Stage 2 — in progress (SSH sessions)

**Part 1 — frontend + IPC contract (DONE, verified tsc+vite):**
- `ui/src/components/session/{Terminal,SessionView,SessionTabs}.tsx` + `layout/WorkArea.tsx`. The work area now shows a tab strip (Host editor + one tab per session) and swaps between `HostDetail` and a live `SessionView`. AppShell body = Sidebar + WorkArea.
- Terminal is xterm.js (`@xterm/xterm` + `addon-fit`). Output flows via a module-level registry in the sessions store (buffered until the terminal mounts, so switching tabs never loses output); keystrokes → `session_send_input`, resize → `session_resize`.
- `useSessionsStore` (in `store/index.ts`): tabs keyed by a stable local `key` (set up before the backend returns the real `sessionId`, avoiding event races), state machine, host-key prompt, reconnect.
- `lib/ipc.ts` `sessions.*` uses a Tauri **Channel** for `SshSessionEvent` (`state_changed/data/auth_failed/host_key_prompt/error/closed`, `CloseReason`) per `tauri-api.md`. Types in `lib/types.ts`.
- Connect button enabled for **saved SSH hosts** (draft/RDP show a reason tooltip). Backend is still the stub → connecting shows "failed: not implemented" gracefully. No regression to the running app.

**Part 2 — russh backend (DONE — live SSH connection verified against AM-NL):**
- Compiled clean on russh 0.45 after one fix (the `Handler` trait there is `#[async_trait]`, so `ClientHandler` is annotated `#[async_trait]`). The `select!` channel-borrow and PTY/auth signatures worked as written.
- `rh-ssh`: `russh` client actor. `lib.rs` (public types: `SessionState`, `CloseReason`, `SshSessionEvent`, `SessionCommand`, `SshOpenOptions`, `RevealedCredential`, `SshSpawnParams`, `SshSessionHandle`, `spawn_session`), `error.rs` (`SshError` + `into_close_reason`), `actor.rs` (russh connect → password auth → PTY shell → select! pump). Events flow out via `mpsc::UnboundedSender<SshSessionEvent>` (crate stays tauri-free).
- `rh-app`: `session.rs` `SessionManager` (registry + per-session supervisor that evicts on exit); `api/sessions.rs` real handlers (`session_open` reveals credential, bridges mpsc→Tauri `Channel`, spawns actor, registers; close/send_input/resize/accept/reject); DTOs (`SessionOpenResponse`, `SessionInputRequest`, `SessionResizeRequest`, `SessionAcceptHostKeyRequest`); `AppState.sessions`; handlers registered in `main.rs`.
- **v1 simplifications (to land a working connect first):** password auth only (SSH-key/agent → friendly not-implemented); host key auto-accepted TOFU (no `known_hosts` pinning, no interactive prompt blocking inside the russh handler — UI prompt surface stays dormant); no keepalive.
- ✅ **COMPILED & LIVE-VERIFIED.** russh pinned `0.45`. Auth has since grown to multi-method (key/.ppk/password/passwordless) — see the "Latest" section at the top. Keep a known-good zip as rollback when touching the backend.

**Part 2 follow-ups:** ✅ all done — known_hosts pinning + interactive TOFU, SSH-key auth, SSH-agent auth, keepalive, `session_list` restore-on-reload, env passthrough (see the SSH-hardening "Latest" section at top). Next is Stage 4 (RDP).

---

This document is the single source of truth for picking up RemoteHub development. Read it first when starting a new chat or after a long break. When a stage closes, **update this file** before packaging the archive.

---

## What RemoteHub is

A cross-platform desktop client for remote sessions (SSH + RDP). Windows-first; architecturally ready for macOS and Linux. The target is "Termius with RDP" — modern UI, no clutter, live-save everywhere, OS keychain for secrets.

- **Shell**: Tauri 2
- **Backend**: Rust stable + Tokio, russh (Stage 2), IronRDP (Stage 4)
- **Frontend**: React 18 + TypeScript strict + Vite, Zustand state, lucide-react icons
- **Storage**: sqlx + SQLite, keyring-rs 3.6 (apple-native / windows-native / sync-secret-service)
- **Crypto**: aws-lc-rs

User environment: Windows 11 (kolen), PowerShell, Rust 1.95, Node v24.15.0, pnpm v11.4.0, Tauri CLI 2.11.2. Real Windows Credential Manager records with `service="RemoteHub"`. DB at `%APPDATA%\RemoteHub\remotehub.db`.

---

## Workspace layout

```
remotehub/
├── crates/
│   ├── rh-core/        # types, errors, IDs (41 tests)
│   ├── rh-storage/     # SQLite + keychain (37 tests)
│   ├── rh-ssh/         # placeholder for Stage 2
│   ├── rh-rdp/         # placeholder for Stage 4
│   └── rh-app/         # Tauri binary, IPC handlers (31 tests)
├── ui/
│   ├── src/
│   │   ├── components/
│   │   │   ├── host/HostDetail.tsx        # the big one (~1100 lines) — main pane
│   │   │   ├── host/SaveStatusIndicator.tsx
│   │   │   ├── sidebar/Sidebar.tsx
│   │   │   ├── layout/{AppShell,DialogHost}.tsx
│   │   │   ├── dialog/{ConfirmDialog,CredentialFormDialog,CredentialsListDialog,GroupFormDialog}.tsx
│   │   │   └── ui/{Button,Combobox,Dialog,EmptyState,Input,ProtocolBadge,TextField}.tsx
│   │   ├── i18n/{en,ru,index}.tsx         # custom i18n, no react-i18next
│   │   ├── lib/{ipc,types,useDebouncedCallback}.ts
│   │   ├── store/index.ts                 # zustand: hosts, groups, credentials, ui
│   │   └── styles/tokens.css
│   ├── package.json, vite.config.ts, tsconfig.json
│   └── vite-env.d.ts                      # CSS modules types
└── docs/
    ├── ROADMAP.md
    ├── STATE.md                            # this file
    └── specs/
        ├── system-overview.md
        ├── data-model.md
        ├── tauri-api.md
        ├── session-protocol.md
        ├── ssh-session.md
        └── rdp-session.md                  # contains sticky-modifier focus-sync requirement
```

---

## Stage status

| Stage | Title | Status |
|---|---|---|
| 1.1 | Rust workspace skeleton + types | ✅ Done |
| 1.2 | Storage layer (SQLite + keychain) | ✅ Done |
| 1.3 | Tauri IPC handlers | ✅ Done |
| 1.4 | UI scaffolding + IPC client | ✅ Done |
| 1.5 | Initial UI (sidebar + detail + dialogs) | ✅ Done |
| 1.5.1 | UX pass #1: i18n, short-view, reveal, duplicate, group actions | ✅ Done |
| 1.5.2 | UX pass #2: live-save everywhere, draft mode, credentials redesign | ✅ Done |
| 1.7 | Visual pass: Inter + JetBrains Mono fonts, lighter dark theme, rounded sidebar items, HostIcon slot | ✅ Done |
| 1.6 | Settings dialog + language toggle UI | ✅ Done |
| 1.8 | Schema extensions: display_name, startup_command, env_vars, detected_os | ✅ Done |
| 1.9 | Command bar (top, search + user@host:port parser) | ✅ Done |
| 1.10 | Import .rdp files | ⬜ Future |
| 1.12 | Export/Import JSON (our format) | ⬜ Future |
| 2.x | SSH session actors (russh) | ✅ Done |
| 2.2 | OS auto-detect after connect (exec probe → detected_os) | ✅ Done (pending compile) |
| 2.3 | SSH hardening: ProxyJump (jump host) + agent forwarding (request-only; serving side TODO) | ✅ Done (compiles) |
| QA-2 | **Manual end-to-end QA of SSH hardening on real hosts** — agent-forward (`ssh -A` chain), ProxyJump through a bastion, OS-detect icon, known-hosts forget/re-pin, TOFU change warning. None of 2.2/2.3 verified by the user yet; agent-forward client callback name unconfirmed. | ⬜ TODO |
| 2.4 | Agent-forward **serving** side (ChannelId relay via data() + session.handle()) | ⬜ TODO |
| 4.0 | RDP foundation: contract types + viewport (focus/modifier-sync) | ✅ Done (FE verified; rh-rdp = types) |
| 4.0b | RDP trusted-cert store (TOFU) + Security-dialog tab | ✅ Done (FE verified) |
| 4.1a | RDP actor shell (spawn + command channel + lifecycle) | ✅ Done |
| 4.1b | RDP connectivity spike (isolated IronRDP → PNG) | ✅ Done (PROVEN on real server) |
| 4.1c | Read-only live desktop in app (2b-1: actor+rh-app+FE) | ✅ Done (FE verified; Rust pending Win compile) |
| 4.1d | RDP mouse input (2b-2: click/scroll, spike-proven) | ✅ Done (FE verified; Rust pending Win compile) |
| 4.1e | RDP keyboard input (2b-2b: scancode map + modifier-sync) | ⬜ Next |
| 5.x | Personal/Team Vault via S3 — cloud sync, identity, e2e crypto | ⬜ Future |

~119 Rust tests (run `cargo test` on Windows to confirm; +10 in Stage 1.8). Vite + tsc strict build green. **DB schema is v2** as of Stage 1.8 — opening an old v1 DB drops & recreates it (alpha policy, data loss expected).

---

## Stage 1.9 — closeout (command bar)

Frontend-only. New full-width strip at the top of the window (`AppShell` is now a column: CommandBar over a `.body` flex row of Sidebar + HostDetail).

`ui/src/components/layout/CommandBar.{tsx,module.css}` — the **single** search/command surface:
- owns `uiStore.searchQuery`, so typing live-filters the sidebar tree. **The sidebar's own search box was removed** (one search, not two — the redundant double-search looked wrong).
- parses `[ssh|rdp]://[user@]host[:port]`; when the text is an explicit address with no exact host match, a slim "New host" suggestion drops under the bar. Activating it (click or Enter) opens a pre-filled draft (`startDraft` + `updateDraft`).
- Enter with no suggestion opens the sole match if exactly one host matches. Ctrl/Cmd+K focuses; Esc clears. `Ctrl K` hint shown (Windows-first).

No backend changes — real connect is still Stage 2, so the "connect" path is quick host creation for now. i18n: `command.*` keys in both locales (`sidebar.searchPlaceholder` now unused but left in place).

---



Added four columns to `hosts` and threaded them through every layer. Schema bumped **v1 → v2**; alpha drop-recreate policy means existing DBs are wiped on first open with this build.

New fields:
- **`display_name TEXT` (nullable)** — explicit user label. **Retires the `name == hostname` auto-label heuristic** in `HostDetail.buildFormState`. The form's Label input now binds straight to `display_name`; unset = `null` and the input shows the hostname as placeholder. The frontend still sends `name` (= label‖hostname) as the canonical sort/search key *and* `display_name` separately; the backend stores both verbatim (no server-side derivation). Blank labels are normalized to NULL in `host_create`/`host_update`.
- **`startup_command TEXT` (nullable)** — command run on SSH connect. UI field is SSH-only (hidden for RDP and forced to `null` on save when protocol≠ssh). Consumed by the Stage 2 session actor (not yet wired).
- **`env_vars` → `env_vars_json TEXT NOT NULL DEFAULT '[]'`** — JSON array of `{key,value}` (order-preserving), mirroring the `tags_json` pattern. Domain type `EnvVar` in `rh-core`. UI: a `KEY=VALUE`-per-line textarea; `parseEnv`/`formatEnv` in HostDetail. Full-replace list on update (empty array clears).
- **`detected_os TEXT` (nullable)** — machine-set OS slug (e.g. `ubuntu`). **Not accepted on create**; persisted through the normal `host_update` path so the **Stage 2.2** detection routine can set it. Shown read-only as a small chip on the detail header when present; will drive the sidebar HostIcon in 2.2.

DTO placement: `display_name` + `detected_os` live on the lean `HostDto` (sidebar needs them); `startup_command` + `env_vars` only on `HostFullDto` (`host_get`).

Validators (rh-app): display_name ≤256, startup_command ≤4096, env_vars ≤64 entries / key ≤256 / value ≤4096, non-empty + unique keys, no NUL bytes.

Files touched: `rh-core/{types,lib}.rs`, `rh-storage/{db.rs, migrations/v1.sql, host_store.rs, tests/integration.rs}`, `rh-app/api/{dto,hosts}.rs`, `ui/src/lib/types.ts`, `ui/src/store/index.ts` (HostDraft gained `startupCommand`/`envVars` for faithful duplicate + dirty-check), `ui/src/components/host/{HostDetail.tsx,HostDetail.module.css}`, `ui/src/i18n/{en,ru}.ts`.

Side note: fixed three **pre-existing** `tsc` errors in the legacy `dialog/HostFormDialog.tsx` (modal form, not in the live render path) — it referenced i18n keys `credentialUseExisting` / `credentialUseInline` / `credentialSelectNone` that were never added. Added them to both locales so the strict gate is green again. Consider deleting that dead modal in a future cleanup stage.

---

## Stage 1.6 — pickup point (ARCHIVED — 1.6 is ✅ done)

What got done before pausing:

**Rust side — COMPLETE**:
- Added `Language` enum (`en` / `ru`) to `rh-core/src/settings.rs` with `Default = En`.
- Added three fields to `Settings` struct: `language: Language`, `default_ssh_port: u16` (= 22), `default_rdp_port: u16` (= 3389). Defaults wired up.
- Added matching key constants: `keys::LANGUAGE`, `keys::DEFAULT_SSH_PORT`, `keys::DEFAULT_RDP_PORT`.
- Updated `rh-storage/src/settings_store.rs` `load()` to read the three new keys, and `is_known_key()` to accept them.
- Added `Language` to `rh-core/src/lib.rs` re-exports.
- Added `language_serde_lowercase` test, updated `default_settings_match_spec` and `keys_are_unique` tests.

**UI side — PARTIAL**:
- Added `Language = "en" | "ru"` and the three new fields to `Settings` interface in `ui/src/lib/types.ts`.
- Nothing else done on UI yet.

**What remains for Stage 1.6** (pick up here):

1. **Settings dialog component** (~400 lines). Layout: sidebar (200px) + content (rest). Sections:
   - **Профиль** — empty state "Stage 5"
   - **Внешний вид** — language toggle (EN/RU), theme (System/Light/Dark)
   - **Подключения** — Default SSH port (22), Default RDP port (3389)
   - **Терминал** — empty state "Stage 2"
   - **Импорт/Экспорт** — empty state with roadmap pointers
   - **О программе** — version, repo link
2. **Settings store** (Zustand). Methods: `load()`, `update(patch)`, `subscribe()` (for events from Rust). Initial load on app start.
3. **Wire the language switcher**: when user picks RU/EN, also call `setLocale()` on the I18nProvider. They should track each other.
4. **Enable gear button** in sidebar footer: opens settings dialog.
5. **i18n keys** for the dialog text — sections, labels, helpers.
6. **Subscribe to `settings:changed` event** from Rust so settings dialog reflects changes in real-time (rare but cheap).
7. Verify with `tsc --noEmit && vite build`.

**Files to touch when resuming**:
- `ui/src/store/index.ts` — add `useSettingsStore`
- `ui/src/lib/ipc.ts` — already has `settings.getAll()` and `settings.update()`; may need a `subscribe` wrapper
- `ui/src/components/settings/SettingsDialog.tsx` (new) + `.module.css` (new)
- `ui/src/components/settings/sections/*.tsx` (one per tab) — keep sections tiny and isolated so adding new ones later is mechanical
- `ui/src/components/sidebar/Sidebar.tsx` — un-disable the gear button, wire it to `setDialog({ kind: "settings" })`
- `ui/src/store/index.ts` — add `"settings"` to `DialogKind` union
- `ui/src/components/layout/DialogHost.tsx` — handle the new `kind`
- `ui/src/i18n/{en,ru}.ts` — add keys for `settings.*`

**Settings dialog should respect existing patterns**:
- Live save (no Save button)
- Save status indicator in the header (same `SaveStatusIndicator` component)
- Discriminated union for tab selection (`type Tab = "profile" | "appearance" | ...`)
- All visible strings via `t("key")`
- CSS modules using existing tokens

---

## The core UX model (Stage 1.5.2 — important)

This is the Termius-style mental model the user wants. Internalize it before touching anything in HostDetail.

### Selection IS edit

Clicking a host in the sidebar opens it as an **editable form** in the right pane. There is no "Edit" button. Every field is editable in place. Changes auto-save with a debounce.

### Live save with status indicator

- Every keystroke triggers `setSaveStatus({kind: "pending"})`.
- Debounce timer fires (`400ms` for fields, `1000ms` for notes).
- `saveAction` runs: `setSaveStatus({kind: "saving"})`.
- On success: `flashSaved()` shows a green check for 1.5s, then back to `idle`.
- On error: `setSaveStatus({kind: "error", message})`. Sticky until the next successful save.

The indicator is a small icon in the header next to `ⓘ`. No red banner. No popup. Just a spinner / check / X icon.

**Implementation:** `SaveStatusIndicator.tsx` + `setSaveStatus` calls scattered through `saveAction`, `linkCredential`, `createGroup`.

### Draft mode for new hosts

`+ Host` does **not** open a dialog. It calls `startDraft(groupId)` which puts a `HostDraft` object into `UiStore.draft`. HostDetail renders the form for that draft. The sidebar shows a "Черновик / Draft" row in italics above the tree.

Once the user fills `hostname` (and only then), the draft is **promoted**: `hostsApi.create` runs, then the draft is cleared and the new host becomes selected. This must happen **without unmounting HostForm** or the input loses focus — see "Focus continuity" below.

### Discard changes confirm

Triggered only when the user has a dirty draft (some field non-empty) but `hostname` is still empty (so it can't be auto-promoted), and they try to navigate away. Shows a confirm dialog with "Discard changes" / "Cancel" actions.

If `hostname` is filled, navigation is silent — the draft is just auto-promoted and the navigation happens against the new real host.

### Hostname validation (silent)

`isValidHostname(s)` in HostDetail.tsx checks against:
- DNS hostname (RFC 1123 labels)
- IPv4 dotted quad
- IPv6 (loose: ≥2 colons)

If invalid, `saveAction` returns early with `idle` status — **no error shown**. The user is just in the middle of typing. As soon as the address parses, save proceeds.

### Credentials in HostForm

Two always-editable inputs: Username + Password. Below them, a button `+ Использовать имеющиеся (SSH-ключ, сохранённый логин...)` (disabled when there are no saved credentials).

- If the host has no `default_credential_id` and both fields are filled — `saveAction` creates a new credential and links it.
- If the host has a linked credential — username is loaded from it (password stays masked, eye button toggles visibility of what the user typed). Changing username → `credApi.update`. Typing a new password → `credApi.rotateSecret`.
- A small chip below shows "Связано с: <name>" when a credential is linked.

**Key invariants:**
1. Never create a credential with one of {username, password} empty. The OS keychain rejects empty secrets and the user would see a "secret must not be empty" error — which is annoying because they just haven't finished typing.
2. **Don't clear the password input after rotateSecret.** The user wants to see their masked dots as proof the password is saved, and to be able to click the eye to verify it. Diff-checking against `committedPasswordRef` prevents re-rotation on every subsequent keystroke.
3. **`committedUsernameRef` / `committedPasswordRef`** track what we've already written to the keychain. `saveAction` only calls `credApi.update` or `credApi.rotateSecret` when the form values differ. Refs are kept in sync at: initial load via the linkedCred-change effect, after successful save, and after a saved credential is picked. Without these refs, every keystroke would trigger another no-op `rotateSecret` call.

### Focus continuity at draft → host promotion

This is the trickiest part of Stage 1.5.2 and **has been the source of multiple bug reports**. The user types "192" in Address. The promotion happens. The user expects to keep typing "192.168.0.12" without re-clicking the input.

For this to work, **HostForm must stay mounted as the same React node** across the promotion. Implementation:

1. `HostForm.saveAction` calls `props.onDraftPromoted(fresh)` instead of `clearDraft + selectHost`.
2. In `HostDetail`, `onDraftPromoted` does `setEditingHost(fresh)` AND `setPromotedId(fresh.id)`.
3. `HostDetail`'s render priority: if `promotedId && editingHost.id === promotedId` → render edit mode immediately, regardless of UiStore. This is the linchpin — it ensures HostForm sees a continuous `host` prop (id flips from `__draft__` to a real id) without ever rendering a different branch.
4. `HostForm`'s `useEffect([props.host.id])` detects the flip from `__draft__` to a real id and **does not** reset form state (`prevHostIdRef`).
5. A `useEffect` in HostDetail then syncs UiStore (`clearDraft + selectHost`) **after** the render with the real host commits.

If you change anything in this flow, test it: `+ Host`, type "192.168.0.12" in one burst, ensure cursor stays in the input.

### Race condition handling

The user can type faster than the create call returns. `promotingRef` blocks concurrent promotions; `pendingDuringPromote` ref captures the last state during a promotion. After `hostsApi.create` returns, a `while (true)` loop applies any pending state via `hostsApi.update` on the new host id before handing off. This prevents losing trailing keystrokes.

**TS quirk:** TS 5.x narrows the ref-derived local to `never` inside the loop after the reassignment. The code uses `const p = pending as FormState;` as a workaround — there's a comment explaining it.

---

## Live invariants

These hold across the whole app. Don't break them.

1. **No raw `invoke()` calls in components.** Everything goes through `lib/ipc.ts`.
2. **Secrets never logged.** `SecretValue` has `zeroize-on-drop`; `#[instrument]` calls have `skip` for secrets.
3. **Coarse mutations.** After any CRUD, reload the full relevant collection. No optimistic patches in stores.
4. **CSS variables in `tokens.css`.** No hex codes in component CSS. Single accent color `#4c8eff`. SSH = green, RDP = blue.
5. **Hairlines, no shadows.** Borders only via `--color-border`; one shadow allowed (popover dropdowns), and that's the maximum.
6. **Discriminated unions for dialogs.** `DialogKind` in `store/index.ts` is the source of truth for what dialogs exist.
7. **ULID newtypes.** `HostId`, `GroupId`, `CredentialId`, `SessionId` — never `String`.
8. **PATCH semantics.** Rust DTOs use `Option<Option<T>>` with custom `deserialize_optional_optional` to distinguish "not in request" from "set to null".

---

## Build & run

User-side (Windows):
```powershell
cd C:\remotehub\ui
pnpm install              # one-time after distributing a fresh archive
cd ..
cargo tauri dev           # starts vite + rust binary
```

Sandbox (Claude side) build verification:
```bash
cd /home/claude/remotehub/ui
npm install --silent
tsc --noEmit              # strict + noUnusedLocals/Parameters
npx vite build            # production bundle test
```

Both must pass green before packaging.

---

## Packaging the archive

```bash
cd /home/claude/remotehub/ui && rm -rf node_modules dist package-lock.json
cd /home/claude
rm -f /mnt/user-data/outputs/remotehub.zip
zip -r -q /mnt/user-data/outputs/remotehub.zip remotehub/ \
  -x '*.DS_Store' '*/target/*' '*/node_modules/*' '*/dist/*'
```

The user unpacks into `C:\remotehub` (overwriting), then runs the commands above.

---

## What's coming in Stage 1.6 (next)

**Settings dialog.** Opens from the gear icon in the sidebar footer (currently disabled with a "coming soon" tooltip).

Contents (per `tauri-api.md` spec):
- Language toggle (EN / RU). Currently switches via `localStorage` only; should persist to backend `Settings.language`.
- Theme (system / light / dark).
- Possibly Connect timeout default, log level, telemetry opt-in.

Wiring:
- `settings_get_all` / `settings_update` IPC commands already exist (placeholders in rh-app).
- Add real Settings store/persistence in rh-storage.
- UI dialog reads `useSettingsStore`, writes via `settings.update`.
- Subscribe to `settings_changed` event to react if changed from another window.

**Tags combobox.** Reuse the `<Combobox>` primitive from `ui/src/components/ui/Combobox.tsx`. Currently tags are a comma-separated input — replace with a multi-pill input with combobox-style suggestions from existing tags.

**Possible quick win — `display_name` column in DB.** Right now we use a heuristic ("if `name === hostname`, treat label as auto-fill") to decide whether to show the label input as empty with a placeholder. A cleaner solution is a nullable `display_name` column in the hosts table, which would require a migration but no schema rewrite. Hold off unless the heuristic causes real problems.

---

## Anti-patterns to avoid

These have come up in development and been pushed back. Don't reintroduce them.

1. **Modal dialogs for editing.** The user explicitly does not want them. Inline editing only. Confirm dialogs are OK for destructive actions (delete).
2. **Card-style read-only credential view.** The user wanted always-editable inputs, not a "linked credential card" with separate edit affordance.
3. **Red error banner.** Errors go in the status indicator with a tooltip. No banners. No popups for fixable errors.
4. **Auto-creating credentials with one field filled.** Wait for both username AND password before any `credApi.create`.
5. **Showing errors for in-progress input.** If the user types "192" and hostname is currently invalid (too short for an IP), just don't save and show idle — not error.
6. **Save buttons.** Termius doesn't have them. We don't either.
7. **Re-using host id as React `key` on form components.** Causes remount on promotion → focus loss.
8. **TS narrowing through `useRef.current`.** Use `as FormState` cast after explicit null check.

---

## i18n notes

Custom implementation in `ui/src/i18n/`. Two locales: `en` (source of truth) and `ru`. The `t(key, vars?)` helper does template interpolation with `{name}` placeholders.

Adding a new string:
1. Add the key to `en.ts` with the English text.
2. Add the same key to `ru.ts` with Russian. If missing, falls back to English silently.
3. Use `const { t } = useT();` and `t("your.key")` or `t("your.key", { name: "value" })` in the component.

Locale auto-detected from `navigator.language` on first load, persisted to `localStorage["remotehub.locale"]`. Stage 1.6 will route this through Settings/backend.

Dates: `formatDate(rfc3339)` from `useT()` — RU = `27.05.2026, 21:15`, EN = `27 May 2026, 21:15`.

---

## Common pitfalls / known gotchas

- **`pnpm install` from project root fails** — `package.json` is in `ui/`, not root. Use `cd ui && pnpm install`. Or rely on `cargo tauri dev` which has `beforeDevCommand` set but doesn't run install.
- **Rust changes require restart of `cargo tauri dev`**. Vite hot-reloads UI changes.
- **TS strict + noUncheckedIndexedAccess.** Be careful with array indexing; use guards.
- **CSS modules need explicit class composition** — `${styles.foo} ${styles.bar}` not nested.
- **lucide-react icons don't accept `title` prop.** Wrap in `<span title="...">`.
- **Zustand's `s` param has no inferred type without proper store types** — but inside the project this works because stores are well-typed; outside (e.g. sandbox without `node_modules`), tsc reports errors. Don't panic.

---

## Decision log

Architectural choices that aren't obvious from the code. Captured here so they don't get re-litigated.

1. **No react-i18next.** Two locales, no plural/genus complexity worth the dep. ~60 lines custom solves it.
2. **No state-management lib beyond Zustand.** Redux would be overkill. Zustand stores in `store/index.ts` cover hosts, groups, credentials, ui (with draft).
3. **`AppState` uses `Arc<dyn HostStore + Send + Sync>`** in Rust for testability — handlers don't see concrete sqlx types.
4. **`keychain-first` create pattern in storage.** Write secret to OS keychain first; if DB insert fails, clean up the orphaned keychain entry. Tested.
5. **`ON DELETE SET NULL` for `hosts.group_id`.** Deleting a group moves hosts to Ungrouped, not deletes them. (User-requested behavior.)
6. **CSS modules over Tailwind.** Tokens in CSS variables; per-component `.module.css` files. No utility-first; explicit names.
7. **No optimistic updates.** Re-fetch after CRUD. Simpler reasoning model; latency is fine for desktop local SQLite.
8. **Rust storage tests with real SQLite + real Windows Credential Manager.** No mocks at this layer. Tests do real I/O and clean up. CI runs on the user's Windows machine.
9. **Frontend tests deferred.** Stage 1.x prioritized end-to-end smoke tests (open the app, do the thing, see the result) over component unit tests. Re-evaluate after Stage 2.

---

## Glossary

- **Host**: a remote server (SSH or RDP).
- **Group**: collection of hosts. Optional `parent_id` for nesting (currently UI shows flat tree).
- **Credential**: a (username, secret) pair. Secret in OS keychain. Linkable to many hosts; one host has at most one `default_credential_id`.
- **Draft**: a new host being filled in the UI. Lives in `UiStore.draft`. Promoted to a real Host once `hostname` is non-empty.
- **Promotion**: the transition of a draft into a real Host record. Triggered by the first valid save; must preserve input focus.
- **Live save**: pattern where every keystroke triggers a debounced backend write. The user never clicks Save.
- **Reveal**: clicking the eye icon on a linked credential's password to see plaintext for 10s.
- **Inline credential**: username/password typed directly into HostForm fields (vs. picked from saved). Auto-creates a credential entry when promoted/saved.

---

## When the user starts a new chat

If you (Claude) are reading this for the first time in a new conversation:

1. The user will probably say something like "продолжаем" or paste a problem.
2. Open this file, the `docs/specs/` directory, and ROADMAP.md.
3. Verify which stage we're on by checking the table above.
4. The user prefers Russian for conversation, English for code/specs/comments. Mirror their language for prose; keep code identifiers in English.
5. Don't ask a wall of clarifying questions. Make reasonable assumptions, state them, code, ship the archive, iterate.
6. **Always test `tsc --noEmit` and `vite build` before packaging.** Multiple bugs have shipped because I (Claude) didn't verify compilation.
7. **Pack the archive to `/mnt/user-data/outputs/remotehub.zip`** with `*/target/*`, `*/node_modules/*`, `*/dist/*` excluded.
8. **Update this file** before packaging if you closed a stage.

The user is technically capable, productive, and direct. Don't over-explain. If you disagree with a decision, push back once with a reason; if they hold their position, go with it.
