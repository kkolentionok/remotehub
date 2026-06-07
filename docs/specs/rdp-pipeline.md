# RDP Pipeline — Implementation Reference (Stage 4)

Status as of Stage 4.2: connect + graphics + **mouse + keyboard** input working
end-to-end, responsive and reasonably smooth. Keyboard forwarding + the
anti-stuck-modifier sync are wired (§7); the `Scancode` API is now proven. This
document describes how the whole RDP path works.

Engine: **IronRDP 0.14** (pure Rust). We own decode → re-encode → transport →
input. We do *not* embed mstsc/FreeRDP — owning the input path is what makes the
anti-sticky-modifier feature possible (see "Why IronRDP").

---

## 1. Component map

```
crates/rh-rdp/
  src/lib.rs      types: RdpCommand, RdpInputEvent, RdpSessionEvent (incl.
                  FrameBatch + FrameTile + ImageFormat), RdpState, RdpOpenOptions,
                  RdpSpawnParams, RevealedRdpCredential, errors.
  src/actor.rs    the engine: spawn_session, run (tokio task), blocking_session
                  (std thread), connect, the active-stage loop, region diff,
                  off-thread encoder_loop, send_input, build_config, tls_upgrade.
  examples/rdp_spike.rs   the original connect/input spike (reference).

crates/rh-app/
  src/rdp_session.rs   RdpSessionManager: registry of live sessions, event
                       forwarder task, register/close/send_input.
  src/api/rdp_sessions.rs   tauri commands: rdp_session_open/close/input.

ui/src/
  lib/ipc.ts          rdpSession.open/close/sendInput
  lib/types.ts        RdpSessionEvent / RdpState / RdpInputRequest mirrors
  store/index.ts      createSession (resolution), handleRdpEvent, frame routing
  components/session/RdpViewport.tsx   canvas render + input + fullscreen
  components/session/SessionView.tsx   connecting overlay, wiring
```

## 2. Threading model

Three layers per session, bridged by channels:

1. **`run` (tokio task)** — owns the command side. Receives `RdpCommand`
   (Input / Resize / Shutdown) from the app, forwards Input to the worker via a
   `std::sync::mpsc` channel, flips an `AtomicBool` for shutdown.
2. **`blocking_session` (dedicated std::thread)** — the synchronous IronRDP
   engine. Connects, then runs the active-stage loop: read PDU → process →
   drain input → diff → hand changed-region pixels to the encoder. Blocking I/O
   is why this is its own thread, not a tokio task.
3. **`encoder_loop` (dedicated std::thread)** — receives a frame's changed
   regions (raw RGB) over a capacity-1 `sync_channel`, compresses them
   **in parallel across all cores (rayon)**, and emits one `FrameBatch`.

Events flow out via a tokio `UnboundedSender<RdpSessionEvent>`; `RdpSessionManager`
in `rh-app` forwards them to the UI `Channel`.

Why split encoding onto its own thread: compression of big regions takes
10–70 ms; doing it on the worker blocked PDU reads + input draining and caused
multi-second-feeling input lag. Off-thread = input stays responsive (~16 ms)
regardless of frame cost.

## 3. Connect flow (`connect`)

Runs the proven *blocking* IronRDP path on the worker thread:

1. `Resolving` → DNS (`to_socket_addrs`).
2. `Connecting` → `TcpStream::connect` (read timeout = `CONNECT_TIMEOUT` 20 s for
   the multi-roundtrip handshake) → `connect_begin` → TLS upgrade
   (`tls_upgrade`, custom rustls verifier, resumption disabled).
3. `Authenticating` → CredSSP/NTLM via `connect_finalize` + `ReqwestNetworkClient`.
4. **After handshake**: switch the socket to a short read timeout
   (`READ_POLL` 16 ms) so the active loop polls input every ~16 ms.

> **Critical Windows gotcha (already fixed):** the short read timeout must be set
> on the *actual* socket inside `framed` (`framed.get_inner_mut().0.sock`), NOT on
> a `try_clone()`'d handle — on Windows the clone's `SO_RCVTIMEO` does not apply to
> reads on the original handle. Setting it on the clone left the loop blocking up
> to 20 s per read → input was drained in ~20 s batches. Don't reintroduce a clone.

