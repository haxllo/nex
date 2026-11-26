# The overlay and the glass material

The panel is a native window hosting a WebView2 control. Everything visible inside
it is HTML, CSS, and JavaScript. The glass material behind it is drawn separately,
by a GPU renderer. The relevant modules are `overlay::host`, `overlay::glass_ps`,
`overlay::refraction`, `overlay::icons`, and `overlay::platform`.

## What renders what

Two surfaces are stacked and kept aligned:

| Surface | Drawn by | Contains |
|---|---|---|
| Page | WebView2, from HTML, CSS, and JavaScript | The search field, rows, tiles, the assistant, settings, What's New |
| Glass | A Direct3D 11 and Direct2D renderer on its own thread | The backdrop capture, the pane material, the rim, the shadow |

The glass window is a separate top-level window: borderless, click-through, never
activating, hidden from the taskbar and from Alt-Tab. It sits pixel-aligned beneath
the page.

It could not be a child window under the WebView. A transparent Direct3D child
presented without error and never reached the screen, under flip presentation,
bitblt, and GDI alike. Input reached it; pixels did not. Moving the material to a
top-level window is what made it visible.

## How the material is built

The renderer captures the desktop behind the panel, blurs it, and then displaces
it toward the edges so the background appears to bend into the pane.

The capture comes from DXGI Desktop Duplication. The composition is a chain of
Direct2D effects (a Gaussian blur, an alpha mask, a displacement map, colour
matrices, and a composite), plus an HLSL pixel shader that carries the per-pixel
part of the material: surface slope, channel separation, and the edge light.

A few consequences follow from that design:

- **The pane shows what is actually behind it, not a screenshot of the page.** The
  centre stays comparatively clear while the border bends.
- **The silhouette is per-pixel.** Rounded corners are part of the surface rather
  than a clipped rectangle, so they stay smooth at fractional display scaling and
  no straight step appears where the curve meets the edge.
- **The tint is derived from the backdrop.** The palette is decided from the
  panel's own darkest captured pixels, so a dark window underneath makes the pane
  read deeper without a fixed tint being applied.
- **All capture and rendering run on one worker thread.** The UI thread only moves
  and shows the window, so a slow frame cannot block input.

## The page's own chrome

The lens is the material. The page sits on it and draws only the surfaces it owns —
the composer, context menus, chat panels, and the footer controls — as tiers of
tint, rim and sheen. It never uses `backdrop-filter`: the page is transparent where
those surfaces are, so a filter would sample the desktop rather than the pane and
would lose the lens a second time over.

Three tiers, from `--glass-thin` to `--glass-thick`:

| Tier | Fill | Worn by |
|---|---|---|
| Thin | 44 % of the base colour | Icon-only controls, on hover and focus |
| Regular | 52 % | The composer, the jump-to-bottom control, a pressed control |
| Thick | 64 % | Context menus and chat panels; the composer once focused |

Fill rises with the amount of text the surface has to carry. The three percentages
are the calibration dials: raise them for legibility, lower them for material.

Every tier surface shares the same edge treatment. A 1 px border carries
`--panel-edge` on the sides and `--panel-highlight` on top, so the rim is lit from
above rather than evenly, and the first `background` layer is a `--panel-glint`
gradient that fades out above the content. Both are plain declarations rather than
pseudo-elements, which keeps `::before` free for the state layers and stops the
sheen from competing with the focus ring drawn in `box-shadow`.

The composer deepens from regular to thick on focus instead of turning opaque: it
is still a surface on the material, and the extra fill is what makes the caret's
row read as active.

### When the material is not available

| Condition | Result |
|---|---|
| `prefers-reduced-transparency: reduce` | Tiers and sheen go solid; the rim stays, because it is the only edge signal left once the fill stops carrying the backdrop |
| `forced-colors: active` | Surfaces become `Canvas` with a `CanvasText` border |
| Opaque panel modes | The page paints its own background; the tiers are already on an opaque surface |

## Acrylic: the same silhouette, no lens

`panel_background = "acrylic"` draws on the same GPU layer as glass, from the
same capture, the same frost pyramid and the same `rounded_mask`. Only the look
constants differ (`glass_ps::Look::acrylic` versus `Look::clear`): the bend, the
colour split and the middle zoom are zero, the frost is much wider
(`ACRYLIC_BLUR`), and the wash is lifted, because on the lens the bend and the rim
carry the edge and here there is nothing else between the text and the wallpaper.

There is no rim light. The lens rim is directional — lit from one corner and
faded around the outline — which is what makes a lens read as a bevel; on a flat
sheet the same term leaves the border thin on one side and gone on the other. So
acrylic draws its edge the way the opaque themes do, as a uniform 1 px inset
outline on the page (`html.bg-acrylic #panel::after`), and the shader's rim terms
stay at zero.

