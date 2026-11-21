# I Rebuilt Apple's Liquid Glass for the Web

Verbatim copy of https://www.sorrell.info/blog/liquid-glass-lens-effect (fetched
2026-11-21, images and embedded demos omitted; text unedited). Kept next to
`apple-liquid-glass-article.md` because it is the only reference that states the
per-pixel maths Nex implements: the surface-slope bend, three-channel
dispersion, the four-layer stack, and the squircle SDF that drives both the
silhouette and the bevel direction.

---

_Snell's Law, GPU shaders, and an unreasonable amount of time spent making rectangles look wet_

* * *

So Apple went and redesigned everything again.

At WWDC 2025, they unveiled **Liquid Glass**, the biggest visual overhaul since iOS 7 flattened everything back in 2013. If you've updated to iOS 26, you've already seen it: buttons, nav bars, and toolbars that look like they're made of actual glass. Not the blurry-rectangle "frosted glass" we've been doing with CSS `backdrop-filter` for years. This is different. This stuff _refracts_. It bends light. The content behind it warps and shifts as you move, like looking through a marble on your desk.

I wanted to understand how it worked. Not the marketing version ("a delightful new material"), but the actual math. The optics. So I built it. From scratch. On the web.

This post is a walkthrough of what I learned, what the physics are doing, and whether any of this is actually practical for web developers. Spoiler: it's complicated.

* * *

## OK But What Is Liquid Glass, Really?

Here's the simplest way I can describe it: Liquid Glass makes UI elements behave like physical lenses. When you put a glass lens on top of a piece of paper, the paper doesn't just get blurry. The image _bends_. The edges warp inward. Colors split apart slightly. There's a bright highlight where the light catches the curve. It looks three-dimensional.

That's what Apple is simulating. Per-pixel. In real time. On every button and tab bar on your phone.

The old approach, `backdrop-filter: blur(10px)`, is a rectangle that smudges whatever's behind it. It's cheap, it's fast, it's everywhere. But it's flat. There's no depth, no bending, no specular highlight. Liquid Glass makes your brain think there's a physical sheet of curved glass sitting between you and the content. And that distinction? It's huge. Your eye instantly reads depth and hierarchy, what's "on top" and what's "behind," without needing drop shadows or border lines.

### Why Does Apple Care This Much?

Two words: **spatial computing**. Apple has been building toward a glasses-first world (Vision Pro, and whatever comes after). In AR, flat UI panels look fake. Panels that refract and reflect their environment feel like they _exist_ in space. Liquid Glass is a rehearsal, training our eyes (and their design system) for a future where every interface hovers in mid-air.

Also, let's be honest: brand flex. Everyone can do `blur(10px)`. Nobody else is running Snell's Law in their navigation bar.

* * *

## The Physics: How Lens Refraction Works

This is the part that made me pull out my college optics notes. The core of the whole effect is **Snell's Law**:

> **n₁ sin(θ₁) = n₂ sin(θ₂)**

When light crosses from one material into another (say, air into glass) it changes direction. The amount of bending depends on the **index of refraction (IOR)** of each material. Air is 1.0. Glass is about 1.5. The steeper the angle of incidence, the more dramatic the bend.

Now imagine a **convex glass surface**, like a magnifying glass. At the center, the surface is flat, so light passes straight through with almost no bending. But at the edges, the surface curves steeply, and that's where you get maximum refraction. Content warps inward, creating that unmistakable lens distortion.

My shader models this as a circular arc. The key function computes the slope of the glass surface at any point:

```
// How steep is the glass surface at position t?
float surfaceSlope(float t) {
    float cl = clamp(t, 0.001, 0.999);
    return (1.0 - cl) / max(
        sqrt(1.0 - (1.0 - cl) * (1.0 - cl)),
        0.001
    );
}
```

At `t = 0` (the edge), this returns a steep slope. At `t = 1` (the center), it's nearly flat. Exactly like a real lens.

Then, for each pixel, I compute how much to displace its UV coordinate based on that slope, the IOR, and the bevel width:

```
float bend = slope * (1.0 - 1.0 / n) * r * bevel * 0.5;
return uv - dir * bend;
```

That `(1.0 - 1.0 / n)` term? That's Snell's Law simplified for small angles. Higher IOR means more bending. Crank it up and you get a fishbowl. Bring it close to 1.0 and the glass is basically invisible.

Here it is in action. Hover to move the lens around and see how IOR and bevel width affect the refraction:

* * *

