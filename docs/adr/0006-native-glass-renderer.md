# ADR-0006: Draw the glass material natively in a separate top-level window

## Status

Accepted.

## Date

2026-10-05

## Context

The panel's material is a live refraction of whatever is behind it: the backdrop
bends toward the edges, the rim catches light, and the tint follows the content
underneath. CSS alone cannot do this. A `backdrop-filter` blurs the page's own
backdrop, not the desktop behind a transparent window, so it cannot bend anything.

The material needs three things the web layer cannot provide: a capture of the
desktop behind the window, per-pixel displacement of that capture, and a
silhouette that is part of the drawn surface rather than a clip applied to a
rectangle.

The first attempt made the glass a transparent Direct3D child window beneath the
WebView. It presented without error and never reached the screen. Input reached it;
pixels did not. That held under flip presentation, bitblt, and GDI alike.

## Decision

Draw the glass on the GPU in a separate top-level window, and keep it aligned
beneath the WebView.

- Capture comes from DXGI Desktop Duplication, with no picker and no OS border.
- Composition is a Direct2D effect chain (blur, alpha mask, displacement map,
  colour matrices, composite) plus an HLSL pixel shader for the per-pixel part.
- All capture and rendering runs on one worker thread. The UI thread only moves and
  shows the window.
- The glass window is borderless, click-through, never activating, and hidden from
  the taskbar and Alt-Tab.

## Alternatives considered

### A Direct3D child window under the WebView

Attempted first and abandoned. A transparent child presented and never appeared.

### CSS `backdrop-filter` in the page

Rejected because it cannot see the desktop. It blurs what the page itself has
painted, which is not the same thing and cannot produce an edge that bends.

### DWM acrylic, or Mica

Kept as the cheap fallback while the renderer warms up and when it cannot start.
Rejected as the material itself because it offers a single blur level, no
displacement, and no control over the rim or the corner profile. It also rounds
frames at a fixed radius, which produced two visible corner curves once the panel
used its own.

## Consequences

- **The panel cannot be screenshotted.** Both windows set display affinity to
  exclude themselves from capture, which also means every screenshot and screen
  recording returns the desktop behind the window. Verification uses GPU readback
  probes or a photograph of the screen. This costs real debugging time and is
  documented in the testing guide so it is not rediscovered.
- The reveal waits for a frame at the panel's real size, so the first show at a new
  height can be slightly slower than in opaque modes. This is deliberate: revealing
  a frame captured at the wrong size shows the wrong backdrop.
- The material needs transparency enabled and a working GPU path. Every failure
  degrades to the opaque surface, including a mid-session renderer failure, so the
  panel never ends up blank.
- The corner profile is shared between the native silhouette and the page, which is
  what stops a straight step appearing where the curve meets the edge.
- Two windows must stay pixel-aligned across moves, resizes, and DPI changes.