The `DnsQuery_W failed` ERROR lines during NTLM are **benign** (sspi probing).

`PerformanceFlags::empty()` is set so the server sends the full experience
(full-window-drag content, wallpaper, themes) rather than the bandwidth-saving
defaults (which show only a window outline while dragging).

## 4. The active-stage loop (per ~16 ms)

```
loop:
  if shutdown -> emit Closed(UserRequested); return
  drain input_rx (coalesce consecutive mouse-moves) -> send_input each
  read_pdu (<=16 ms; WouldBlock/TimedOut -> continue)
  active_stage.process(&mut image, ...) -> outputs
    ResponseFrame -> write back to socket;  Terminate -> Closed
  every FRAME_INTERVAL (33 ms): compute_regions(image vs last_frame)
     -> try_send to encoder (cap 1):
          Ok        -> advance last_frame
          Full      -> skip frame (don't advance last_frame; changes roll into
                       next diff) — natural coalescing, no backlog
          Disconnected -> break
```

`image` is an IronRDP `DecodedImage` (RGBA), mutated in place by `process`. So
the framebuffer is always the latest decode regardless of the codec the server
used (bitmap / RemoteFX → we get crisp pixels; the blur was only ever our own
JPEG re-encode, since fixed by PNG for small regions).

## 5. Region diff + transport (the perf core)

- **Banding:** the screen is sliced into horizontal bands of `BAND_H` (64 px).
  Each band is diffed against `last_frame` independently and only the changed
  sub-rectangle is extracted. This avoids one giant bounding box merging two
  far-apart changes (e.g. a click up top + a tray-clock tick at the bottom) into
  a near-fullscreen rect — that merging caused periodic lag spikes.
- **Extraction** (`compute_regions` / `make_region`): cheap memcpy of changed
  RGB into owned `RegionJob`s. Done on the worker; the expensive compression is
  not.
- **Parallel compression** (`encoder_loop` + rayon): each tile is PNG or JPEG:
  - `w*h <= PNG_MAX_AREA` (40 000 px) → **PNG** (lossless → crisp text/UI).
  - larger → **JPEG q85** (compact; wallpaper / big repaints / first frame).
  Tiles of a frame are compressed across all cores, then sent together.
- **One `FrameBatch` per frame** (`tiles: Vec<FrameTile>`). The UI draws all
  tiles of a batch in a single synchronous pass so the frame is presented
  coherently — drawing tiles individually as they decoded caused **tearing**
  during fast motion (an older band landing on top of a newer one). Batches are
  also chained in arrival order on the UI side (`drawSeq`).

### Tuning constants (`actor.rs`)
| const | value | meaning |
|---|---|---|
| `READ_POLL` | 16 ms | active-loop socket read timeout (input responsiveness) |
| `CONNECT_TIMEOUT` | 20 s | handshake read timeout |
| `FRAME_INTERVAL` | 33 ms | diff/emit tick (~30 fps target) |
| `BAND_H` | 64 px | diff/encode band height |
| `PNG_MAX_AREA` | 40 000 px | PNG below this, JPEG above |
| `JPEG_QUALITY` | 85 | JPEG quality |
| `CMD_CHANNEL_CAP` | 256 | input command channel |

### Telemetry
`encoder_loop` logs once/sec: `rdp frame stats fps=.. avg_encode_ms=..
max_encode_ms=.. total_kb=..`. `max_encode_ms` = worst single-frame compress
time; `total_kb` = transport/sec. Also `rdp mouse button` per click.

## 6. Resolution & display

- The UI requests a resolution at connect (`createSession`). We render at the
  **monitor resolution** so the picture is native at fullscreen and only ever
  *downscaled* (sharp) when windowed — never upscaled (blurry). The backend
  reports the server's actual negotiated size via the `Resized` event; the canvas
  sizes to that so region coordinates line up.
- Canvas CSS: `object-fit: contain` (letterbox if aspect differs),
  `image-rendering: auto` (smooth downscale).
- Pointer mapping (`toCanvas`) accounts for the contain letterbox (scale +
  centering offsets) so clicks land correctly at any display size.
