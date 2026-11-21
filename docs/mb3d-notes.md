# Mandelbulb3D internals (notes for the port)

Gathered from the Delphi source in `../mb3d`. Offsets are bytes; all records are packed
little-endian.

## Parameter files (.m3p) and text parameters

- `.m3p` = `TMandHeader10` (840 bytes) + `THeaderCustomAddon` (1136 bytes, 8-byte head + 6
  formula slots of 188 bytes). MandId (first i32) 20..44 supported; 45 (test build) has 8 extra
  header bytes (light record at 440) and 8 slots.
- Header fields used: Width 4, Height 8, Iterations 12, dZstart 20, dZend 28, dXmid/dYmid/dZmid
  36/44/52, 4D rotations 60/68/76, dZoom 84, RStop 92 (bailout radius), dFOVy 108, hard shadow
  flags 133, refine steps 134, AO flags 149, cut flags 176, sDEstop 177 (f32), DOF type 181,
  mZstepDiv 182 (f32), bIsJulia 190, dJx..dJw 191..215, hVGrads 246 (3x3 f64, rows right, down,
  forward; renormalise; quaternion if MandId > 43 and byte 18 bit 0), cut positions 346/354/362,
  authors 382 (2 x 16 chars x 7 bit), version f32 424, lighting record 432 (408 bytes).
- Text format: `Mandelbulb3Dv18{...}`, radix-64 alphabet
  `./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz`, 3 bytes -> 4 chars, first
  char = lowest 6 bits of `b0 | b1<<8 | b2<<16`. 840 header bytes, then the addon truncated to
  the used slots. Title in `{Titel: ...}`.
- Formula slot: iItCount 0 (i32), iFnr 4 (0..9 internal, >= 20 external), iOptionCount 8,
  CustomFname 12 (32 bytes), option types 44 (16 bytes), option values 60 (16 f64). Internal
  names: Integer Power, Real Power, Quaternion, Tricorn, Amazing Box, Bulbox, Folding Int Pow,
  test, testIFS, Aexion C.
- Addon head: version 0 (16), hybrid type 1 (&3: alternating, interpolation, DE combination,
  KIFS), options 2 (bit0 no ADE, bits1-2 inside rendering), end/repeat 5 (low/high nibble).
- Lighting record (offsets from 432): TBpos[3..11] at 8 (i32 sliders), TBoptions 44, AmbCol 52,
  AmbCol2 56, DepthCol 60, DepthCol2 64, lights 68 (6 x 32 bytes: option byte, function byte,
  ShortFloat intensity, RGB, map nr, Double7B angles), colors 260 (10 x 10 bytes: position u16,
  diffuse RGB u32, specular u32), interior colors 360, background image name 384.
- Camera: pixel size `sw = 2.1345 / (zoom * width)`; rays start on a plane through `mid`,
  directions fan out with `fov/height` radians per pixel. The import uses a pinhole camera at
  `mid + forward * (dZstart - dZmid - sw*H/2/tan(fov/2))`.
- Depth fog reaches `28000 * TBpos[4] * 0.8e-6` at the far end of the ray.

## Formula calling convention (.m3f machine code)

- `procedure(var x, y, z, w: Double; PIteration3D)`, Delphi register convention: eax/edx/ecx =
  &x, &y, &z, `[ebp+12]` = &w, `[ebp+8]` = record pointer (at field C1), `ret 8`.
- Record (`TIteration3Dext`, offsets from C1): J4 -56, Rold -48, RStopD -40, x/y/z/w -32..-8,
  C1..C3 0..16 (start position), J1..J3 24..40 (constant added), PVar 48, SmoothItD 52, Rout 56
  (squared length), ItResultI 64, maxIt 68, RStop 72, OTrap 192, VaryScale 200, bFirstIt 208,
  Dfree1/2 216/224, Deriv1..3 232..248, Ju1..Ju4 320..344, map functions 352/356.
- 3D formulas with analytic DE use w as the derivative (starts at 1); 4D formulas use Deriv1.
- PVar: constants at 0, 8, 16, … (8 bytes each), `PVar-8` = 0.5, options downward from -16 in
  `FillCustomVBufWithVars` order (Double 8 bytes, Single 4, Integer 4, angles as sin/cos,
  3 angles as a rotation matrix, BoxScale = Scale/MinR^2 and MinR^2, Folding R/2R/-R/-2R, …).
- `.DEoption`: -1 transform (does not count as an iteration), 0/1 numeric DE, 2 analytic
  (r/|w|), 11 Amazing Box analytic, 4 4D numeric, 5/6 4D analytic, 20..22 dIFS (other
  convention, not translated yet).
- Hybrid loop: Rold = Rout before each call; counted formulas increase ItResultI and set
  Rout = x^2+y^2+z^2; stop at maxIt or Rout > RStop^2.

