# Reference material

External material kept here because the code implements ideas from it. None of
this is Nex's own writing, none of it is a specification of anything Nex must do,
and none of it should be edited.

| Document | Why it is here |
|---|---|
| [apple-liquid-glass-article.md](apple-liquid-glass-article.md) | A published account of Apple's Liquid Glass material, kept as background for the panel's glass |
| [liquid-glass-lens-effect.md](liquid-glass-lens-effect.md) | The only reference that states the per-pixel maths the glass shader implements: surface-slope bend, three-channel dispersion, and the squircle signed distance field behind the silhouette and the bevel direction |
| [win-key-hotkey-research.md](win-key-hotkey-research.md) | How a bare Win key can be used as a global hotkey without opening the Start menu, including the low-level hook mechanism and the suppression problem |

The first two are verbatim copies with images omitted. The third is Nex's own
research report, kept here because it is reference material about the Windows
platform rather than documentation of the product.

What Nex actually implements is described in
[../development/overlay-and-glass.md](../development/overlay-and-glass.md) and
[../development/ipc-and-privileges.md](../development/ipc-and-privileges.md).