- **Fullscreen:** the viewport uses the Fullscreen API; toggle via the corner
  button or **Ctrl+Alt+Enter**, exit also via **Esc**.
- **Known limitation:** resolution is fixed at connect time. Resizing the app
  window after connecting does not re-render at the new size (windowed = downscale
  of the monitor-res frame). True live reflow needs the DisplayControl dynamic
  virtual channel — see Future work.

## 7. Input

- **Mouse: done.** `RdpInputEvent::{MouseMove, MouseButton, MouseWheel}` →
  `ironrdp::input::Database` → `process_fastpath_input`. Moves are throttled to
  ~25/s on the UI and coalesced on the worker. Lesson from the spike: a pointer
  move and a click need a beat between them; in the live app real motion provides
  it.
- **Keyboard: done.** The UI emits `Key { code }`, `SyncModifiers`,
  `ReleaseAllModifiers`; the actor (`send_input`) maps the browser
  `KeyboardEvent.code` → PS/2 **Set 1** scancode via `code_to_scancode`
  (letters/digits/punct/function/nav/numpad/intl; extended keys carry the
  `0xE0` flag), builds `ironrdp::input::Scancode::from_u8(extended, code)` and
  feeds `Operation::KeyPressed/Released` to the `Database` →
  `process_fastpath_input`. The `Database` synthesises release-before-press on
  auto-repeat, so we forward browser repeats as-is.
  - **Modifier-sync (the anti-stuck fix):** on focus loss the UI sends
    `ReleaseAllModifiers` → `Database::release_all()` (blanket KeyUp of
    everything held — cures stuck Ctrl/Alt after Alt-Tab). On focus gain it
    sends `SyncModifiers` (OS modifier state) → we diff against the
    `Database` (`is_key_pressed`) and emit only the deltas via `sync_mod`, so a
    redundant press never fires a spurious repeat. We sync Ctrl/Alt/Shift/Meta;
    **lock-LED sync** (Caps/Num/Scroll) needs a `TS_SYNC_EVENT`, not exposed by
    ironrdp-input's fast-path `Database` — deferred (see Future work).
  - `Scancode` (`ironrdp-input` 0.5.0) is now proven: `Scancode::from_u8(extended:
    bool, code: u8)`, plus `From<(bool,u8)>` / `From<u16>` (0xE000 = extended).

## 8. Why IronRDP (and not mstsc/FreeRDP)

The killer feature is fixing stuck modifiers (Ctrl/Alt) after focus changes —
a real mstsc annoyance on multi-monitor. That fix lives in the keyboard/scancode
layer, which the mstsc ActiveX control hides as a black box. Owning the input
path (IronRDP) is the whole point. The cost is we own rendering/transport — hence
this pipeline. If rendering ever needs native codecs (H.264), the fallback is
**FreeRDP everywhere** (renders to our surface, input still ours), never the
Windows-only ActiveX control.

## 9. Performance characteristics (measured)

At 1640×988 full-window-drag, after all optimizations:
- compress: avg ~5 ms, max ~20 ms (was 130 ms before region-diff + parallel).
- transport: ~0.5–0.9 MB/s (network is not the bottleneck).
- fps: ~12–15 during heavy drag. **This is now the server's repaint rate**, not
  our limit — encoding and input are no longer bottlenecks. Half the diff ticks
  see "no change" because the server only repaints the drag ~15×/s.
- Going beyond ~15 fps on heavy drag needs the server to send faster, i.e. the
  RDP 8+ Graphics Pipeline (RemoteFX/H.264 GFX) — a large separate effort.

## 10. Future work / backlog

- **Keyboard: done** (4.2). Remaining keyboard polish: **lock-LED sync**
  (Caps/Num/Scroll) via a `TS_SYNC_EVENT` so the server's lock state matches the
  client at focus time; and PrintScreen/Pause full sequences (currently
  best-effort / omitted).
- **Dynamic resize** via `ironrdp-displaycontrol` (DisplayControl DVC) — live
  reflow on window resize / fullscreen instead of fixed connect-time resolution.
  Note IronRDP issue #447: post-resize perf degradation on the RemoteFX codepath
  (fine on bitmaps).
