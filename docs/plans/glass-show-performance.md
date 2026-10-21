# Glass show: responsiveness, smoothness, weight

Tracking doc for the glass-theme show path. Started 2026-10-21.

The complaint: in glass theme the panel feels heavy and slow to come up, and
growth while typing is not smooth. Opaque/acrylic reveal the moment the page
paints; glass reveals only after a presented frame that matches the panel
size, and its worker is a separate thread with its own pacing.

## How a glass show is assembled

| Stage | Where | Glass-specific cost |
|---|---|---|
| `Show`: position, glass sync, chrome, theme detect, state push | `host.rs:1000-1055` | config sends, `sync_to_main` twice |
| Page: apply state, render, rAF, measure, `resize`+`painted` | `app.js:901-912` | 1-2 frames (all themes) |
| Worker wake + drain config | `refraction.rs:2200-2299` | 0-10 ms dead time (`sleep(10)` while hidden) |
| Lens content rebuild at new size | `refraction.rs:2413-2425` | ~2-15 ms |
| Crop + sample + present + ack | `refraction.rs:2440-2520`, `2655-2665` | 1-2 frames |
| Reveal + state push, page drops its surface | `host.rs:1346-1355` | +1 frame |

The floor for any show is the page's first paint after the state push. Glass
adds wake, rebuild, present and ack on top of it.

## Work items

Status: `[ ]` todo, `[x]` done, `[~]` in progress.

### 1. Growth must not lag the window (smoothness) — `[x]` (revised)
Original finding: the region advances the instant a resize is requested, but
the glass repaint lands a frame or two later, so the newly exposed band could
show unblurred desktop under new rows.

First attempt gated region growth on the renderer's ack (150 ms fallback).
That was worse in practice: growth visibly trails the window by one frame, and
the panel feels late while typing. Reverted to immediate growth; kept the
`(w, h, scale)` dedupe so repeated syncs at the same size do not rebuild the
GDI region.

Why the flash is rare without the gate: the worker now wakes on the config
itself (item 3) and the resize pass re-presents from the mirror before it
waits for a new desktop frame, so the present usually lands before the next
composition. If a band artifact is reproducible on a real machine, the next
option is a headroom pre-render (draw the body ~96 px taller than the panel
and let the region clip it); that trades GPU per frame for zero growth lag.

Acceptance: growth tracks the window immediately; no persistent band artifact.

### 2. Remove the 60 px show step (quickness) — `[x]`
`Show` forces the window to `INITIAL_HEIGHT = 60` and syncs the glass to it
(`host.rs:1005-1017`), then the page measures its real idle height (~109) and
resizes. Every show therefore does two lens rebuilds and two presents, and the
first ack is always discarded.

Fix: stop resizing at Show. The window keeps its last height (cloaked, so
nothing visible), the page's first `resize` drives both window and glass.

Acceptance: one rebuild per show; panel appears at its final height, no
first-paint grow; show latency drops by roughly one present.

**Implemented.** Show no longer calls `apply_window_height`/forces 60. It
re-syncs the glass at the window's retained logical height (so a monitor or
DPI move while hidden still lands) and records that height as
`last_applied_height`; the page's first measurement owns the resize. The
`WebviewReady` race path still applies `INITIAL_HEIGHT` because the page has
not measured yet.

### 3. Wake the worker on config, not on a 10 ms timer — `[x]`
The hidden loop sleeps 10 ms (`refraction.rs:2297-2301`), so `prepare_show`
waits up to 10 ms before any GPU work starts, and the thread wakes 100 times a
second for nothing.

Fix: sleep on the config channel with `recv_timeout(until_poll)` so a show
config wakes the worker immediately; keep the 4 Hz material poll.

Acceptance: `show_arm` to first glass present loses the 0-10 ms dead gap;
idle wakeups drop to ~4/s.

**Implemented.** Idle wait is now `recv_timeout` bounded by the next material
poll (1-250 ms). A config consumed by the wait is pushed into `pending_cfgs`
and drained at the top of the next pass, so it is not lost.


### 4. Wait for a live desktop frame on show refresh (quality) — `[x]`
`refresh_mirror` uses `AcquireNextFrame(0)` (`refraction.rs:2130`), so on a
static or slow desktop the reveal carries the held mirror, up to
`HIDDEN_POLL_MS` (250 ms) old, then jumps to live.

Fix: wait up to ~8 ms for the show's refresh pass only.

Acceptance: no visible backdrop jump when the overlay opens over moving
content; +0-8 ms only when the desktop is still.

