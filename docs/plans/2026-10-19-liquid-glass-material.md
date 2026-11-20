# Liquid Glass Material Implementation Plan

**Goal:** Improve Nex's native lens material while preserving capture/reveal correctness, legibility, and responsiveness.

**Architecture:** Keep DXGI capture and D2D rendering on the glass worker. Add a spatial blur blend and channel-separated refraction, then native lighting, restrained grain, backdrop-aware tint, and spring-driven material response. Share the corner profile between native clipping, displacement, and the page. Preserve immediate hide-before-launch semantics.

**Tech stack:** Rust 2024, windows 0.61 D3D11/D2D/DXGI, tao/wry WebView2, CSS. No new runtime dependencies planned.

## User requirements

- Complete subtle rim chromatic aberration; sharper center with softer rim; spring response on opening, closing, resizing and pressing; pointer-responsive curved highlights; focus dimming; smoothed local backdrop tint and contrast support; continuous corners; interior depth/contact shadow; very low transmitted-image grain.
- Shape morphing is optional and last, separate from optical changes.
- Decide HDR scope; honor reduced motion/transparency; measure performance.
- No regressions to moving-backdrop first frame, palette hysteresis/dwell, dither, cloak visibility, or stale-hover fixes.
- Keep effect processing native. Use CSS only for page-specific styling/clipping.
- Add focused numeric regression tests, build using `G:\nex-target`. Do not run the full hanging Windows test suite.
- User explicitly requested a complete handoff document if work cannot finish in this session.
- No release, commit, or push requested.

## Corrections to the initial brief

- Apple's implementation details are not public. These are visual design goals, not claims that every effect is mandatory or matches Apple's shader.
- D2D displacement is `scale * (channel - 0.5)`: scale 10 gives approximately +/-5 pixels, not 10 pixels.
- Check D2D saturation's supported property range. A color matrix supports deliberate vibrancy above identity without relying on out-of-range saturation behavior.
- Multi-scale blur should preserve detail, not increase general milkiness. Grain must remain weaker than the already reduced CSS noise.
- `HideSync` is used for hide-before-launch: never delay its acknowledgement for animation. Normal hide can be designed separately, but must not resurrect a stale/hidden overlay.
- Current glass HWND uses a rounded region; main HWND uses DWM default rounding. Continuous corners require replacing both silhouettes in glass mode, not just a CSS change.

## Starting point (clean master, 74bb3a2)

`apps/core/src/overlay/refraction.rs`:
- `Engine::{build,rebuild_content,present_frame,render_frame}` owns graph/resources.
- Current graph: Gaussian blur -> Saturation -> DisplacementMap -> flat tint.
- `bezel_bytes` builds the rounded rectangular map. `ShowCycle` controls one refresh plus 220 ms ease-out swell.
- `worker_main` drains `GlassConfig`, handles hidden 250 ms polling, render pacing, refreshes, captures and stats.
- `desktop_recent` (60 ms) and `swell_wants_mirror` prevent frozen-mirror animation over video. Preserve this invariant for new animations.
- `Material::update` preserves palette EMA, hysteresis and 8-frame dwell; fresh show bypasses dwell.
- `GlassWindow::apply_round_region` clips native glass; max-size HWND/swapchain remain stable across height changes.

`apps/core/src/overlay/host.rs`:
- `apply_window_height` synchronizes glass region/content with requested geometry.
- `prepare_glass_show`, `Painted`, `GlassReady`, `glass_active` own reveal readiness.
- `hide_overlay_window` serves both `Hide` and `HideSync` and cloaks immediately.
- `apply_window_chrome` uses DWM default corners.
- `snapshot_state_json` gates `glassNative` on readiness and background mode.

`apps/core/assets/style.css`: native glass ring/glint pseudo-elements around lines 2092–2148; dark-only text halo; glass palette easing.

`apps/core/src/overlay/platform.rs`: reusable `detect_registry_dword`; currently no motion/transparency preference helpers.

## Implementation sequence

1. **Optics:** build tight and wide Gaussian branches. Blend wide blur using a smooth bezel mask over tight center. Feed vibrancy matrix into three displacement branches, mask RGB separately, add channel outputs with correct alpha. Keep padded source coordinates identical on all branches. Test neutral center, inward edge normals, channel separation and blur mask. Build and run targeted refraction tests.
2. **Motion/input:** exact damped spring integration with bounded elapsed time and settle threshold; resize and press targets must not reset velocity. Read pointer/button/focus on native worker or send coalesced host state. Animate cached source on static desktop only, never starve live capture. Honor Windows reduced-motion setting. Design normal close separately from synchronous hide.
3. **Tint/depth:** sample a small spatial grid, smooth RGB and luminance without weakening palette dwell; interpolate native tint/contrast smoothly so tile boundaries cannot show. Add curved native highlight, inner shade and static low-amplitude grain; avoid animating random noise.
4. **Corners:** one continuous profile for map normals and native regions; project same profile into CSS clipping. Update main and glass HWNDs together, restore standard corners when leaving glass. Keep hit testing and context menus functional.
5. **Accessibility:** read Windows animation and transparency preferences; opaque fallback must suppress native capture presentation and page transparency together. Re-check preferences on show (or settings change); reduced motion stops spring and pointer light animation, not contrast updates.
6. **Verification:** targeted numeric tests; actual offscreen D2D readback probes for center/rim/channel behavior; debug/release build as appropriate; runtime moving/static backdrop timing. CPU submission timing is not GPU completion timing—label measurements honestly.
7. **Optional morph:** only after optical work is verified. Do not block hide-before-launch or move text independently from native glass.

