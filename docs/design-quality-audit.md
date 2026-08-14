# Design Quality Audit — verified against code (2026-08)

Each point: claim → code reality → credibility (does it actually add quality?) → priority.

Scoring: **HIGH** = real visible gap, meaningful quality jump. **MED** = real but subtle. **LOW** = mostly already done / marginal.

---

## HIGH priority (genuine quality gaps)

### 5. Selection transition — instant swap is the #1 tell
- **Code reality**: `.row.selected { background: var(--sel); }` (style.css:243) has **no transition**. Row bg swaps instantly on arrow-key navigation. The only 120ms transitions are search-icon (93), #help (128), pin/add icons (534).
- **Credibility**: HIGH. Smooth selection is the single most-felt polish item in launchers; instant bg flash reads as "default Windows list". Cost: 1 line (`transition: background 100ms ease` on `.row`).
- **Note**: a crossfade also makes fast arrow-holding readable instead of strobing.

### 4. Row enter/exit stagger — results currently "appear"
- **Code reality**: zero `@keyframes` for rows anywhere in style.css. `render()` (app.js:160-260) rebuilds `li` elements with no entrance motion.
- **Credibility**: HIGH. Staggered 120ms fade+slide (6-8ms interval, ease-out) on results update is the classic "premium" signature (Raycast, Spotlight). Guard with `prefers-reduced-motion`.
- **Cost**: moderate — animation classes in render(), but no structural change.

### 13. Grid tiles are the weakest surface
- **Code reality**: `.grid-view .row` (style.css:194) = bare flex-column, **no background, no border, no radius treatment, no hover**. Icons sit on raw acrylic.
- **Credibility**: HIGH. Grid mode is where Nex shows next to other launchers (it's a headline feature — "grid view"). Flat tiles = unfinished look. Fix: `--icon-bg` fill + 8px radius + hover lift (-1px translateY + shadow), consistent with list icon rounding.
- **Cost**: low, pure CSS.

### 19. Dynamic Windows accent — free "native" feel
- **Code reality**: `--accent: #6ea8fe` dark / `#2f6bff` light (style.css:28,44) — hardcoded. No `ImmersiveColorSet` / `ImmersiveStartColor` registry read anywhere (platform.rs only reads AppsUseLightTheme).
- **Credibility**: HIGH. Matches theme-detection pattern already in place (registry read in platform.rs). Users with custom accent see their color — instant native product feel. **But**: low priority if product identity = fixed brand color (Raycast keeps fixed blue — same argument applies). Keep as "option", not default.
- **Cost**: low (registry read + CSS var injection).

---

## MEDIUM (real, subtle)

### 2. Tabular numbers in subtitles
- **Reality**: font-variant-numeric not set; path/byte subtitles (11px, style.css:233) jitter width as digits change. **Credibility**: MED — only visible in path-heavy lists.

### 7. Row hover affordance
- **Reality**: no `.row:hover` background rule exists (only #help:hover). Hover = cursor change only. **Credibility**: MED — mouse users get no hover feedback; 1 CSS rule.

### 9. Scrollbar refinement
- **Reality**: 8px scrollbar with padding-box thumb exists (style.css:173-181) — already custom. Missing: idle fade. **Credibility**: MED-LOW — current one is acceptable; fade is the refinement.

### 11. Folder placeholder icon
- **Reality**: FOLDER_ICON (app.js:46) is a generic file-page glyph, not a folder silhouette; shared shape with FILE_PLACEHOLDER. **Credibility**: MED — folders currently show a *file* icon; wrong affordance.

### 14. Grid label truncation
- **Reality**: `.grid-view .row .title` max-width 100%, single line ellipsis (style.css:300-304). No line-clamp. **Credibility**: MED — two-line app names (rare in grid since names short) — minor.

### 16. Power menu / footer polish
- **Reality**: footer #help styled (26px, radius 8); power popup has own window + shadows (style.css:553,589,632 — 0 4px 16px). **Credibility**: MED-LOW — already styled; spacing/radius alignment with rows is the remaining delta.

### 17. Launch feedback
- **Reality**: hide is instant vanish (`window.set_visible(false)` host.rs:414); no 90-120ms scale/fade-out on Enter. **Credibility**: MED — decisive vanish is *acceptable*; fade makes launch feel deliberate. Careful: adds latency to launch.

### 18. Acrylic-aware faint text
- **Reality**: `--text-faint: #76767f` on `--bg: rgba(0,0,0,0.70)` (style.css:19,25) — 70% black tint mitigates, but busy wallpapers + acrylic blur can still swallow faint text. **Credibility**: MED — legibility risk is real but partially mitigated by the 0.70 tint. Test on bright wallpaper.

---

## LOW (mostly done / marginal)

### 1. Font stack — already InterVariable
- **Reality**: InterVariable woff2 embedded + `font-weight: 100 900` + 0.2px letter-spacing (style.css:5-16, 63). **Already premium.** Remaining delta: `font-variant-numeric: tabular-nums` only.

### 3. Section header styling
- **Reality**: Files/Folders/Actions headers exist as rows (runtime_overlay_rows.rs) — styled via status/empty rules (~457). **Credibility**: LOW-MED — uppercase small-caps is a nice-to-have; current is already dim.

### 6. Icon pop-in on patchIcons
- **Reality**: placeholder→real swap (app.js:370-381) is a plain src swap. **Credibility**: MED-LOW — 120ms scale would mask the swap but swap is fast; risk of motion sickness on every keystroke (icons re-patch on each render).

### 8. Panel elevation
- **Reality**: `box-shadow: 0 8px 32px rgba(0,0,0,0.5)` already at style.css:74. **Done.**

### 10. Idle/empty states
- **Reality**: `#list:empty` + `#query::placeholder` + status text (style.css:112,169,457) all present. **Credibility**: LOW — glyph+breathing animation is garnish, current is clean.

### 12. DPI min canvas for icons
- **Reality**: pass-through <128px (icons.rs:229-237) + CSS 24px box. **Credibility**: LOW — 24px display makes native 16/32 pass-through visually adequate; content-aware upscale (v2.14.0) already solved the real complaint.

### 15. Search-icon state swap
- **Reality**: 130ms fade implemented (app.js:384-396, style.css:93). **Done.**

---

## Suggested order (impact/effort)

1. **5** selection transition (1 line)
2. **13** grid tiles (pure CSS)
3. **4** row stagger (JS animation class)
4. **7** row hover (1 rule)
5. **11** folder placeholder (asset swap)
6. **18** contrast check on bright wallpaper
7. **19** dynamic accent (optional — brand vs native)
8. **2** tabular numbers (1 rule)