// glass.hlsl - optical model and presets after tomacco/amazing-glass (MIT, (c) 2026 Ivan Tomac)
//
// Written here from the numbers and formulas of reference-sheet-amazing.md (its "Corrections" section applied); no code is copied.
// Used by rf_win::glass. Differences to the experiment this grew out of: in a window whose corner radius is smaller than the rim, the
// direction of the bend comes from a rounder box than the outline while its depth keeps the true outline (see "DEPTH AND DIRECTION" in
// PSGlass); a window's corners are not cut out here (uOpt.x); and the footprint uses the fine derivatives (see "ddx_fine" there).
// One pixel shader (PSGlass) does the whole look per pixel; PSCrop and PSDown only prepare the backdrop (crop + mip chain for frost).
//
// UNITS. The shape, rim width, thickness, shifts, blur and line widths are CSS px (96 dpi). The window is in device px; s = uShape.w is
// device px per CSS px. Geometry is computed in CSS px (p = device offset / s) and converted back to device px for the texture taps.
// y points DOWN everywhere (screen coordinates), so the light direction (cos a, sin a) with a = -135 deg is the top left.
//
// COLOUR SPACE. Everything is sRGB (gamma) encoded values, like the SVG filter chain of the original (no linear light): the mirror of
// the screen is read as plain UNORM, the mip chain is built from encoded values, grade / tint / rim light work on encoded values.
//
// FROST. The original blurs the backdrop with a Gaussian of deviation sigma (CSS px) BEFORE the displacement. Here the backdrop has a mip
// chain built with the 13-tap downsample of PSDown (Jimenez). Variance of that filter, worked out from its taps: every level adds
// 1.5 (tap offsets) + 4/12 (the 2 x 2 box each bilinear tap covers) = 1.83 texel^2 of the previous level, so the chain up to level L has a
// variance of 0.61 * (4^L - 1) px^2, and reading it with bilinear filtering adds about 4^L / 12. That is sigma ~ 0.83 * 2^L, therefore
//   level L = log2(max(1, sigma_device / 0.83)) = log2(max(1, 1.2 * sigma_device)),  sigma_device = sigma_css * s,
// fractional levels are blended by the trilinear sampler. (The sheet's own guess 2.5 * sigma came from the repo's WebGL path, which has a
// different mip chain; with this chain 1.2 is the number that matches a Gaussian in variance.)

cbuffer CB : register(b0)
{
    float4 uWin;    // xy: window size, device px           zw: region texture size px (mip 0)
    float4 uReg;    // xy: crop origin in the source px     zw: source texture size px
    float4 uShape;  // xy: half size CSS px   z: corner radius CSS px   w: s (device px per CSS px)
    float4 uA;      // x: depth (thickness slider)  y: bezel (rim slider, fraction of the short side)  z: maxBezel px  w: zoom
    float4 uB;      // x: dispersion (rainbow)  y: blur sigma CSS px (frost)  z: saturate  w: lum
    float4 uC;      // x: contrast  y: rim gain  z: rimWidth px  w: shade (inner dark line)
    float4 uD;      // x: light angle deg  y: outer shadow opacity at the outline (0 = none)  z: shadow offset down CSS px  w: how far the shadow reaches, CSS px
    float4 uTint;   // rgb, a
    float4 uPass;   // xy: source texel size (down pass)  zw: output size px (down pass)
    float4 uExt;    // x: non-finite check 0/1  y: debug 0 = normal, 1 = write raw offsets, 2 = no rim shift (zoom term only)  z: region pad px
                    // w: 1 = point sampling of the sharp backdrop (what the screenshot shows), 0 = bilinear
    float4 uSub;    // xy: shift of the glass centre from the window centre, device px (sub pixel placement; also where a SHAPE sits inside a larger window)
                    // zw: extra shift of the outline only (test placement)
    float4 uOpt;    // x: 1 = the corners are not cut out here (the whole rectangle is covered; a window's corners are cut by the system)
                    // y: share of the frost left at the very edge (it grows to the whole frost across the rim): less frost, more visible bending
    float4 uLensA;  // x: number of lenses (0: the lens loop does not run)  y: frost sigma CSS px  z: zoom  w: thickness (share of the rim width)
    float4 uLensB;  // x: rim width max CSS px  y: rim width as a share of the lens's short side  z: brightness  w: highlight line gain
    float4 uLensC;  // x: dark line gain  y: shadow opacity (0 = none)  z: shadow offset y CSS px  w: shadow blur CSS px
    float4 uLensT;  // tint rgb, a
    float4 uLens[24]; // lens i: [i] centre xy + half size zw, device px   [MAX_LENSES + i] clip rectangle x0 y0 x1 y1, device px   [2 * MAX_LENSES + i] x: radius device px  y: opacity
};