## Translation status

`rmb-cli m3f ../mb3d/M3Formulas` prints the current count and the reasons for failures.

## Shading (PaintThread.CalcPixelColor2, HeaderTrafos.MakeLightValsFromHeaderLight)

- MB3D shades in display space: colors are 0..255 sRGB values, no linear conversion, no tone
  mapping (the import sets `post.display_space`). Option "internal gamma 2" (AdditionalOptions
  bit 0) squares the colors instead (close to a linear workflow with gamma 2).
- Pixel = ambient + diffuse + specular, then depth fog, then dynamic fog:
  - ambient = mix(AmbCol, AmbCol2, n_down * 0.5 + 0.5) * AO * (TBpos[8] & 0xFFF) / 90, times the
    object color; `n_down` is the normal along the camera's down axis (world axis if TBoptions
    bit 29).
  - diffuse = object color * sDiff * sum(light color * lamp * cos), sDiff = TBpos[5] * 0.02.
  - specular = spec color of the stop * sum(light color * lamp * sSpec * (R.L)^p), Phong with
    p = 2 << (LFunction & 7), sSpec = max(0.004, (TBpos[7] & 0xFFF) * 0.02).
- Depth buffer: Zpos = 32767 * (1 - u), u = (sqrt(1 + k z) - 1) / (sqrt(1 + k Z) - 1), z the
  distance from the ray start plane, Z = dZend - dZstart, k = sin(max(fov, 1 rad) / H) / stepWidth.
- Depth fog: amount = (28000 - Zpos) * TBpos[4] * 0.8e-6 (squared if "far fog", TBoptions bit
  18; negative amounts brighten); the fog color is the background: DepthCol (top) to DepthCol2
  (bottom) over the screen height, curve TBoptions >> 30 (0 linear, 1 square, else sqrt).
- Dynamic fog: amount = (steps - sShad) * sShadGr with sShadGr = (TBpos[6] - 53) * mZstepDiv *
  0.00065 and sShad = (137 - sqrt(TBpos[3] & 0xFFFF) * 11.3137) * 2.2 / mZstepDiv * 0.28 (other
  factors with bDFogIt, header 223); color mix(DynFogRGB, DynFogCol2, min(1, steps * mZstepDiv *
  0.015)); Lights[0].FreeByte bit 0: blend instead of add.
- Color index (CalcThread, CalcColors): SIgradient = 32767 - (S + dColPlus + VarMul *
  ln(threshold)) * 32767 / (maxIts + 1), S = n + (ln ln RStop - d) / (d - d_old), d = ln(0.5 ln
  Rout); gradient position = (SIgradient - start) / (end - start), start/end from TBpos[9]/[10]
  as ((v + 30) / 90)^2 * 32767 - 10900. VarMul = 0.5 / (ln(power) + 0.03) per analytic formula
  (0.6 for numeric ones), dColPlus = 0.1 * maxIts (+ a term for boxes). Stops wrap only with
  color cycling (TBoptions bit 14).
- DOF (post process, DOF.pas): enabled by bit 0 of byte 181; blur radius in pixels =
  Ap * (z - zSharp) / pixel size(z), zSharp = sDOFZsharp (164) * Width pixels from the start
  plane, Ap = sDOFaperture (172) * 0.5 for one pass. Equals a thin lens with radius Ap * focus.
- Lights: diffuse function (LFunction >> 4) & 3 = cos, cos², cos/2 + 1/2, (cos/2 + 1/2)²
  (DiffCosTabNsmall). Lights with shadows enabled but not calculated are multiplied by the
  ambient shadow, others by 1 - DiffuseShadowing * (1 - AO) (Lights[3].AdditionalByteEx / 256).
  Positional lights (Loption bit 2): position = the three Double7B values (world), color scaled
  by lamp * 1.3, falloff 1 / d² for diffuse and specular.
- Ambient shadow amplitude a = (TBpos[11] & 0xFF) / 53: 1 - a * s up to 1, above it
  d + (a - 1) * (d² - d) with d = 1 - s. Byte 149: bit 0 on, bits 2-3 type (0/1/2 screen space,
  3 DEAO); DEAO rays reach sDEAOmaxL (374) * half the image diagonal in pixels; the screen-space
  kinds search in levels up to about the image size.
- Dynamic fog on iteration (byte 223 = N > 0): only ray steps whose DE evaluation ended after
  N iterations count.

## Maps (Calc.pas, maps/Maps.pas)

- Map `n` is the file `<n>.jpg|png|bmp|jpeg|pgm` (or `<n><non-digit>…`) in the M3Maps folder.
  Pixels are kept as Windows bitmap bytes: channel 0 = blue, 1 = green, 2 = red (0..255).