## Checks and commands

PowerShell:
```powershell
$env:CARGO_TARGET_DIR='G:\nex-target'
$env:CARGO_TERM_COLOR='never'
cargo test -p nex --lib overlay::refraction::tests -- --test-threads=1
cargo build --release --bin Nex --bin NexHelper
```

Stop Nex/NexHelper before linking when the target executable is running. Existing installed executable at session start: `%LOCALAPPDATA%\Programs\Nex\bin\Nex.exe`.

Regression scenarios: show/hide over video, paused video, bright/dark/split backdrop, resize between search/chat, pointer enter/leave/press/release outside, focus loss, opaque mode, reduced motion/transparency. Show/hide rapidly to expose stale completion events. Check no new square corners, edge halos or unreadable text.

`WDA_EXCLUDEFROMCAPTURE` prevents desktop screenshots from proving the panel's material appearance. Use renderer readback for pixel correctness and user eyeball for final tuning. Log submitted render time and cadence separately; GPU readback probes must stay out of production critical path.

## HDR decision

Keep SDR output for this iteration. Current BGRA8 duplication and swapchain are an SDR pipeline; true HDR needs capture format negotiation, linear/scRGB processing, color-space metadata and HDR presentation together. Do not claim that changing a single bitmap format solves it.

## Progress / handoff

### Implemented in working tree

1. `refraction.rs`: tight sigma-2 and wide sigma-8 Gaussian branches, smooth bezel blend (center wide weight 0.12, edge 0.90), saturation color matrix, three RGB displacement branches with +/-8% dispersion, additive channel recombination. D2D AlphaMask reads scatter weight from the normal map's otherwise-unused blue channel.
2. Exact damped spring (`Spring::step`) drives show, press/release, and resize impulse. Show deadline 420 ms; ~4% overshoot. Bend clamps to 1.12. Native worker reads pointer, left button, foreground HWND. Windows animation setting disables spring and pointer-following light on next show.
3. Native static directional highlight bitmaps follow lens normals; pointer changes only their draw opacity. Focus loss reduces highlight intensity. Native inner bottom/right shade and fixed <=1-level grain replace CSS glint/rim/noise. Four light bitmaps plus one depth/grain bitmap rebuild with geometry, not every frame.
4. A 4x4 backdrop RGB grid supplies continuous hue-biased tint and local contrast scrim. D2D scales that tiny premultiplied bitmap with linear interpolation. Each cell gets EMA smoothing; fresh show clears the grid. Existing material palette/hysteresis/dwell remains intact.
5. Power-4 corner profile supplies map normals and surface shading; sampled corner offsets supply both GDI polygon regions and CSS `clip-path`. Both main and glass HWNDs receive the same native region in glass mode. Main region clears on hide/opaque switch; DWM default rounding remains for opaque modes. Radius 8 logical pixels; render geometry receives scale through `GlassConfig`.
6. `platform.rs`: Windows client-area animation preference and `EnableTransparency` registry read. `host.rs`: opaque snapshot/WebView fallback when transparency disabled; `glass_active` gates reveal too. `app.js`: receives shared `glassClip`. CSS removes obsolete painted optical layers and disables palette transitions with reduced motion.
7. Hidden-state fix while adding animation: hide cancels pending spring/refresh; no mirror animation presents during hidden poll. Reveal refresh updates a fresh material before presenting, rather than waiting for another desktop frame.

### Checks so far

- `cargo check -p nex --lib`: passed (pre-existing warnings).
- Targeted refraction tests, including explicit real D2D GPU probe: **19 passed** before later snapshot-test isolation and benchmark edits.
- GPU readback verifies gray preservation/opaque alpha, inward refraction, RGB edge separation, no center color split, and greater center detail than rim. Initial gentle ramp produced less than one quantized 8-bit level of channel separation; strengthened synthetic ramp, not production aberration.
- Debug probe: 160x96 graph + GPU readback ~0.91 ms/frame. Full 700x441 material CPU submission+Present ~4.33 ms/frame; initial full graph/resources build ~306 ms in DEBUG. These are not release-runtime cadence or show-latency guarantees.
- Broader overlay tests initially **85 passed, 2 failed** because machine has Windows `EnableTransparency=0` and old snapshot tests assumed true. Fixed by extracting `snapshot_state_with_transparency` and injecting preference in tests; added opaque-fallback regression. Re-run result to be recorded below.
- Release binaries built successfully once before latest hidden-state/snapshot changes. Must rebuild latest tree before user testing.
- `node --check apps/core/assets/app.js` and `git diff --check`: passed before latest small changes.
- Logs on G: `glass-tests.txt`, `glass-gpu.txt`, `glass-overlay-tests.txt`, `glass-release.txt`, `glass-release-tests.txt` under `G:\Ciqada\tmp\opencode`.

### Remaining mandatory work (do not label feature complete)