## The Four Layers of Liquid Glass

Refraction alone isn't enough to sell the illusion. Apple's effect (and my implementation) stacks four distinct optical layers, all computed in a single shader pass. Here's what each one does:

### Layer 1: Refraction (Snell's Law)

The foundation. Each pixel inside the lens gets its UV coordinate remapped based on the surface slope. Background content warps inward at the edges, creating the magnifying-glass look. This is where 80% of the "wow" comes from.

### Layer 2: Chromatic Aberration (Dispersion)

Real glass doesn't bend all colors equally. Red, green, and blue light each refract at slightly different angles. I run three separate refraction passes with offset IOR values and composite the channels: `half3(colR.r, colG.g, colB.b)`. The result is those subtle rainbow fringes at the edges that make it feel optically real.

```
float iorR = ior - dispersion * 0.02;
float iorG = ior;
float iorB = ior + dispersion * 0.02;
```

Each color channel samples from a slightly different refracted position. It's a small detail that does a ton of heavy lifting.

### Layer 3: Frosted Blur (Multi-Tap Sampling)

Apple's glass isn't crystal clear. It's slightly frosted. I sample a ring of 8 points around each refracted UV, weight the center sample 2×, and average them:

```
for (int i = 0; i < TAPS; i++) {
    float a = float(i) * 6.2831853 / float(TAPS);
    float2 off = float2(cos(a), sin(a)) * spread;
    acc += scene(uv + off);
}
```

It softens the refracted image just enough to feel like textured glass without obliterating the content behind it.

### Layer 4: Specular Highlights (Fresnel Reflection)

At grazing angles, glass reflects more light than it transmits. That's the Fresnel effect. A virtual light direction creates a bright specular dot, and the rim of the lens glows:

```
float spec = pow(max(dot(dir, lightDir), 0.0), specPower) * edgeFactor;
float rim = pow(edgeFactor, 2.5) * 0.4;
```

This is what makes it feel like there's an actual light source in the room hitting the glass.

Individually, each layer is a well-understood optical phenomenon. But stack all four together, animate them at 60fps, and suddenly you've got something that makes people do a double-take. That's the magic. It's not one trick, it's the orchestration.

* * *

## Squircles: Apple's Secret Favorite Shape

One more thing before we talk about the web implementation. You know those rounded rectangles on every Apple product? They're not actually `border-radius` rounded rectangles. They're **superellipses**, affectionately called "squircles."

> **|x/a|ⁿ + |y/b|ⁿ = 1**

When _n = 2_, you get a regular ellipse. At _n = 4_, you get Apple's signature squircle. At _n → ∞_, it's a rectangle.

Why does Apple care about this instead of just using `border-radius`? Because a CSS rounded rectangle has a **discontinuous curvature**: there's an abrupt transition where the curved corner meets the straight edge. A squircle has continuous curvature everywhere. It's smoother, more organic, and your eye can't quite tell where the curve starts. It just flows.

In the frosted glass panel demo, I use a signed distance function (SDF) to compute the squircle shape, and the same SDF gradient drives both the shape mask _and_ the bevel refraction direction. One math function, two purposes. Efficient and elegant (if I say so myself).

```
float sdSquircle(float2 p, float2 size, float n) {
    float2 d = abs(p) / size;
    float v = pow(pow(d.x, n) + pow(d.y, n), 1.0 / n);
    return (v - 1.0) * min(size.x, size.y);
}
```

* * *

## Building This for the Web

Now for the part that gave me the most headaches: actually shipping this in a browser.

**The stack:** React Native + Expo for cross-platform (yes, the same codebase runs on iOS, Android, and web). `@shopify/react-native-skia` for GPU-accelerated rendering. SkSL (Skia Shading Language) for the fragment shaders, which compile down to GPU code. And on web, CanvasKit WASM, which is Skia compiled to WebAssembly so it runs in the browser.

Here's the catch, and it's a big one: Skia's web backend doesn't implement `RuntimeShader` as an image filter. On native, you can wrap a group of React components in a shader that samples their rendered pixels. On web, you can't.

My solution was to make the shader fully self-contained. The background scene is generated _procedurally inside the shader itself_, so I use `Shader` as a paint on a `Fill` element instead of `RuntimeShader` wrapping a component tree. Everything happens in one GPU pass. This is actually closer to how Apple likely does it natively: keep it all on the GPU, don't bounce between CPU and GPU.