- `PMapFunc` (record +352, `GetMapPixelSphereSpline`): u = atan2(x, y) · (-1/2π) + 0.5,
  v = 0.5 - asin(z)/π of the normalized vector; `PMapFunc2` (+356, `GetMapPixelDirectXYspline`):
  u, v = fract of the two inputs. Both sample with `SplineIpolMap`: uniform cubic B-spline
  (`MakeSplineCoeff`) over the 4×4 pixels around (u·width, v·height), wrapping at the edges,
  result / 255. Call convention: eax = input vector, edx = map number, ecx = 3 result doubles.

## Visible lights (PaintThread.CalcPixelColor2, CalcPosLightShape)

- Type = iLightPos & 14 with iLightPos = ((Loption >> 2) & 7) | ((LFunction & 128) >> 4):
  2 sharp, 4 halo ("more transparency"), 6 ball, 8 and others soft. Squared radius
  R = sLmaxL * 1e-8 (point light: 800 * (color sum * lamp * 1.3 * ls² + 128 * lamp), world units;
  global light: 800 * (color sum * ls + 128) * lamp, in 1 - cos(angle); type 8 five times).
- u = squared distance of the light from the view ray / R (point lights; skipped behind the
  viewer or behind the surface), or (1 - dot(light, view)) / R for global lights on the
  background only. CalcPosLightShape gives the light added (∝ 1/R) and the share of the pixel
  kept; the light is dimmed by 1 - 0.9 · depth fog at its position. Types 2 and 8 add, the others
  blend: pixel · kept + depth color · fog · (1 - kept) + light.

## Background images (FileHandling.LoadBackgroundPicT, ImageProcess.GetLightMapPixelSphere)

- File name at light record +384 (24 chars), searched in the picture, map and other folders.
- Spherical: the world view direction is multiplied with the transposed `PicRotMatrix`
  (BuildRotMatrixS(π - PicOffsetX·π/128, PicOffsetY·π/128, PicOffsetZ·π/128); offsets at light
  +50, +51, +67), u = atan2(v0, v2)·(-1/2π) + 0.5, v = 0.5 - asin(v1)/π, B-spline sampled with
  horizontal wrap and vertical clamp. TBoptions bit 15: screen coordinates instead.
- The depth fog still blends toward the depth colors (DepthCol/DepthCol2), not the image.
- Screen coordinates (TBoptions bit 15, bDirectImageCoord): primary rays read the image at the
  pixel position (GetLightMapPixel without wrap); reflections at x = atan2(v0, v2)/FOVx + 0.5,
  y = asin(v1)/FOVy + 0.5 of the view-space direction.
- Brightness sIntensity = 1.04^(Lights[4].AdditionalByteEx − 40) multiplies every image sample.
- Load options (LoadLightMap): AdditionalOptions bit 7 "convert to spherical" squeezes row y
  toward the center by s = cos(asin(2(y + 0.5)/H − 1)): pixel x reads (x − c)·s + c with the
  cubic 1/6·(3f² − 2f − f³, 3(f³ − 2f² − f + 2), 3(f² + 2f − f³), f³ − f) over pixels i..i+3,
  which samples one pixel to the right (i + 1 + f); bit 4 "fit border" blends the six outer
  columns of both sides (weights 0.5 ∓ 0.5·sin(π/2·(x + 0.5)/6)); TBoptions bit 19 smooths
  (not implemented).
- Ambient light from the image (AdditionalOptions bit 5, bUseSmallBGpicForAmb): a 16 × 8 copy
  (MakeSmallLMimage: box averages of columns, then rows, leaving out the last source pixel;
  rows widened toward the poles by 2·16/(sin(y·π/7) + 1) − 16 pixels, the first and last row
  averaged completely) is read in the direction of the world normal like the background. It
  replaces the ambient colors (no ambient strength) and blends into the depth color by the fog
  at infinity f∞ = min(1, 28000·sDepth) (squared with far fog): amb·(1 − f∞) + DepC·f∞; with
  AdditionalOptions bit 3 (bAddBGlight) amb + DepC·f∞, and the background image is then not
  covered by the fog either (the fog color is added).

## Reflections (CalcSR.pas) and interpolation hybrids (formulas.pas)

- Reflections are calculated when byte 336 bit 0 is set (bit 1: transmission, bit 2: only on
  dIFS); amount SRamount (f32 at 332), bounces SRreflectioncount (byte 337). Each bounce adds
  the reflected color weighted by: the specular color of the reflecting surface · amount · what
  the fog leaves visible there; the weights multiply over the bounces. Reflected hits are shaded
  like primary ones (lights, hard shadows, AO with fewer rays); misses show the background.