1. **Normal close elasticity:** currently all hides still cloak immediately. Preserve `HideSync`'s immediate semantics. For regular hide, design an epoch-guarded visual close that releases input/focus immediately, does not expose stale content, and cannot re-hide a newer show. Do not sleep UI thread. Existing `Hide` resets/pushes page state while hidden, so delaying cloak alone will show cleared/reflowing content. Needs an explicit close-state contract, not a timeout graft.
2. **Geometry elasticity:** present implementation changes material curvature on resize/press, not the actual window silhouette. If physical compression is required, coordinate page, native regions and hit testing; this overlaps optional morphing. Never transform only the WebView while glass remains fixed.
3. **External contact shadow tied to thickness:** implemented inner shade only. Current glass/window region clips at the panel; an external soft shadow needs shadow-bearing margins/composition surface with matching positioning. Do not paint a shadow inside and claim it is external.
4. **Per-text contrast proof:** local 4x4 scrim is backdrop-aware but not text-bound-aware. Test bright/dark split backdrops and tune without washing the whole pane. Existing dark-only halo stays. No guarantee of WCAG contrast from current sampling alone.
5. **Full-frame performance and runtime regressions:** compare original vs new release on static and moving content at 700x441 and max size, including resize cost and first-show. Offscreen probes prove graph correctness only. Measure p95 and GPU completion separately from existing CPU counters. Consider caching geometry layers by dimensions if rebuilds cost too much; no cache added speculatively.
6. **Accessibility live changes and mode transitions:** currently preferences re-read on show/state pushes, animation on show. Verify transparency off/on while visible, opaque->glass and glass->opaque transitions, high-contrast behavior if supported. Machine has transparency disabled: new app intentionally renders opaque until Windows preference permits transparency. Do not silently edit user's registry.
7. **Corner integration:** eyeball both HWND/CSS silhouettes, context-menu overflow and edge hit testing at changing panel heights. Numeric test proves mathematical agreement, not DWM rasterization. The polygon has 16 samples per quarter and integer native coordinates. Check tiny endpoint rounding and any corner halo.
8. **GPU resilience:** runtime D2D rebuild/error recovery remains inherited; test failures/device loss with new graph. `set_bend` currently ignores property errors as before; defaults are valid, but follow up if logs indicate rendering drift.
9. **Final cleanup:** stale comments/log names (`swell`, blur/saturation log) can be clarified. Do not reformat the whole file or remove unrelated user code. No release/commit requested.

### Optional / explicitly deferred

- Shape merge/morph after mandatory work passes.
- HDR kept SDR per decision above; do not extend capture pipeline here.
- Persistent public tuning UI not requested. Internal constants remain tuning points.

### Continuation instructions

- Read current diff before editing; changes are **uncommitted**.
- Do not discard the implementation just because this is a handoff. Numeric and actual GPU validation already exist.
- No subagents authorized by user; main agent performed all work.
- No application was launched or user preferences altered during the checks recorded above. Installed Nex/NexHelper had exited before release build.
- Run targeted release tests, then build latest release. Use the full GPU test command above with `--include-ignored` or exact `optical_graph_gpu_readback -- --ignored --exact --nocapture`.
- Update this document with final results and any new limitations before handing off again.

### Latest verified checkpoint

**Partial implementation, not feature-complete.** No commits or release created. Source and release binaries now contain the changes listed above.

- Final command: `cargo test --release -p nex --lib overlay:: -- --include-ignored --test-threads=1 --nocapture` — **88 passed, 0 failed, 0 ignored**, including D2D GPU readback and reduced-transparency snapshot test.
- Final command: `cargo build --release --bin Nex --bin NexHelper` — **passed**, pre-existing warnings only. Output `G:\nex-target\release\Nex.exe` and `NexHelper.exe`.
- Last probe: optical 160x96 + synchronous GPU readback **0.54 ms/frame**; 700x441 content rebuild + buffer growth **21.93 ms**; full material CPU submission + Present **3.40 ms/frame**. One machine/run; **not p95**, not captured-video benchmark, not full GPU-completion timing.
- Reduced expensive exponent/root evaluation outside the bezel/corners; brought release rebuild probe down from 36.13 ms to 21.93 ms. Still needs production resize profiling (normal resizes do not grow swapchain buffers).
- `SetWindowRgn(NULL)` now runs only when transitioning away from visible glass, avoiding repeated region clearing on every opaque-state push.
- `node --check apps/core/assets/app.js` and `git diff --check` passed; recheck after any continuation changes.
- No runtime visual test yet. Windows transparency remains disabled (`EnableTransparency=0`), so current installed settings will select opaque fallback in new build. User preferences unchanged.
- Latest logs: `glass-final-tests.txt`, `glass-final-build.txt`, `glass-final-gpu.txt` under `G:\Ciqada\tmp\opencode`.

**Next agent's first work:** inspect final diff; validate native/CSS corners and new optical appearance with user, then benchmark moving/static full-size backdrop. Resolve normal close animation and external contact shadow with explicit lifecycle/geometry design. Do not call the existing press/resize curvature response a physical shape morph.