**Implemented differently than written.** Waiting unconditionally would
charge every show over a still desktop up to 8 ms to prove nothing changed —
and over a still desktop the mirror is already current, so the wait buys
nothing. The wait is now gated on `moving` (`desktop_recent`, the same signal
the swell uses): animating desktop → up to `SHOW_REFRESH_WAIT_MS` (8 ms, one
120 Hz frame) for a fresh frame; still desktop → 0 ms. Policy lives in
`refresh_wait_ms`, with a unit test (`a_show_waits_only_on_a_desktop_that_can_answer`).

**Result:** reveal carries a current backdrop over video/animations; static
opens pay nothing.

### 5. Remove the per-frame GPU readback stall (weight) — `[x]`
`sample_backdrop` (`refraction.rs:1247-1295`) does D2D `EndDraw` →
`CopyResource` → `Map(READ)` every presented frame: a full pipeline sync, also
on the reveal frame.

Fix: double-buffer the 16×16 staging texture and map the previous frame with
`DONOTWAIT`, or sample at 4-10 Hz and interpolate the tint.

Acceptance: frame times steady under typing/scroll; no stall on the show
frame; tint follows the backdrop a frame late at most.

**Implemented.** Two staging mirrors, alternating: copy this frame into one,
`Map(READ | DO_NOT_WAIT)` the other. A failed map just keeps the current
material for that frame instead of failing the frame. The first read after an
engine build still blocks once, so the probe tests and the very first frame
get real pixels. Call sites updated: the normal path presents even without a
sample (tint skipped), and a sample miss no longer counts toward the
engine-reset failure budget.

**Result:** the per-frame D3D11 pipeline sync is gone; material/tint now trail
by one frame (it already moved at hysteresis speed, so nothing visible).


### 6. Cache the D2D target bitmap per swapchain buffer (weight) — `[x]`
`render_frame` calls `CreateBitmapFromDxgiSurface` every frame
(`refraction.rs:1352-1360`) because flip-model buffers rotate.

Acceptance: no per-frame bitmap creation; lower CPU per presented frame.

**Implemented.** `Engine::targets` caches the wrapper keyed on the buffer's
COM identity (`as_raw()`), so the two alternating flip-model buffers each get
one bitmap and every frame after the first two is a cache hit. `resize_buffers`
clears the cache *after* `SetTarget(None)` — the wrappers point at the buffers
DXGI is about to replace, and a live one would make `ResizeBuffers` fail.

**Result:** one COM allocation per buffer per swapchain generation instead of
one per frame.

### 7. Clip the glass HWND to the panel (weight) — `[x]`, awaiting visual check
The glass window is permanently max-panel size plus shadow margin
(`refraction.rs:526-530`); DWM composites the whole alpha surface even when
the panel is 60 px tall.

Fix: `SetWindowRgn` on the glass HWND to panel + shadow margin, updated on
resize. Verify it clips the DComp surface with `WS_EX_NOREDIRECTIONBITMAP`;
fall back to a DComp clip if not.

**Implemented.** `apply_glass_region` (rect: panel + `SHADOW_MARGIN` on every
side) runs from the same `apply_region` funnel that writes the main window's
silhouette, so it shares the (size, scale) dedupe and lands on sync/show/
prepare/reveal. `ensure_window_max` drops the dedupe key when the HWND grows,
so a grown window does not keep an old smaller region.

Rectangular on purpose: the rounded silhouette and its AA come from the
backbuffer alpha mask, and GDI regions have hard edges. Soft shadow falls off
inside the 44 px margin (`SHADOW_BLUR` 26), so nothing is cut.

Not yet confirmed: whether DWM honours the region for a
`WS_EX_NOREDIRECTIONBITMAP` + DComp surface. If composition area does not drop
(and the visuals are unchanged), the fallback is a DComp clip on the visual —
not worth doing unless the region proves inert.

Acceptance: compositor work scales with the visible panel; smoother desktop
animation behind the panel, especially on iGPU/4K.

### 8. Move icon encoding off the UI thread (quickness) — `[x]`
`push_state` base64-encodes icon PNGs synchronously (`host.rs:2429-2460`),
called at Show and again after reveal, plus on Hide. Cold decode can block.

Fix: cache encoded payloads by (path, mtime, size); on miss, send state
without icons and follow up when the worker finishes.

Acceptance: show and reveal never block on PNG encode; worst case (first open
after boot) improves the most.

