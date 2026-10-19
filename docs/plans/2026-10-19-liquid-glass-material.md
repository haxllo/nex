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