**Additional integration concerns to inspect:**
- `glassClip` is currently serialized in every snapshot (~68 polygon points); move immutable profile to initialization or asset serving if snapshot overhead matters.
- Local-tint EMA advances on presented frames, including mirror animation. It is not yet wall-clock based; behavior can differ with frame cadence. Palette EMA/dwell intentionally remains the old design.
- Small tint bitmap is linearly interpolated across the full panel; assess boundary/clamp behavior visually and with a split-color GPU probe.
- Native source normal map uses 8-bit neutral 128 (not exact 0.5), inherited from old graph; residual center displacement is tiny but not mathematically zero. Current GPU test tolerates one level. Float normal map is an upgrade only if measurable artifacts remain.
- New surface lighting is four directional opacity weights, not a physical environment reflection model. No independent animated interior diagonal sheen remains. Add a moving low-frequency glint only if needed after visual review, without reinstating static CSS glare.
- Hard cutoff at 32 logical pixels for native shade/highlight is below roughly one 8-bit level; check no perceptible seam on flat backdrops.
- Device lifecycle inherited from existing worker does not yet report a persistent fatal-engine failure back to host as opaque fallback. This is worth addressing before declaring resilience complete.

### User visual feedback: cleaner material / low-quality border

User reported initial material looked less clean and border looked low quality. Windows transparency is now enabled (`1`) and `panel_background="glass"`; user was running `G:\nex-target\release\Nex.exe`. Existing live logs confirmed active capture/presentation, so this feedback concerns native material, not opaque fallback.

Changes:
- Disable DWM border with `DWMWA_COLOR_NONE` sentinel on both main glass-mode HWND and companion glass HWND. Restore DEFAULT for main opaque/acrylic modes. Unsupported Windows versions may ignore this attribute.
- Replace inset Gaussian ridge (peak 1.4 px inward) with monotonically fading boundary glint. Lower ambient edge intensity and taper along edges; no uniform framed outline.
- Reduce bottom/right inner shade from 0.055 to 0.025; shorten falloff.
- Halve local tint/scrim strength (0.008 + conflict * 0.035), retaining hue sampling.
- Center Gaussian sigma 2 -> 3.5: suppress distracting transmitted detail while preserving center/rim difference. Rim stays sigma 8.
- New regression checks edge profile never forms a secondary ridge and top highlight tapers.

Verification: **89 release overlay tests passed**, including GPU probe; latest release build passed. Preview restarted from `G:\nex-target\release\Nex.exe`. First build hit a respawned running executable; stopped/waited for process exit, rebuilt successfully. No commit/release. Logs: `glass-clean-edge-tests.txt`, `glass-clean-edge-build.txt` on G temp path.

Visual quality still requires user confirmation; no claim made that numeric tests prove border appearance. Binary polygon corner aliasing remains a possible separate issue if complaint persists specifically at corners.

### Continuation checkpoint: committed baseline + close elasticity + engine fallback

Committed the handoff tree on branch `liquid-glass-material` as `8c19c4e` (no push, no release). User confirmed the border is removed.