The CanvasKit WASM binary is about **8MB**. That's not nothing. It loads asynchronously, and nothing Skia-related can render until it's ready. I built a `useSkiaReady()` hook that gates rendering. You get a loading state until the WASM is hydrated, then everything pops in.

For interaction, it's mouse-follow on hover with a click-to-pin toggle. The position flows from React state through shared values into shader uniforms, updated every frame. On web it's `onMouseMove`; on native it's `onResponderMove`. Platform-branched, but the shader code is identical.

* * *

## Can You Actually Use This in Production?

Okay, real talk. This is the section where we get honest.

### For a hero element or landing page?

**Absolutely yes.** The 8MB WASM is a one-time download. A single shader-powered glass card as your centerpiece? That's compelling, distinctive, and totally justifiable. Nobody's bouncing from your landing page because of an 8MB asset if the experience is worth it.

### For every card in a list?

**Nope.** Each element needs its own canvas, its own shader pass, and access to the background behind it. The web doesn't have a native "give me the rendered pixels behind this element" API. Apple can do this because they control the entire rendering pipeline: compositor, Metal shaders, the display server. They can sample any rendered layer as a texture input for any other layer's shader. On the web, we have isolated `<canvas>` elements and CSS, and those two worlds don't share pixel data easily.

### The realistic middle ground

Use `backdrop-filter` for your bulk UI glass effects. It's cheap, it's composited by the browser, it works everywhere. Reserve the full shader treatment for one or two signature interactive elements where you want someone to stop scrolling and go "wait, how did they do that?"

The CSS ecosystem just doesn't have the plumbing for this yet. The closest web standards, CSS `element()` (abandoned) and Houdini paint worklets (limited), never got far enough. Until browsers give us a way to sample rendered pixels behind an element, we're working around the architecture, not with it.

* * *

## Liquid Glass: The Honest Verdict

After spending way too many hours staring at shader code and reading about Fresnel equations, here's where I've landed:

**The Good:**

*   Creates genuine depth and spatial hierarchy. Your brain just _gets_ what's in front.
*   Responds to motion and context in a way that feels alive, not static
*   Makes UI feel physical and premium
*   Primes the design language for AR/glasses interfaces
*   When it's done well, it's genuinely beautiful

**The Not-So-Good:**

*   Computationally expensive. Apple dedicates serious GPU budget to this.
*   Accessibility is a real concern: refraction + transparency can tank readability
*   The effect depends on the background. Boring background = boring glass.
*   Not great for dense text areas where you need to actually _read_ things
*   The design community is split. Some call it visual noise.

Apple reportedly had to adjust Liquid Glass transparency multiple times during the iOS 26 beta after legibility complaints. That tells you something: even with total control over the pipeline, it's a balancing act. And they've made it clear this isn't going away. Liquid Glass is expanding and will be mandatory for developers in future Xcode versions.

My take? It's a bet on the spatial computing future. If you squint (sorry), you can see the through-line from this to the kind of UI that would feel right hovering in front of your face on Apple glasses. Whether it's the right trade-off for a phone screen _today_ is a different conversation. But the craft behind it? Undeniable.

* * *

## Go Break Some Glass

The demo has sliders for IOR, dispersion, blur, and specular power. Crank the IOR to 3.0 and watch things get weird. That's half the fun.

It runs on iOS and Android too via Expo, same codebase, same shaders, same physics.

**[Try all the demos →](https://kennsorr.github.io/glass-lens-effect/)** · **[View on GitHub](https://github.com/kennsorr/glass-lens-effect)**

* * *

## References

*   [Snell's Law — Wikipedia](https://en.wikipedia.org/wiki/Snell%27s_law) — The fundamental optics equation governing how light bends when crossing material boundaries, and the core math behind the refraction shader.
*   [Superellipse — Wikipedia](https://en.wikipedia.org/wiki/Superellipse) — The mathematical shape behind Apple's squircles, including the parametric equation and how exponent values control the curvature.
*   [React Native Skia — Shopify](https://github.com/Shopify/react-native-skia) — The library used for GPU-accelerated rendering in React Native, including SkSL shader support and the Skia canvas API.
*   [CanvasKit — Skia](https://skia.org/docs/user/modules/canvaskit/) — Skia's WebAssembly module that powers the browser runtime for the demos, including shader compilation and canvas rendering.
*   [Fresnel Equations — Wikipedia](https://en.wikipedia.org/wiki/Fresnel_equations) — The physics behind how reflectivity changes at grazing angles, used for the specular highlight and rim glow layers.