**Implemented (root cause, cheaper than the written fix).** The decode was
already off the UI thread and already progressive (`png_bytes_cached` +
`ApplyIcons`); what repeated was the *encoding*: every push re-base64'd every
visible icon, up to 48 of them per show and again per keystroke. The cache now
keeps the `data:` URI next to the PNG (`IconCache::data_uri_cached`), encodes
once per cache entry, and drops the URI with the entry on evict/clear/resize.
`data_uri` moved from `host.rs` to `icons.rs` with it.

No (path, mtime, size) keying: the cache key already carries the
normalization version, and a changed file on disk is picked up by the same
eviction/trim path that drops its PNG.

**Result:** first show after boot still pays one encode per icon (on the host
thread, but only once); every later push is a hash lookup plus a JSON copy.
Test: `a_data_uri_is_encoded_once_per_icon` (padding + cache-hit identity).

### 9. Reveal and push state before focus work (quickness) — `[x]`
`Painted` uncloaks, activates, focuses, evaluates script, re-asserts topmost,
then reveals and pushes state (`host.rs:1326-1355`).

Fix: reveal + push first, focus/topmost after. Keep raw-input registration and
uncloak where they are.

Acceptance: 2-6 ms less to first visible frame; no frame with an opaque page
over already-revealed glass. No WM_CHAR beep regression.

**Implemented.** The glass block (`set_visible(true)` → `reveal()` → state
push) moved from after `set_focus()` to just after `window.set_visible(true)`,
so the panel no longer spends the focus/topmost work without its backdrop.
`register_raw_input_sink`, `focus_input` (DOM focus primed before the window is
visible — that is what prevents the WM_CHAR beep) and the foreground request
stay exactly where they were.

First attempt moved it earlier still, ahead of `request_foreground`/
`set_visible`, and produced a visible flash: glass on screen before the page
was. The reveal must follow the page becoming visible, or the backdrop appears
alone. `OVERLAY_VISIBLE` also stays with the show, not with the focus step.

### 10. Arm the open animation at reveal (quality) - `[x]`
`prepare_show` arms the swell and the interaction spring before the reveal
(`refraction.rs:406-418`), and the first content resize adds a ±1.5 velocity
kick (`2340-2347`) as the panel appears.

Fix: arm the swell on reveal; skip the interaction kick for the first resize
after show.

Acceptance: the 420 ms settle is visible from the moment the panel appears; no
wobble at appearance. Honour reduced motion.

**Implemented.** `ShowCycle::arm` now only arms the held re-present; the settle
is a second arm (`arm_swell`) sent by `reveal()` itself as a `GlassConfig
{ revealed: true }`, so every reveal path (Painted, and the `GlassPresented`
ack) starts the spring at the moment the panel becomes visible. Skipped
entirely when animations are disabled.

The typing kick is gated on `kick_ready`, false from show until that reveal
config: the page's first measurement resize would otherwise wobble the panel
exactly as it appears.

### 11. Adaptive frame cap (smoothness) — `[ ]`
`MIN_FRAME_MS = 16` (`refraction.rs:220`) caps follow at 60 Hz.

Fix: 8 ms when the monitor refresh is ≥120 Hz.

Acceptance: growth follows within one refresh interval on high-refresh
displays.

### 12. Correct mixed-DPI placement (correctness) — `[ ]`
`position_window` centres with the window's current scale, i.e. the monitor it
is leaving (`host.rs:2814`), and `ScaleFactorChanged` is not handled.

Fix: use the target monitor's DPI; on scale change re-sync the glass and
re-measure content.

Acceptance: first show after moving to a different-DPI monitor is centred and
does not resize after appearing.

### 13. Minor cleanups — `[ ]`
- Cache accent/theme detection instead of registry reads on every Show
  (`host.rs:1023-1025`); invalidate on `WM_SETTINGCHANGE`.
- Skip `keep_webview_viewport_max` when already at max.
- Guard `find_webview_child` against class names longer than `WRY_WEBVIEW`
  (slice panic risk) and cache the child HWND.
- `effective_panel_mode` reads state under two locks (`host.rs:2344-2359`).

## Measurement

Existing log lines: `show_latency show_arm_ms`, `show_latency painted_wait_ms`,
`panel {w}x{h} re-presented in {ms}ms (live|mirror)`, `glass revealed {w}x{h}`.

Add per-show timestamps before tuning further: `show → config sent`,
`config → first present`, `present → ack`, `ack → revealed`, `revealed →
uncloaked`. `captured/s` includes the idle spin and is not a perf metric.

## Order

1, 2, 3 first (highest payoff per line). Then 4, 5, 6, 7 for weight. Then 8-10
for the remaining show latency and feel. 11-13 opportunistic.

## Progress log