static const float IOR = 1.5;                                  // refraction index, constant in all three modes
static const float3 LUMA_W = float3(0.2126, 0.7152, 0.0722);   // luma weights of the repo's GLSL grade
static const float MAX_LOD = 8.0;
static const uint MAX_LENSES = 8;                              // keep equal to MAX_LENSES in mod.rs

Texture2D<float4> tA : register(t0);   // crop pass: the source; others: the region pyramid
SamplerState sLin : register(s0);      // linear, clamp
SamplerState sAni : register(s1);      // anisotropic (16), clamp: for the rim, where the image is squeezed in one direction

// SMOOTH SAMPLING (the default; uExt.w = 0). The browser's displacement filter reads whole pixels, which is what makes the original
// look pixelated: letters under the 1.35 x zoom get uneven 1 / 2 px steps and the squeezed rim sparkles. Here instead:
//   - where the image is enlarged or 1 : 1, a Catmull-Rom (bicubic) read of the sharp level: smooth letters, no softness of bilinear;
//   - where it is squeezed (the rim), an anisotropic read whose footprint is the real one (ddx / ddy of the read position);
//   - with frost, the trilinear read of the blur level, as in point mode.
// Catmull-Rom of mip 0 at region px q, in 9 bilinear taps (the 16 weights folded pairwise).
float3 TapCubic(float2 q)
{
    float2 c1 = floor(q - 0.5) + 0.5;
    float2 f = q - c1;
    float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    float2 w3 = f * f * (-0.5 + 0.5 * f);
    float2 w12 = w1 + w2;
    float2 t0 = (c1 - 1.0) / uWin.zw;
    float2 t3 = (c1 + 2.0) / uWin.zw;
    float2 t12 = (c1 + w2 / w12) / uWin.zw;
    float3 r = tA.SampleLevel(sLin, float2(t0.x, t0.y), 0).rgb * (w0.x * w0.y);
    r += tA.SampleLevel(sLin, float2(t12.x, t0.y), 0).rgb * (w12.x * w0.y);
    r += tA.SampleLevel(sLin, float2(t3.x, t0.y), 0).rgb * (w3.x * w0.y);
    r += tA.SampleLevel(sLin, float2(t0.x, t12.y), 0).rgb * (w0.x * w12.y);
    r += tA.SampleLevel(sLin, float2(t12.x, t12.y), 0).rgb * (w12.x * w12.y);
    r += tA.SampleLevel(sLin, float2(t3.x, t12.y), 0).rgb * (w3.x * w12.y);
    r += tA.SampleLevel(sLin, float2(t0.x, t3.y), 0).rgb * (w0.x * w3.y);
    r += tA.SampleLevel(sLin, float2(t12.x, t3.y), 0).rgb * (w12.x * w3.y);
    r += tA.SampleLevel(sLin, float2(t3.x, t3.y), 0).rgb * (w3.x * w3.y);
    return saturate(r);
}