- Transmission (glass, bit 1): the alpha byte of each gradient stop's specular color (light
  record +260 + 10 k + 9) is the transparency `a`; refraction index sTRIndex (f32 at 116; 1 for
  inside-and-outside rendering), scattering sTRscattering (120), absorption
  sTransmissionAbsorption (370). CalcRay builds a tree: at a surface with `a` > 0 it follows the
  refracted ray (eta = 1/n entering, n leaving; normal from the DE, facing the ray) and then the
  reflected one. Fresnel F = ((ci − η ct)/(ci + η ct))² + ((ct − η ci)/(ct + η ci))², halved.
  Refracted weight · (1 − F · max(spec)) · a, reflected weight · spec · (F + (1 − F)(1 − a));
  total internal reflection: spec · (1 − a + a²/(max(spec) + 0.01)). A ray inside the solid
  marches with inside rendering switched (bInsideRendering flipped), has no fog, and is absorbed
  by `diffuse color of the entry surface ^ (length · absorption)` (world units: absorption ·
  dStepWidth · length in steps); scattering adds the lights times that color times
  (1 − Y(absorbed)) · sTRscattering / 330. Surfaces the path continues from scale their diffuse
  and ambient light by 1 − a · SRamount (bScaleAmbDiffDown; not on the last bounce).
  RMandelbulb3D follows one branch of the tree per sample, picked by its weight (averaged over
  the samples it gives the same result).
- Interpolation hybrid (addon byte 1 = 1): the iteration counts of formulas 1 and 2 are f32
  weights. Every iteration runs both on the same point; result = blend of the two vectors scaled
  to the blended length; derivative |w1|·s1 + |w2|·s2 (doInterpolHybridPasDE).

## Smoothed normals and roughness (Calc.RMCalculateNormals, PaintThread)

- iOptions (u16 at 16) bits 6..9: smoothing steps S (0 off, 8 = 5×5×5 cube normal). Normal
  offset No = 0.15 · min(1, DEstop) · pixel size; center = average of 5 estimates (±2No along the
  screen axes); along two tangents 2S estimates at steps 6No/(S + 0.5): slopes (DE - center)/i;
  roughness = clamp(sqrt(7 · spread · dt2² / |N|²) - 0.05, 0, 1) with spread = Σ(slope²)·2S -
  (Σslope)² per tangent and dt2 = No / (2S · step); the mean slopes bend the normal (S < 8).
- Shading: dRough = roughness · RoughnessFactor (light +2, /255). Specular weight
  (1 + min(1, 2·dRough)·(1/p - 1)); diffuse = mix(table, rough table, dRough), rough tables =
  blurred square roots of the diffuse tables (LightAdjust.SetCosTabFunction).

## Depth color variation and interior colors (PaintThread.CalcColors / CalcColorsInside)

- VarColZpos (i16 at light +0): the color index gets sColZmul · zPos added, sColZmul =
  VarColZpos · -0.005 / (stepWidth · Width), zPos = depth relative to the mid plane: the gradient
  position moves by 0.5 · sColZmul per world unit.
- Interior colors (ICols at light +360, 4 × position u16 + RGBA, alpha = specular) color pixels
  whose ray meets the inside of the set (no DE stop: cut faces, camera inside): SIgradient =
  32768 + 32767 · Rout / RStop (inside rendering: smooth iterations · 32767 / (maxIt + 1));
  start/end from TBoptions bits 0..6 / 7..13 as 32768 + s⁴ · 0.000158025.

## Color maps (HeaderTrafos.MakeLightValsFromHeaderLight, PaintThread)

- Map number = bColorMap (light +3) | Lights[1].AdditionalByteEx << 8; mode = Lights[1].FreeByte
  & 3: 0 orbit trap × color index (2D: trap · 4096 and SIgradient, both 0..32767 → -0.5..0.5,
  rotated by ((TBpos[8] >> 20) & 255 - 128) · 2π/256, offset ((TBpos[7] >> 12 / >> 20) & 255)/256,
  scale 1.05^(Lights[2].AdditionalByteEx - 30)); 1 by normal (sphere lookup, rotation
  BuildRotMatrixS(π - a·π/128, b·π/128, c·π/128)); 2 3D sine wrap and 3 three planar projections
  (world position · 1.2^(Ex2 - 30)). Modes 1..3 blend toward the map's average color by the
  roughness. AdditionalOptions bit 2: keep the gradient hue, take the map's luminance.

## DE combination "mix" (Calc.pas, types 6/7)

- The first chain iterates normally (until bailout or max iterations); the dIFS chain then
  starts from that point (doHybridIFS3DnoVecIni: x, y, z kept, J unchanged, VaryScale 1). Result:
  dIFS distance / |w| (first chain DEoption 2, 11) or / Deriv1 (5, 6), else · FmixPow^(-1 - its1)
  (sFmixPow f32 at 104, 2 before version 39).