- **GFX/H.264** decode for higher server frame rates + native-grade quality.
- **TOFU cert pinning**: replace the accept-all rustls verifier with fingerprint
  pinning against the existing `rdp_known_certs` store + a `CertPrompt` flow.
- **Clipboard** (text first), audio, smartcard.
- **Server cursor** rendering (currently `enable_server_pointer: false`).
- Off-thread improvements: adaptive JPEG quality during motion; libjpeg-turbo
  (turbojpeg) for faster compression (C dependency trade-off).

## 11. Test server

`5.42.106.222:3389`, user `Administrator`. Connect/auth/graphics/mouse proven via
live sessions. Password is not stored in the repo — rotate if it leaked into logs.

---

## Activation (Stage 4.x) — now a user setting

GFX is opt-in via `Settings.rdp_gfx` (default off → proven legacy IronRDP path).
Toggle in **Settings → Connections**. Plumbing: `rdp.gfx` key (flat k/v settings
table) → `RdpOpenOptions.gfx` → `actor.rs` enables the GFX DVC when
`params.options.gfx || env RDP_GFX`. The `RDP_GFX=1` env var still forces it on
for dev. Reconnect to apply (read at session open).

Diagnostic logging is quiet by default: the 1/s `GFX ops/sec` op-counter and the
startup `S2Cache`/`Cache2S`/`Wts1 ClearCodec` dumps emit only under
`RDP_GFX_TRACE=1`. The `Wts2 prog` trace prints per-row coverage
`rows=[y:x0-x1 ...]` in addition to the bbox.

### Maximize→restore "disocclusion band" — resolved (was stale cache, not a clear-miss)

Earlier suspected as the server not repainting the vacated region. A black-desktop
trace disproved it: on restore the server DOES clear the vacated top strip (black
`Cache2S` tiles into rows y=0/64/128). The residual fragments were a STALE
bitmap-cache replay — a `CacheToSurface` of a cache slot still holding
old-wallpaper-era pixels (blue) stamped where the current (black) desktop belongs.
Root cause: the desktop wallpaper was changed **mid-session**, and the GFX bitmap
cache lives for the whole session, so a slot cached under the old wallpaper got
replayed. It does NOT reproduce on a fresh reconnect (empty cache). Not a real
ongoing bug; a possible hardening item (cache coherence on wallpaper change) only.

### Dynamic resize (DisplayControl DVC) — enabled, GFX-gated

The full path exists: `RdpCommand::Resize` → `active_stage.encode_resize(w,h,None,None)`
(coalesced; handles the server's resolution-change reactivation; emits `Resized`)
→ `rdp_session_resize`/`RdpResizeRequest`/`RdpSessionManager.resize()` → IPC →
`SessionView`/`RdpPopoutApp`. The continuous window-resize `ResizeObserver` in
`RdpViewport` was previously hard-disabled (`ENABLE_DYNAMIC_RESIZE=false`) because
the legacy RemoteFX reactivation degrades repaint (IronRDP #447) and can leave an
unrepainted region. It is now an `enableDynamicResize` prop, driven by the
`rdp_gfx` setting: **GFX sessions reflow live, legacy stays off**. The fullscreen
one-shot resize is independent and always on. Reconnect after toggling GFX so the
live session's path matches the gate.

### AVC/H.264 GFX decode — PARKED (needs an AVC-capable server)

The last RDP item, parked in the backlog. Caps already advertise AVC420; the current
build logs AVC arrival via the skip arm. Blocked because the test server negotiates
RemoteFX Progressive, never AVC. To resume: enable the "Prioritize H.264/AVC 444 graphics
mode" GPO on a Windows host (works without a GPU, software encode), connect with GFX on,
confirm `skip codec Avc420/Avc444` in the log. Then: backend manual `RFX_AVC420_METABLOCK`
parse + forward H.264 to the webview (`RdpSessionEvent::Avc`), frontend WebCodecs
`VideoDecoder` → draw region rects (decode already spiked green). AVC444 dual-stream after.
Design: AVC pixels live on the canvas (parallel draw path), not the backend fb. Full
plan in docs/STATE.md "RDP AVC/H.264 — parked".
