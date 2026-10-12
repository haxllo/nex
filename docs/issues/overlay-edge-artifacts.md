# Overlay edge artifacts (double lines, square corners, cut shadows) — RESOLVED

Black/white opaque modes showed a rotating cast of edge defects: a thick double edge, then two corner curves of different radii, then a square border poking past rounded corners, then sheared drop shadows with unclean right/bottom cuts. Glass mode was unaffected throughout (different silhouette path). Each defect had a different painter — fixed in order below. All on `master`, verified by rebuild + visual check each step.

## 1. Thick double edge — inset highlights + outer ring (`a7c37cd`)

The 1px `panel-edge` stroke plus the 1px outer ring shadow rendered as one 2px band on black. Dropped both in opaque modes; depth left to the soft drop shadow and inner highlights.

## 2. Two corner curves — DWM acrylic corners (`bedcdc8`)

The main window painted a DWM acrylic backdrop over its full square rect while DWM rounds frames at its own fixed ~8px. Panel at 12–20px → a smaller curve visibly behind ours. Opaque panels cover the window fully, so the backdrop was set to `NONE` there (as glass mode already did).

## 3. Flat ring + leaking acrylic sliver (`ede3959`)

With stroke and ring gone, the transparent 1px border still showed acrylic through it, and the inset highlights printed as extra lines. Border dyed to the panel color (`#000`/`#fff`, width kept so layout doesn't shift); inset highlights removed in opaque modes.

## 4. Region side effects — added then scoped, then reverted (`3da6c4f`, `313f452`, `4a2b425`)

A 20px main-window round region was tried to force a single silhouette. It caused worse problems than it solved, in order found:
- clipped drop shadows and AA fringes, and risked cutting live content (tao applies `set_inner_size` asynchronously, so a region built from a read-back rect can be stale);
- confused Chromium's viewport math (shifted/clipped compositing inside the page — deterministic per size, no stutter).

Reverted to fallback-only; opaque and glass modes draw their own silhouette and must keep it. Lesson: window regions and WebView2 do not mix — shape the page, not the window.

## 5. Square ticks at the corners — DWM 1px window border (`459fee5`, `7234233`)

Win11 draws a 1px DWM border on the raw (square) window rect regardless of backdrop/region/corner settings. First dyed to the panel color (`DWMWA_BORDER_COLOR` is opaque RGB, no alpha), then removed at the source with `DWMWA_NCRENDERING_POLICY = DISABLED` (borderless custom UI needs no non-client rendering). Dye stays as fallback.

## Rules for future edge work

- One silhouette owner per mode: glass region + CSS in glass mode, solid panel in opaque modes, region only for the acrylic fallback.
- Never blanket-clip the main window; Chromium reads window geometry.
- DWM draws three independent things (backdrop, border, shadow) — verify each, assume none.
- Radius lives in three places that must move together: CSS `--radius`, glass round region, bezel-map `CORNER_RADIUS` (currently 20px).