- 2026-10-21 — Audit done, doc created. No code changes yet.
- 2026-10-21 — 1, 2, 3 implemented; release build (`--bin Nex --bin NexHelper`)
  passes. Still to verify on a live session: region gate has no visible growth
  lag beyond one frame; region dedupe does not leave a stale silhouette on
  monitor/DPI change; show appears at its retained height with one rebuild.
- 2026-10-21 — 10 implemented: the settle is armed by `reveal()` (new
  `GlassConfig { revealed }`) instead of by the show, and the typing kick is
  suppressed until then. Visual check passed. To verify: open over a static
  desktop — the full settle should play, with no wobble as the panel appears.
- 2026-10-21 — 9 implemented: glass reveal + state push now run after the page
  is shown but before the focus/topmost calls. First attempt (reveal ahead of
  the show) flashed glass without the page; narrowing the move fixed it.
  Visual check passed. To verify: 2-6 ms less to the first complete frame.
- 2026-10-21 — 8 implemented: data URI cached beside the PNG in `IconCache`;
  every push after the first per icon is a lookup. Targeted lib tests pass
  (`a_data_uri_is_encoded_once_per_icon`). To verify: open twice in a row and
  compare `show_latency ... show_arm_ms` / `painted_wait_ms`; the second should
  not pay encoding.
- 2026-10-21 — 7 implemented: glass HWND region = panel + shadow margin,
  applied through `apply_region`, invalidated when the window grows. Visual
  check passed (shadow intact, no clipping on grow/shrink). Committed.
- 2026-10-21 — 6 implemented: D2D target wrapper cached per swapchain buffer
  (COM identity), cache cleared on ResizeBuffers. Release build passes. To
  verify: normal glass use on a DPI change (drag between monitors) still
  renders and resizes without "ResizeBuffers failed"; per-frame CPU no longer
  pays the wrapper allocation.
- 2026-10-21 — 4 implemented: show refresh waits up to 8 ms for a newer
  desktop frame, but only when the desktop is animating (the still case is
  already current and would just burn show latency). Release build + test
  target compile pass. To verify: open over playing video — the first glass
  frame should already be the current frame; open over a still desktop —
  unchanged.
- 2026-10-21 — 5 implemented: double-buffered statistics readback, non-blocking
  map of the previous frame. Release build passes. To verify: `render_avg` in
  the per-second refract log should drop and stay flat under typing; material
  may flip one frame later than before (not visible in practice).
- 2026-10-21 — Region gate reverted after live test: growth felt one frame
  late. Immediate growth restored, region dedupe kept. See item 1.
- 2026-10-21 — Regional caveat found while implementing 1: region growth now
  depends on acks, so a worker that stalls without failing would lag by
  `REGION_ACK_TIMEOUT_MS` (150 ms) per growth event, not pin permanently.

## Expected effect of the landed items

- Show: one lens rebuild instead of two, first presented frame is no longer
  discarded, and the worker starts as soon as `Show` is dispatched. Order of
  magnitude: 16-40 ms off a warm show; more when the previous session's height
  differs from the idle height.
- Growth: tracks the window immediately again. The gate that caused the
  one-frame trail is gone; the tradeoff (a possible band of unblurred glass if
  a present misses the next composition) is invisible in practice with the
  instant worker wake.
- Idle: ~100 wakeups/s drop to ~4/s; the desktop-duplication session stays
  warm as before.
- Unchanged: the page's first paint is still the floor. Anything below ~1
  frame of WebView2 paint needs a different state protocol, not these fixes.
- Weight (5): no GPU stall per presented frame. Expect lower `render_avg` in
  the refract log, steadier frames while typing/animating, and less contention
  with the compositor on iGPUs. Tint/material may lag one frame; not visible.
- Weight (6): no per-frame COM wrapper allocation; CPU per presented frame
  drops a little, mostly visible as smoother pacing under load.
- Weight (7): DWM composites panel + shadow margin instead of the whole
  max-panel surface. Biggest effect when the panel is collapsed (bar height)
  on a large window: most of the composited pixels are gone.
- Quickness (8): icon base64 no longer repeats per push. The first show after
  boot still encodes each icon once; after that, Show and every keystroke push
  copy cached strings instead of encoding. Most visible with a full result
  list of distinct app icons.
- Quickness (9): the panel no longer waits for the foreground request and
  focus assignment before its backdrop appears. Expect 2-6 ms less to the
  first complete frame, and the open looking the same otherwise.
- Quality (10): the full 420 ms settle plays while the panel is visible
  instead of running behind the page's first paint, and the panel no longer
  wobbles from the appearance resize. Reduced-motion users get no settle at
  all, same as before.