The mode is held in `refraction::PanelMaterial` and sent through the existing
`GlassConfig` channel, so switching material never rebuilds the effect graph and
never touches a lens constant. That is what keeps the corners identical: they are
part of the shared mask, not a per-material clip. `PanelBackground::is_opaque()`
returns false for acrylic — reporting it opaque parks the layer and leaves the
panel solid.

Acrylic needs the same conditions as glass (desktop duplication available,
transparency not reduced). When those fail, `effective_panel_mode` falls back to
the opaque panel for the current theme, exactly as it does for glass.

One known gap: the D2D effect-graph fallback (used only when the HLSL shader
fails to compile) still draws the lens, so acrylic would look like glass there.
That path is a startup-failure path, not a mode.

## Why the first show can be slightly slower

The glass reveals only after a presented frame that matches the panel's real size.
Opaque modes reveal as soon as the page paints. That is deliberate: revealing a
frame prepared for the collapsed search bar and then growing into rows would show
backdrop captured at the wrong size.

The result is that the first show at a height or size not used since Nex started
can take a fraction of a second longer. After that the frame is ready and the show
is immediate.

## Fallbacks

The material degrades rather than failing.

| Condition | Result |
|---|---|
| Windows transparency off | Opaque surface, drawn by the page |
| Glass renderer cannot start | Opaque surface |
| Renderer fails mid-session | Opaque surface, applied at the time of failure rather than leaving a blank panel |
| `NEX_REFRACT_LAB=0` in the environment | Forces the fallback, which is how to tell a glass problem from a page problem |

An Acrylic backdrop is applied to the native window as the cheap path while the
glass renderer warms up, and it remains available if the renderer never starts.

## Screenshot and input caveats

Two of these will waste an afternoon if you do not know them.

**The panel cannot be captured.** Both windows call `SetWindowDisplayAffinity` with
`WDA_EXCLUDEFROMCAPTURE`, so screen capture returns whatever is behind the window
where the glass and the page should be. Snipping Tool, OBS, desktop duplication,
and a browser-driven screenshot harness all return the desktop underneath. A
phone photograph of the screen is the only way to capture the material, because
hardware bypasses the affinity flag.

**The hotkey cannot be driven by synthetic input.** Both keyboard hooks drop
events flagged `LLKHF_INJECTED`, so `SendInput` and `keybd_event` can never open
the panel. A synthetic Win press opens the Start menu and nothing else.

Verifying glass work therefore means a GPU readback probe of the real effect graph
at panel size, or reproducing the maths in a separate harness. Verifying page
layout means rendering the HTML outside the app and measuring
`getBoundingClientRect` values, which is the approach the testing notes describe.

## Positioning and display scaling

The panel centres on the monitor that holds the cursor, in the upper third of the
screen. The process is per-monitor DPI aware (v2), so the panel is sized in
physical pixels for the monitor it lands on and does not overflow its own frame at
fractional scaling.

A borderless window carries an invisible resize frame. The page is inset to
compensate, which is why dividers and the scrollbar reach the visible edge rather
than being clipped by roughly eight pixels.

## Page to host communication

Rust pushes state and the page renders it, with no synchronous round trip on the
hot path.

- State leaves Rust as JSON through `PostWebMessageAsJson`.
- User intent returns as short messages: query changes, submits, context menu
  actions, settings writes.
- HTML, CSS, and JavaScript are embedded with `include_str!` and served over a
  `nexasset://` custom protocol, so there is no asset directory to lose or to
  drift from the binary.

Rebuilding is required for any asset change to take effect. Editing the files
alone changes nothing at runtime.

## Icons

Icons are decoded to PNG and sent to the page as base64 data URIs inside the state
snapshot. The reason is specific: WebView2 custom protocols do not serve
sub-resource loads, so an `nexasset://` URL in an `<img>` tag is silently ignored.
Inlining the bytes is the way an icon reaches the page at all.

Extraction uses a 256-pixel native icon request normalised to a square, which is why
icons stay crisp in tiles and rows alike.

Two caches back it: an LRU in memory and a bounded disk cache under
`%APPDATA%\Nex\icon-cache`, validated by source size and modification time. The
disk cache is versioned, and directories from an older version are removed on
first write. Hiding the panel trims the memory tier only; the disk tier survives,
which is why the second show is cheap.

Shortcut files are resolved to their targets so the icon comes from the real
executable rather than from a low-resolution embedded copy.

## The warm release

The WebView is never torn down between shows. A timer thread arms on hide and
trims the icon cache after the configured delay, keeping the web view resident.
That is the difference between an instant show and a cold start, and it is the
reason `ui_warm_release_ms` exists as a knob.

## Next

- [architecture.md](architecture.md) for where the overlay sits in the process model
- [testing.md](testing.md) for how to verify layout and glass work
- [../adr/](../adr/) for the stack and renderer decisions