**Normal close elasticity (was Remaining #1).** Regular hides no longer cloak immediately:
- `UiCommand::Hide` keeps hotkey masking, foreground restore and click-through immediate, then posts `{"t":"closing"}` to the page and sends `GlassLayer::begin_close()`. It schedules `UiCommand::CloseDone(epoch)` on a timer thread; the UI thread never sleeps.
- Close contract: `close_epoch` bumps on every Hide-with-animation and every Show. `CloseDone(epoch)` cloaks only when `close_pending == Some(epoch)` and the panel is still logically hidden. A Show inside the deadline wins; a second Hide or `HideSync` invalidates the pending close. `HideSync` remains immediate with no animation.
- The page fade (CSS `html.panel-closing #panel`) runs while the native lens releases from bend 1 to 0 (`close_bend`, smoothstep over `CLOSE_MS = 190`). The host cloaks at 220 ms, after the lens has settled. `swell_pass_due` re-presents the held mirror during the release on a static desktop, using the existing mirror/live invariants.
- The page is not pushed the cleared state until `CloseDone`, so the fade shows the last painted content, not an emptied panel. The next show push removes the close class before the reveal.
- Input: parent and WebView child both get `WS_EX_TRANSPARENT` for the duration; the child is not hidden (hiding it would pop the content). The show path clears both.
- Reduced motion (`animations_enabled == false`) or an unready page skips the animation and uses the old immediate hide.

**GPU resilience (was Remaining #8).** `note_engine_failure` counts consecutive build/rebuild failures; three requests `UiCommand::GlassFailed` once. The host then renders opaque (`glass_active` and `snapshot_state_json` consult `refraction::glass_failed()`, `sync_webview_background` falls back to the theme's opaque surface) and parks the layer. The worker keeps retrying, so a lucky recovery still presents; the page surface is only downgraded.

**Verification (this checkpoint).**
- `cargo test --release -p nex --lib overlay:: -- --include-ignored --test-threads=1`: **90 passed, 0 failed, 0 ignored**. The first run had one environmental failure (`instance_signal_handles_absent_window`) because the preview instance was running; stopped Nex/NexHelper, rerun clean.
- New regression: `the_close_releases_the_lens_monotonically_without_overshoot` (start 1, midpoint 0.5, end 0, no reversal/overshoot, no close = 1).
- `cargo build --release --bin Nex --bin NexHelper` passed; `node --check apps/core/assets/app.js` and `git diff --check` passed.
- GPU probe at this tree: optical 160x96 readback 0.94 ms/frame; 700x441 content rebuild + buffer growth 34.57 ms; full material CPU submission + Present 4.30 ms/frame. Still single-run, CPU-side, not p95.

**Open visual question:** close feel. Fade length, lens release, and whether the page fade reads as a settling panel or a plain dissolve need eyeball confirmation. Nothing numeric proves the close looks right.

**Remaining mandatory work, renumbered.**
1. External contact shadow tied to thickness (was #3): still inner shade only; needs shadow-bearing margins/composition surface.
2. Per-text contrast proof (was #4): 4x4 scrim is backdrop-aware, not text-bound-aware.
3. Full-frame performance/runtime (was #5): static vs moving release benchmark at 700x441 and max size, resize cost, first-show; p95 and GPU completion separate from CPU submission.
4. Accessibility live changes/mode transitions (was #6): transparency toggle while visible, opaque↔glass, re-check on show.
5. Corner visual verification (was #7): eyeball both HWND/CSS silhouettes, context menus, edge hit testing, corner aliasing.
6. Geometry elasticity (was #2): optional physical silhouette morph; still last.
7. Cleanup (was #9): stale `swell` naming now also covers close; blur/saturation log text. Not done to avoid churn.

Also carried: `glassClip` serialized per snapshot; tint EMA frame-based; 8-bit neutral normal map; 32 px shade cutoff seam check.

### Continuation checkpoint: GPU-resident frames, no close animation, refraction/silhouette

User asked to drop the close elasticity (it read as a dissolve and did not justify its cost) and focus on material performance and appearance. The close path is back to the immediate cloak for every hide; `HideSync` still acknowledges without waiting. CSS `panel-closing`, `playClose`, `CloseDone`, close epochs and the close bend were all removed. `GlassFailed`/opaque fallback stays.

**Structural: GPU budget / no full-frame CPU readback.** The steady-state path no longer maps the panel to the CPU:
- The desktop duplication texture is mirrored into a `D3D11_USAGE_DEFAULT` + `BIND_SHADER_RESOURCE` texture (`cap.mirror`) on the GPU, retaining the full desktop for later resizes.
- Only the panel box is copied GPU→GPU (`Engine::copy_crop` → `source_tex`) and wrapped as the D2D source bitmap via `CreateBitmapFromDxgiSurface`.
- Material/tint statistics come from one 16x16 D2D downsample (`sample_backdrop`): 1 KiB map instead of a full-panel Map + memcpy + upload. `backdrop_grid` averages all 4x4 pixels per cell and `frame_luma` steps full pixels.
- The stretched last-frame fallback and `stretch_bgra_nearest` are gone; clamped/partial GPU crops cover the edge cases. `present` reads the held `source_tex`; `present_frame` no longer exists.
- Stats sampling still synchronizes once per presented frame (small `Map`); it is the only remaining per-frame CPU readback.

**Refraction realism.** The normal map gained a thickness tail (`SLAB_BEND = 0.08`, `SLAB_DECAY = 30.0`): past the bevel the surface keeps bending slightly before flattening, so the lens reads as a slab with depth instead of a pure edge. The tail direction is radial from the panel centre, avoiding a seam at the medial axis. The centre stays neutral and does not split RGB (within one 8-bit level).

**Edge silhouette.** The main window's native region is grown by `SILHOUETTE_GROW_PX = 1.0` device pixel while the glass window uses the exact profile (`grow = 0.0`). The page's antialiased CSS clip defines the visible edge, so the native polygon no longer acts as a hard, aliased outer cut. Region generation is factored into `lens_region_points(w, h, scale, grow_px)`.

**Verification at this tree.**
- `cargo test --release -p nex --lib overlay:: -- --include-ignored --test-threads=1`: **90 passed, 0 failed, 0 ignored**.
- New regressions: `thickness_tail_bends_past_the_bevel_but_not_the_center`; `silhouette_growth_only_expands_the_cut`; the ignored GPU probe now writes a four-colour quadrant desktop pattern, copies/samples the GPU path and asserts orientation and channel order (TL red, TR green, BL blue, BR white).
- GPU probe: optical 160x96 **0.49 ms/frame** with readback; 700x441 rebuild + buffer growth **28.94 ms**; full material **3.54 ms/frame** CPU submission + `Present` (not GPU completion). Single run, not p95.
- `cargo build --release --bin Nex --bin NexHelper` passed (release binary at `G:\nex-target\release\Nex.exe`). No commit/release/push.

**Open questions.**
- Live eyeball still required: silhouette/corner edge after the 1 px growth, thickness tail strength, and whether the 16x16 stats granularity causes visible tint banding.
- The sample readback's channel/orientation is now asserted; an earlier single failure was a stale first sample, not a flip.
- User was actively using the machine, so the preview was stopped for tests and not restarted automatically.

**Remaining mandatory work, renumbered.**
1. External contact shadow (unchanged): needs shadow-bearing margins/composition surface.
2. Per-text contrast (unchanged): 4x4 scrim is backdrop-aware, not text-bound-aware.
3. Full-frame performance: static vs moving release benchmark at 700x441 and max size, first-show, p95 and GPU completion separate from CPU submission.
4. Accessibility live changes: transparency toggle while visible, opaque↔glass, re-check on show.
5. Corner visual verification: both HWND/CSS silhouettes, context menus, edge hit testing, corner aliasing.
6. Optional geometry morph; last.
7. Cleanup: stale `swell` naming (now no close), blur/saturation log text; sample cadence if the per-frame `Map` shows up.
8. Stats sample cadence: sampling every presented frame is the remaining sync; consider every N frames only if measured to matter.

### Continuation checkpoint: composition corners, stronger slab edge

User feedback after the circular-corner build: corners still read as a diagonal cut, edge light only slightly visible while moving the pointer, and the pane still lacks the look of a thick block of glass. "3D" here means how the live backdrop is warped and presented at the borders, not a CSS transform.

**Corner geometry.**
- Radius raised `8 -> 12` logical px. At 8 px a hard-rasterised quarter arc reads as a chamfer at 100% scale.
- `lens_region_points(w, h, scale, grow_px)` now expands each corner radially from its own arc centre. The previous `x/y - grow` trick was a square dilation: 1.41 px outward at the diagonal, 1 px inward at the tangents once clamped. Points may sit just outside the window rect; Windows clips them. The test now asserts radial growth is exactly `SILHOUETTE_GROW_PX` at every sample.
- The glass HWND no longer uses `SetWindowRgn` at all. It presents through a **composition swapchain** (`CreateSwapChainForComposition`, premultiplied, flip-sequential) hosted on an `IDCompositionVisual` with an `IDCompositionRectangleClip`; the window has `WS_EX_NOREDIRECTIONBITMAP`. The visual clip is updated per rebuild (`Engine::apply_clip`, DIPs = physical / scale). This removes the binary GDI corner staircase from the glass layer; the main window keeps a 1 px-grown GDI region only so its antialiased CSS clip is not shaved.
- If composition setup fails, the glass engine fails and the existing acrylic/opaque fallback takes over; there is no second region-based glass path.

**Edge / slab readability.**
- `BEND_SCALE 11 -> 14`: the rim now pulls ~6.5 px at the boundary instead of ~5 px.
- `SLAB_BEND 0.20` (was 0.16) keeps a sub-pixel inward draw across the body, so the whole pane participates instead of only the 32 px bevel.
- Rim scatter reduced from `0.10 + 0.55 * smoothstep` to `0.10 + 0.45 * smoothstep`, so the refracted edge stays legible instead of milky.
- `ABERRATION` held at `0.07`; `scale * aberration = 0.98 px`, under the existing "no coloured outline" invariant. Do not raise it without changing that test and re-eyeballing text edges.

**Diagnostics caveat.**
- A temporary `NEX_GLASS_CAPTURE=1` bypass skipped `WDA_EXCLUDEFROMCAPTURE` so screenshots could see the panel. It is removed from the tree. With exclusion off, desktop duplication captures Nex's own page, so the glass refracts a copy of its own rows ("effects on the components instead of the background"). Any screenshot taken that way is invalid for material or corner judgement.
- Normal builds keep capture exclusion on. Visual checks require a phone photo or a user eyeball; renderer readback can prove pixels but not the composed HWND edge.

**Verification at this tree.**
- Debug refraction suite: **20 passed, 1 ignored** (GPU probe).
- GPU probe after the tuning: optical 160x96 **0.56 ms/frame** with readback; 700x441 rebuild + buffer growth **29.71 ms**; full material **2.51 ms/frame** CPU submission + `Present` (not GPU completion). Single run, not p95.
- `cargo build --release --bin Nex --bin NexHelper` passed. No commit/release/push.
- The full release overlay suite was last green before the composition swapchain change; rerun it with Nex stopped before calling this checkpoint verified.

**Still mandatory.**
1. Eyeball the composed panel over a bright grid/text backdrop: corner silhouette, pointer edge light, border convergence, and whether the 12 px radius matches the rest of the UI.
2. External contact shadow (unchanged): needs shadow-bearing margins/composition surface.
3. Per-text contrast (unchanged): 4x4 scrim is backdrop-aware, not text-bound-aware.
4. Full-frame performance: static vs moving release benchmark at 700x441 and max size, first-show, p95 and GPU completion separate from CPU submission.
5. Accessibility live changes: transparency toggle while visible, opaque<->glass, re-check on show.
6. Context menus / edge hit testing / panel-height changes with the composition clip; confirm the visual clip tracks every resize and DPI change.
7. Optional geometry morph; last.
8. Cleanup: stale `swell` naming (no close), blur/saturation log text, unused DXGI imports, and the per-frame stats `Map` if it shows up in measurements.

### Continuation checkpoint: alpha-mask corners, physical-pixel DComp transform

User report on the composition-clip build: corners curved the right shape but **pixelated**; content **overflowed the panel at 125% display scale**; edge light visible only on two opposite corners.

- The `IDCompositionRectangleClip` is gone. DirectComposition rounded clips are hard-edged, so they only moved the staircase from GDI to DComp. The silhouette now lives in the **backbuffer alpha**: `rounded_mask()` builds a panel-sized premultiplied bitmap (analytic rounded-rect SDF, 1 px coverage ramp) and `render_frame` composites it with `D2D1_COMPOSITE_MODE_DESTINATION_IN` after the material. The compositor blends that alpha, so the arc is antialiased in the same pass that draws the glass. `target_props()` is back to `D2D1_ALPHA_MODE_PREMULTIPLIED` (valid on the composition swapchain), and the backbuffer is cleared to transparent before the material.
- **125% overflow:** the DComp visual tree is DPI-scaled by the host window, so a 700 px surface was being presented at 875 px while the panel stayed 700 px. `Engine::apply_visual` now sets `visual.SetTransform2(Matrix3x2::scale(1/scale, 1/scale))`, cancelling the window DPI; the mask and material remain in physical pixels. `windows-numerics` is a direct dependency for `Matrix3x2`.
- Edge light base raised (`0.10 -> 0.20` opacity, taper `0.18 + 0.82(1-t)^2 -> 0.34 + 0.66(1-t)^2`) so all four edges read as glass, not just the two nearest the pointer.
- Verification: debug refraction suite **21 passed, 1 ignored** (new `rounded_mask_is_transparent_outside_and_opaque_inside`); release GPU probe **0.53 ms optical / 3.08 ms full frame**; release binaries built and Nex restarted. The DPI transform direction assumes the compositor scales by window DPI (confirmed by the 125% overflow symptom); if a photo shows the panel too small instead, the sign of that scale is wrong.
- Still needs a user eyeball: pixelation gone, 125% geometry correct, edge light on all four sides.

### Continuation checkpoint: contact shadow, reveal handshake, geometry cache

User reports across this round: the right edge lost its last ~8 px of glass and the scrollbar read as clipped; the pane still lacked a body ("3D" means the backdrop must visibly converge into the rim); the first frame of a show could be a picture from the previous session; and the show felt late — first the whole glass layer, then (after that was fixed) only the edge bend.

**Right-edge clipping (committed in `1bcf284`).** The glass region was built at the client origin but the window it clipped sat at the *outer* origin, so the region stopped short of the right edge by the frame inset. `apply_lens_region` now translates the polygon onto `client_origin_in_window()`.

**External contact shadow.** The main window is `SetWindowRgn`-clipped to the panel silhouette, so it cannot hold pixels outside the panel and the shadow belongs to the glass layer: the glass window, its backbuffer and its DComp visual all gained `SHADOW_MARGIN = 44` physical px, with the material still starting at the margin so the panel stays aligned with the page's client area. `shadow_bytes()` builds a premultiplied-black rounded-rect falloff (blur 26, peak 0.30), the panel interior is zeroed, and it is drawn after the alpha mask with a plain `DrawBitmap` (SOURCE_OVER), so it can only reach outward and never lifts the glass itself. There is no offset: the cue is identical on all four sides.

**Reveal handshake.** A one-shot `GlassReady` was replaced by `UiCommand::GlassPresented { w, h }`: every present made while the layer is not warm acks its size, `GlassLayer::note_frame` records it, and `reveal()` lifts the cloak only when that size equals `last_region`. `set_visible(false)` re-arms and also forgets the frame size, so a reveal can never match the previous session's held buffer — the log had shown `glass revealed 700x441` landing before that show's `panel 700x441 re-presented`. The page keeps its painted panel until the reveal push and only then goes transparent.

**Bend on the reveal frame.** The show swell ran a spring from flat, so the edge stretch was at 40-70 % when the panel became visible and finished ~100 ms later — read as the edge effect arriving late. `SWELL_START = 0.85` opens most of the way on the first frame; the settle (with a hair of overshoot) remains. `BEND_SCALE 14 -> 24`, `BEZEL_MARGIN 32 -> 40`, `SLAB_BEND 0.20 -> 0.30` for the requested border convergence.

**Geometry cache (the show latency fix).** The reveal waits for a frame at the panel's current size, and the pass that produces it was regenerating ~42 ms of frame-independent layers at 700x441: refraction map 11.2, four edge lights + grain upload 18-20 (generation 11-14), mask 4-7, shadow 4-5. `SizedGeometry` (map, lights, grain, mask, shadow) now lives in a 2-deep MRU `GeometryCache` keyed by `(panel_w, panel_h, scale_bits)`; a hit is a refcount bump and only the capture source and effect pointers are rebuilt. Two entries is the show cycle itself — bar box while hidden, settled box while shown, each promoting itself — so the pair stays resident while a live resize that crosses other heights still rebuilds once per height. Live log after the change: `panel 700x441 re-presented` 32-54 ms -> 2-10 ms, and `glass revealed` lands with `show_latency host_ms` instead of a rebuild later. The same change removes a ~36 ms stall from every visible resize.

**Verification at this tree.**
- `cargo test -p nex --lib overlay::refraction::tests -- --test-threads=1`: **24 passed, 1 ignored** (new `geometry_cache_keeps_the_two_hottest_boxes`; `the_swell_opens_most_of_the_way_and_ends_exactly_once` and the shadow symmetry assertions retuned).
- GPU probe (release): optical 160x96 0.59 ms/frame with readback; content rebuild 700x60 **8.51 ms**, 700x441 **35.79 ms**; cached rebuild of the same boxes **0.50 / 0.96 ms**; rebuild + buffer growth **2.01 ms**; full material ~7 ms/frame CPU submission + `Present` (not GPU completion).
- Release binaries built; Nex restarted; ten consecutive shows logged from the live runtime.

**Still mandatory.**
1. Per-text contrast: the 4x4 scrim is backdrop-aware, not text-bound-aware.
2. Accessibility live changes: transparency toggle while visible, opaque<->glass, re-check on show.
3. Context menus / edge hit testing / live resize with the composition surface; confirm the visual clip and the shadow margin track every resize and DPI change.
4. Optional geometry morph; last.
5. Cleanup: stale `swell` naming (no close animation any more), blur/saturation log text, unused DXGI imports, and the first show at a never-seen height still paying the uncached ~36 ms.

**Cleared by user eyeball (2.28.0).** Composed corners and contact shadow at 100 % and 125 % scale, edge light on all four sides, edge bend on the reveal frame, overall show feel: accepted. The contact-shadow and corner items that the previous list held were dropped from "Still mandatory".

### Checkpoint: full-frame perf, CPU submission vs GPU completion

`overlay::refraction::tests::full_frame_perf_report` (ignored, needs a GPU) drives the real visible-panel sequence at both boxes: `copy_crop` from a stand-in duplication mirror, the 16x16 `sample_backdrop` readback, `update_local_tint`, `present`. Three variants per frame, so the costs cannot hide in each other:

- **submit** — CPU submission only: crop, tint upload, D2D draw, `Present`.
- **frame** — the production frame, including the 16x16 statistics map (this is the frame's built-in CPU/GPU sync point).
- **gpu** — the same frame with a `D3D11_QUERY_EVENT` recorded after `Present` and waited on, i.e. GPU completion rather than CPU hand-off.

Static runs hold the crop box and bend; moving runs scroll the crop through the mirror and sweep the bend from `SWELL_START` to resting.

Release, 120 measured frames after 20 warm, `p50 / p95` in ms (second run; the first run's 700x441 `first_show` read 24.68 ms because it was the process's first effect build — it settles at ~12 ms):

| box | mode | submit | frame | gpu | fps at gpu p95 |
|---|---|---|---|---|---|
| 700x441 | static | 3.82 / 5.22 | 4.34 / 5.97 | 4.17 / 5.67 | 176 |
| 700x441 | moving | 3.94 / 5.42 | 4.40 / 6.04 | 4.13 / 5.69 | 176 |
| 700x680 | static | 5.17 / 6.44 | 5.79 / 7.25 | 5.68 / 8.06 | 124 |
| 700x680 | moving | 5.41 / 6.90 | 6.05 / 7.35 | 5.50 / 7.16 | 140 |

First show at a fresh size (cold geometry, plus the frame the reveal waits on): **11.9 ms** at both 700x441 and 700x680.

Reading: the frame fits the runtime's 16 ms cap with more than 2x headroom at the settled box and ~2x at `MAX_HEIGHT`, so `MIN_FRAME_MS` is the limiter and not the material. `gpu` tracks `frame` within a few tenths of a millisecond, so there is no GPU tail hiding behind the CPU hand-off: the per-frame 16x16 map already serialises on the GPU, and waiting on it explicitly costs almost nothing. Moving costs ~0-0.3 ms over static, i.e. the animated bend and the scrolling crop are not a special case. The probe asserts one ceiling (settled static frame p95 < `MIN_FRAME_MS`) so a real regression fails instead of only printing.


### Checkpoint: capture visibility follows the material

`SetWindowDisplayAffinity` used to be set on **both** the glass and the main HWND at layer creation, for every launch and every panel mode. That is the same flag screenshots, PrintScreen, OBS and Discord read, so the whole app vanished from captures in every mode - including Black/White, which never touch the glass and have nothing to feed back. The affinity now follows the material: `sync_capture_exclusion` (host) sets `WDA_EXCLUDEFROMCAPTURE` on the main window only while the native glass actually paints, and `WDA_NONE` for opaque panels and for `NEX_REFRACT_LAB=0`/glass-failure fallbacks. The glass window itself stays self-excluded always (its duplication feed would otherwise contain its own last frame). Known ceiling: while the glass material is live the app still cannot be captured - a window that refracts the live desktop cannot also be inside that same capture. Restoring glass captures means moving the material to a system-composited backdrop, which loses the refraction.

### Checkpoint: DirectComposition composes in device pixels

The `1 / scale` visual transform added with the composition swapchain was a
regression, not a fix. DirectComposition targets a window's client area in
device pixels (its clip, offsets and transforms are all pixel-space; content
is clipped to the window rect), so cancelling a "window DPI scale" shrank the
whole layer instead: at 125% a 988 px buffer painted at 790 px, leaving the
page's content outside the right and bottom of the glass. `Engine::apply_visual`
is gone and the visual keeps its identity transform. Buffers, the alpha mask,
the contact shadow and the margin-compensated placement were already physical
pixels, so they map 1:1 with no transform. The earlier 125% "overflow" that the
transform was written for did not come from the compositor; treat it as
misattributed. This is a no-op at 100% scale, which is why the 100% eyeball did
not catch it.