// q: read position in region px; gx, gy: its change per output pixel; major: the longer of the two; lodFrost: blur level (0 = sharp).
float3 TapSmooth(float2 q, float2 gx, float2 gy, float major, float lodFrost)
{
    // Frost keeps the plain trilinear read of the blur level (an anisotropic read sharpens wherever the footprint is thin, e.g. in the
    // corners where the rays converge, and the blur must never get weaker than asked); only a footprint more than twice as wide as
    // the blur lifts the level. Under half a level of frost the two reads are blended, so the Frost slider has no jump at zero.
    float2 uv = q / uWin.zw;
    float w = saturate(lodFrost * 2.0);
    float3 fr = 0, sh = 0;
    if (w > 0.0)
        fr = tA.SampleLevel(sLin, uv, max(lodFrost, log2(max(major, 1.0)) - 1.0)).rgb;
    if (w < 1.0)
    {
        sh = tA.SampleGrad(sAni, uv, gx / uWin.zw, gy / uWin.zw).rgb;
        if (major < 1.5)
            sh = lerp(TapCubic(q), sh, smoothstep(1.05, 1.5, major));
    }
    return lerp(sh, fr, w);
}

float4 VSMain(uint id : SV_VertexID) : SV_Position
{
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

// Pass 1: copy the window rectangle (plus margin) out of the source. The clamp sampler repeats the edge pixels where the region sticks out.
float4 PSCrop(float4 pos : SV_Position) : SV_Target
{
    return tA.SampleLevel(sLin, (uReg.xy + pos.xy) / uReg.zw, 0);
}

// Pass 2: 13 tap downsample (Jimenez, "Next generation post processing in Call of Duty: Advanced Warfare"), one level of the chain.
float4 PSDown(float4 pos : SV_Position) : SV_Target
{
    float2 uv = pos.xy / uPass.zw;
    float2 t = uPass.xy;
    float4 e = tA.SampleLevel(sLin, uv, 0);
    float4 a = tA.SampleLevel(sLin, uv + t * float2(-2,  2), 0);
    float4 b = tA.SampleLevel(sLin, uv + t * float2( 0,  2), 0);
    float4 c = tA.SampleLevel(sLin, uv + t * float2( 2,  2), 0);
    float4 d = tA.SampleLevel(sLin, uv + t * float2(-2,  0), 0);
    float4 f = tA.SampleLevel(sLin, uv + t * float2( 2,  0), 0);
    float4 g = tA.SampleLevel(sLin, uv + t * float2(-2, -2), 0);
    float4 h = tA.SampleLevel(sLin, uv + t * float2( 0, -2), 0);
    float4 i = tA.SampleLevel(sLin, uv + t * float2( 2, -2), 0);
    float4 j = tA.SampleLevel(sLin, uv + t * float2(-1,  1), 0);
    float4 k = tA.SampleLevel(sLin, uv + t * float2( 1,  1), 0);
    float4 l = tA.SampleLevel(sLin, uv + t * float2(-1, -1), 0);
    float4 m = tA.SampleLevel(sLin, uv + t * float2( 1, -1), 0);
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

// STEP 0. Rounded box: half sizes h, corner radius r (circle: h = r). p from the centre. Returns d (negative inside) and the outward unit normal.
float ShapeDist(float2 p, float2 h, float r, out float2 n)
{
    float2 q = abs(p) - (h - r);
    float2 sg = float2(p.x >= 0.0 ? 1.0 : -1.0, p.y >= 0.0 ? 1.0 : -1.0);
    if (q.x > 0.0 && q.y > 0.0)
    {
        float l = length(q);
        n = q / l * sg;
        return l - r;
    }
    n = (q.x > q.y) ? float2(sg.x, 0.0) : float2(0.0, sg.y);
    return max(q.x, q.y) - r;
}

// STEP 2. Height profile of the rim: a squircle, vertical at the edge (t = 0), flat on top (t = 1). 1 - (1-t)^4 written without the cancellation.
float HeightProfile(float t)
{
    float q = t * (4.0 - t * (6.0 - t * (4.0 - t)));
    return pow(max(q, 0.0), 0.25);
}

// STEP 2. Slope -> incidence angle -> Snell's law -> sideways shift of the ray over the glass thickness T, in CSS px. No table, direct formula.
float RimShift(float t, float B, float T)
{
    if (t >= 1.0) return 0.0;
    float t0 = max(t, 0.001);                       // the small clamp that keeps the very edge finite
    float t1 = min(t0 + 0.001, 1.0);
    float slope = (HeightProfile(t1) - HeightProfile(t0)) / 0.001 * T / B;
    float inc = atan(slope);
    float refr = asin(sin(inc) / IOR);
    return T * tan(inc - refr);
}

float SigmaToLod(float sigmaDevice)
{
    return min(log2(max(1.0, 1.2 * sigmaDevice)), MAX_LOD);
}

// ---------------------------------------------------------------------------------------------------------------------------------
// LENSES: pieces of clear glass on the window's glass (the selection pill of a list). Drawn after the window's own colour, in slot order.
// The same optics as the window (HeightProfile / RimShift), reading the same backdrop pyramid, but nearly without frost: the lens is the
// sharp, clear piece in the frosted pane. A lens is a rounded rectangle with a 1 px anti aliased outline, its coverage is multiplied by the
// clip rectangle's coverage and by its opacity.
// There are no screen derivatives here (the loop is under flow control that depends on the lens count), so every read is SampleLevel, and
// its level is the frost plus a term that grows towards the edge, where the picture is squeezed: the footprint of the read position there
// is |1 + d(shift)/d(depth)| pixels per pixel along the normal and 1 along the edge; half of that (in levels) is the geometric mean.
// ---------------------------------------------------------------------------------------------------------------------------------

// Distance (device px, negative inside) to a box with centre c and half size h.
float BoxDist(float2 p, float2 c, float2 h)
{
    float2 q = abs(p - c) - h;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
}

// base: premultiplied colour so far.
float4 ApplyLens(uint i, float4 base, float2 pos)
{
    const float s = uShape.w;
    float4 g = uLens[i];
    float4 cl = uLens[MAX_LENSES + i];
    float2 ro = uLens[2 * MAX_LENSES + i].xy;
    float opacity = ro.y;
    float2 p = (pos - g.xy) / s;                    // CSS px from the lens centre
    float2 h = g.zw / s;
    float hmin = min(h.x, h.y);
    float r = min(ro.x / s, hmin);
    float B = max(1.0, min(uLensB.x, 2.0 * hmin * uLensB.y));   // rim width: min(10 px, 0.45 * the short side)
    float T = B * uLensA.w;

    // depth into the lens from the true outline, direction from the smooth one (as in PSGlass; a capsule has the same one)
    float rO = min(max(r, B), hmin);
    float2 n, nT;
    ShapeDist(p, h, rO, n);
    float D = -ShapeDist(p, h, r, nT);
    float cov = saturate(D * s + 0.5);              // the lens's own coverage, a ramp one pixel wide
    float2 cc = (cl.xy + cl.zw) * 0.5;
    float clipCov = saturate(0.5 - BoxDist(pos, cc, (cl.zw - cl.xy) * 0.5));

    // the soft shadow on the glass around the lens: the shape moved down, blurred (logistic ~ Gaussian, as the window's shadow); it is
    // clipped like the lens, and does not show through a lens that is only partly opaque, so it is taken outside the shape's own coverage
    float sh = 0.0;
    if (uLensC.y > 0.0)
    {
        float2 n2;
        float d2 = ShapeDist(p - float2(0.0, uLensC.z), h, r, n2);
        float sig = max(uLensC.w * 0.5, 0.01);
        sh = uLensC.y / (1.0 + exp(clamp(1.702 * d2 / sig, -30.0, 30.0))) * (1.0 - cov) * clipCov * opacity;
    }
    float4 o = base;
    o.rgb *= 1.0 - sh;
    o.a = o.a + sh * (1.0 - o.a);

    float covL = cov * clipCov * opacity;
    if (covL > 0.0)
    {
        float t = saturate(D / B);
        float rim = RimShift(t, B, T);
        float2 v = -n * rim - p * (1.0 - 1.0 / uLensA.z);
        // squeeze of the picture at this depth: |1 + d rim / d depth| along the normal (rim falls with depth, so the sum is a difference)
        float t1 = max(t, 0.002), t2 = min(t1 + 0.02, 1.0);
        float dRim = (RimShift(t2, B, T) - RimShift(t1, B, T)) / (max(t2 - t1, 1e-4) * B);
        float squeeze = max(1.0, abs(1.0 + dRim));
        float lod = min(SigmaToLod(uLensA.y * s) + 0.5 * log2(squeeze), MAX_LOD);
        float2 uv = (pos + uExt.z + v * s) / uWin.zw;
        float3 c = tA.SampleLevel(sLin, uv, lod).rgb;

        // the window's grade, then brighter, then the lens's own tint
        float luma = dot(c, LUMA_W);
        c = saturate(lerp(float3(luma, luma, luma), c, uB.z));
        c = saturate(c * (uB.w * uC.x) + 0.5 * (1.0 - uC.x));
        c = saturate(c * uLensB.z);
        c = lerp(c, uLensT.rgb, uLensT.a);

        // light: a highlight line just inside the edge, strongest on the side the light comes from, a soft glow behind it, and a faint
        // dark line further in on the opposite side
        float ang = radians(uD.x);
        float facing = dot(n, float2(cos(ang), sin(ang)));
        float lit = pow(max(0.0, facing), 1.6);
        float back = pow(max(0.0, -facing), 1.6);
        float Dd = D * s;                           // device px
        float rw = max(0.9 * s, 1.0);
        float aa = saturate(Dd + 1.0);
        float edgeLine = exp(-((Dd - 0.8 * rw) / rw) * ((Dd - 0.8 * rw) / rw)) * (0.25 + 0.75 * lit);
        float glow = exp(-Dd / (0.5 * B * s)) * 0.2 * lit;
        c = lerp(c, float3(1, 1, 1), min(1.0, (edgeLine + glow) * aa * uLensB.w));
        float dz = (Dd - 2.4 * rw) / (1.2 * rw);
        c = lerp(c, float3(0, 0, 0), exp(-dz * dz) * back * aa * uLensC.x);

        o.rgb = c * covL + o.rgb * (1.0 - covL);
        o.a = covL + o.a * (1.0 - covL);
    }
    return o;
}

float4 PSGlass(float4 pos : SV_Position) : SV_Target
{
    const float s = uShape.w;
    float2 pw = pos.xy - uWin.xy * 0.5 - uSub.xy;   // device px from the centre
    float2 p = pw / s;                              // CSS px
    float2 h = uShape.xy;
    float r = min(uShape.z, min(h.x, h.y));
    // Rim width B (CSS px). Needed before the optics now, see the next comment.
    float B = max(4.0, min(uA.z, min(2.0 * h.x, 2.0 * h.y) * uA.y));
    // DEPTH AND DIRECTION. A window's corner radius (8 px) is far smaller than the rim width B (about 30 px).
    //   - The DEPTH into the glass (how far from the edge: it sets how much a ray is bent) is the distance to the TRUE outline, radius r.
    //     The bent band then has the same width all the way round and hugs the window's real corner.
    //   - The DIRECTION a ray is bent in is the normal of a rounder box (radius at least B, capped at the half sizes). The true outline's
    //     own normal jumps from one side to the other along the corner diagonal, a visible seam in a band that wide; the rounder box's
    //     turns smoothly through the corner.
    // (An earlier version took the depth from the rounder box too. The band then followed a 30 px curve inside an 8 px corner: a second,
    // rounder outline showed inside the window and the wedge between the two read the backdrop from one far place.)
    // The rim light's DIRECTION (which side is lit) also comes from the smooth normal: with the outline's own, a light from one corner
    // showed a faint 45 degree line in the corners it does not face.
    float rO = min(max(r, B), min(h.x, h.y));
    float2 n, nTrue;
    ShapeDist(p, h, rO, n);
    float D = -ShapeDist(p, h, r, nTrue);           // depth into the glass, CSS px
    float2 nE;                                      // outline: coverage, rim light and shadow use the outline's own centre (uSub.zw, zero in normal use); its normal is not used
    float2 pE = (pw - uSub.zw) / s;
    float DE = -ShapeDist(pE, h, r, nE);
    // STEP 7: edge coverage. Point mode: crisp, a pixel whose centre is 0.5 px or more inside is fully covered (the screenshot's clip).
    // Smooth mode: a ramp one pixel wide centred on the contour (the pixel's covered share), so the outline has no steps.
    float cov = (uExt.w > 0.5) ? saturate(DE * s * 2.0) : saturate(DE * s + 0.5);
    // A window: the system cuts the corners itself, to its own curve. Cutting them here too leaves the pixels between the two curves empty.
    if (uOpt.x > 0.5) cov = 1.0;

    // The read position without dispersion and its footprint (how far it moves per output pixel). Taken here, outside every branch,
    // for all pixels: screen derivatives are not allowed inside varying flow control. Outside the shape t = 0, so v stays continuous.
    float2 v0 = -n * ((uExt.y == 2.0) ? 0.0 : RimShift(saturate(D / B), B, B * uA.x));
    if (uA.w != 1.0) v0 += -p * (1.0 - 1.0 / uA.w);
    float2 q0 = pos.xy + uExt.z + v0 * s;
    // ddx_fine / ddy_fine (the pixel's own row pair / column pair), not ddx / ddy: those are allowed to take one value for the whole 2 x 2
    // quad, in practice the top row's and the left column's, so the footprint in the bent corners differed between the left and the right
    // side of a symmetric window (up to 20 of 255).
    float2 gx = ddx_fine(q0), gy = ddy_fine(q0);
    float major = max(length(gx), length(gy));
    float glim = 48.0 / max(48.0, major);           // a jump of the normal (a seam) must not smear the whole pyramid in
    gx *= glim; gy *= glim; major *= glim;

    // STEP 7: outer drop shadow, for a glass that is a shape inside a larger transparent window (a dock island; an ordinary window has the
    // system's own). The shape moved down by uD.z px; d2 is the distance of this pixel from that outline (negative inside it). The shadow
    // is uD.y (the opacity AT the outline) up to the outline and falls to nothing over uD.w px outside it, along a smoothstep, so it has no
    // visible edge at either end. It is only ever drawn outside the glass: (1 - cov) takes it off the covered pixels, so the glass is as
    // clear as without it. With uD.y = 0 (everything but a dock island) none of this runs. Mirrored on the CPU by `Shadow::alpha` (tests).
    float sh = 0.0;
    // (a real branch: left to itself the compiler flattens it and every pixel of an ordinary window pays for the shadow's maths)
    [branch] if (uD.y > 0.0)
    {
        float2 n2;
        float d2 = ShapeDist(pE - float2(0.0, uD.z), h, r, n2);
        float t = saturate(d2 / max(uD.w, 0.01));
        sh = uD.y * (1.0 - t * t * (3.0 - 2.0 * t)) * (1.0 - cov);
    }

    float3 col = 0;
    if (cov > 0.0)
    {
        // STEP 1: rim width B and thickness T
        float T = B * uA.x;
        float t = saturate(D / B);

        // STEP 2 + 3: rim shift (inward) and zoom term; dispersion scales the WHOLE vector per channel
        float rim = (uExt.y == 2.0) ? 0.0 : RimShift(t, B, T);
        float2 v = -n * rim;
        if (uA.w != 1.0) v += -p * (1.0 - 1.0 / uA.w);
        if (uExt.y == 1.0) return float4(v.x, v.y, rim, B);

        float2 rp = pos.xy + uExt.z;                // this pixel in region px
        // STEP 4: frost, the blur is in the region pyramid, taken before the displacement.
        // Deviation from the sheet: the outermost ring samples a sharper mip (factor uOpt.y at the edge, 1 where the band ends) so the rim bending stays visible.
        float lod = SigmaToLod(uB.y * s) * lerp(uOpt.y, 1.0, smoothstep(0.0, 1.0, t));
        float3 k = float3(1.0 + uB.x, 1.0, 1.0 - uB.x);
        float3 c;
        if (uExt.w > 0.5 && lod < 0.001)
        {
            // POINT sampling: the displacement filter of the browser reads the source pixel that holds the displaced position (rounded to a
            // whole pixel), per channel. The screenshot shows it: 1 px lines stay 1 px crisp and the colour fringe is made of pure channel steps
            // ((39,255,255) next to (60,60,255)), which bilinear taps cannot give. With a blurred backdrop (lod > 0) bilinear is used.
            int2 hi = int2(uWin.zw) - 1;
            // the browser's 8-bit map cannot show a red/blue gap under about half a pixel (0.25 px per side) is dropped (identical taps)
            float2 dd = v * (uB.x * s);
            dd = float2(abs(dd.x) < 0.25 ? 0.0 : dd.x, abs(dd.y) < 0.25 ? 0.0 : dd.y);
            float2 vs = v * s;
            c.r = tA.Load(int3(clamp(int2(floor(rp + vs + dd)), int2(0, 0), hi), 0)).r;
            c.g = tA.Load(int3(clamp(int2(floor(rp + vs)), int2(0, 0), hi), 0)).g;
            c.b = tA.Load(int3(clamp(int2(floor(rp + vs - dd)), int2(0, 0), hi), 0)).b;
        }
        else if (uExt.w > 0.5)
        {
            // point mode with frost: plain trilinear taps of the blur level, as before
            c.r = tA.SampleLevel(sLin, (rp + v * (k.r * s)) / uWin.zw, lod).r;
            c.g = tA.SampleLevel(sLin, (rp + v * (k.g * s)) / uWin.zw, lod).g;
            c.b = tA.SampleLevel(sLin, (rp + v * (k.b * s)) / uWin.zw, lod).b;
        }
        else
        {
            c.r = TapSmooth(rp + v * (k.r * s), gx, gy, major, lod).r;
            c.g = TapSmooth(rp + v * (k.g * s), gx, gy, major, lod).g;
            c.b = TapSmooth(rp + v * (k.b * s), gx, gy, major, lod).b;
        }

        // STEP 4: saturation, then lum * contrast as one linear transfer (SVG feColorMatrix + feComponentTransfer, each clamped)
        float luma = dot(c, LUMA_W);
        c = saturate(lerp(float3(luma, luma, luma), c, uB.z));
        c = saturate(c * (uB.w * uC.x) + 0.5 * (1.0 - uC.x));
        // STEP 5: tint
        c = lerp(c, uTint.rgb, uTint.a);

        // STEP 6: rim light and the inner dark line
        float ang = radians(uD.x);
        float2 L = float2(cos(ang), sin(ang));
        float facing = dot(n, L);
        float lit = pow(max(0.0, facing), 1.6);
        float back = pow(max(0.0, -facing), 2.2) * 0.55;
        float Dd = DE * s;                          // device px
        float rw = max(uC.z * s, 1.0);              // line at least 1 device px wide (the sheet's 0.3 px hairline is under a pixel; deliberate deviation)
        float glow = 0.55 * B * s;
        float aa = saturate(Dd + 1.0);
        float ln = exp(-(Dd / rw) * (Dd / rw)) * (0.22 + 0.78 * (lit + back));
        float soft = exp(-Dd / glow) * 0.22 * (lit + 0.6 * back);
        float alpha = min(1.0, (ln + soft) * aa * uC.y);
        float dz = (Dd - 1.6 * rw) / (0.9 * rw);
        float shade = exp(-dz * dz) * uC.w * aa * (1.0 - lit);
        c = lerp(c, float3(1, 1, 1), alpha);
        c = lerp(c, float3(0, 0, 0), shade);
        col = c;
    }

    float a = cov + sh;                             // the shadow is black: premultiplied colour = glass * coverage
    if (uExt.x > 0.5 && (any(isnan(col)) || any(isinf(col)) || isnan(a))) return float4(1, 0, 1, 1);
    float4 o = float4(col * cov, a);
    // the lenses, in slot order; with none (count 0) the picture is exactly the window's own
    // A real loop, not unrolled: ApplyLens is large and eight copies of it would make the shader slow to compile for no gain (the count
    // is a constant of the draw, so every pixel takes the same path). With none the loop does not run at all.
    uint nLens = min((uint)uLensA.x, MAX_LENSES);
    [loop] for (uint i = 0; i < nLens; i++)
    {
        if (uLens[2 * MAX_LENSES + i].y > 0.0)
            o = ApplyLens(i, o, pos.xy);
    }
    if (uExt.x > 0.5 && (any(isnan(o)) || any(isinf(o)))) return float4(1, 0, 1, 1);
    return o;
}
