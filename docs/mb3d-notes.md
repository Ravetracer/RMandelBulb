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
