// SPDX-License-Identifier: LGPL-2.1-or-later
//
// Double-float arithmetic ("df64"): a value is the unevaluated sum hi + lo of two f32 (x = hi,
// y = lo, |lo| <= ulp(hi) / 2), about 48 bits of mantissa. Used by the deep zoom shader for the
// fractal iteration (see codegen.rs, deep mode). The error-free transformations rely on IEEE
// f32 rounding; products use fma.
//
// GPU compilers simplify float expressions algebraically ((a + b) - a = b), which destroys the
// rounding errors these functions capture. `dd_one` is 1.0, but read from a buffer at run time
// (the shader sets it before any dd arithmetic), so the compiler cannot see through the
// products with it.

alias dd = vec2<f32>;

var<private> dd_one: f32;
// Deep zoom: half the surface threshold at the point being estimated; the fractal iteration
// switches to f32 once rounding there stays far below it (codegen.rs DD_SWITCH; 0: never).
var<private> dd_target: f32;

fn dd_f(a: f32) -> dd {
    return dd(a, 0.0);
}

fn dd_hi(a: dd) -> f32 {
    return a.x + a.y;
}

// Exact sum of two f32 (Knuth).
fn dd_two_sum(a: f32, b: f32) -> dd {
    let s = (a + b) * dd_one;
    let bb = (s - a) * dd_one;
    return dd(s, (a - (s - bb)) + (b - bb));
}

// Exact sum when |a| >= |b|.
fn dd_quick(a: f32, b: f32) -> dd {
    let s = (a + b) * dd_one;
    return dd(s, b - (s - a));
}

// Exact product of two f32.
fn dd_two_prod(a: f32, b: f32) -> dd {
    let p = (a * b) * dd_one;
    return dd(p, fma(a, b, -p));
}

fn dd_add(a: dd, b: dd) -> dd {
    var s = dd_two_sum(a.x, b.x);
    let t = dd_two_sum(a.y, b.y);
    s.y += t.x;
    s = dd_quick(s.x, s.y);
    s.y += t.y;
    return dd_quick(s.x, s.y);
}

fn dd_sub(a: dd, b: dd) -> dd {
    return dd_add(a, -b);
}

fn dd_neg(a: dd) -> dd {
    return -a;
}

fn dd_mul(a: dd, b: dd) -> dd {
    var p = dd_two_prod(a.x, b.x);
    p.y = fma(a.x, b.y, fma(a.y, b.x, p.y));
    return dd_quick(p.x, p.y);
}

fn dd_mulf(a: dd, b: f32) -> dd {
    var p = dd_two_prod(a.x, b);
    p.y = fma(a.y, b, p.y);
    return dd_quick(p.x, p.y);
}

// a / b for an f32 b (exact integers in the series below).
fn dd_divf(a: dd, b: f32) -> dd {
    let q1 = (a.x / b) * dd_one;
    let r = dd_sub(a, dd_two_prod(q1, b));
    let q2 = (r.x / b) * dd_one;
    let r2 = dd_sub(r, dd_two_prod(q2, b));
    return dd_add(dd_quick(q1, q2), dd_f(r2.x / b));
}

fn dd_sqr(a: dd) -> dd {
    return dd_mul(a, a);
}

fn dd_div(a: dd, b: dd) -> dd {
    if (b.x == 0.0) {
        return dd_f(a.x / b.x);
    }
    let q1 = (a.x / b.x) * dd_one;
    let r = dd_sub(a, dd_mulf(b, q1));
    let q2 = (r.x / b.x) * dd_one;
    let r2 = dd_sub(r, dd_mulf(b, q2));
    let q3 = r2.x / b.x;
    return dd_add(dd_quick(q1, q2), dd_f(q3));
}

fn dd_lt(a: dd, b: dd) -> bool {
    return a.x < b.x || (a.x == b.x && a.y < b.y);
}

fn dd_le(a: dd, b: dd) -> bool {
    return a.x < b.x || (a.x == b.x && a.y <= b.y);
}

fn dd_gt(a: dd, b: dd) -> bool {
    return dd_lt(b, a);
}

fn dd_ge(a: dd, b: dd) -> bool {
    return dd_le(b, a);
}

fn dd_eq(a: dd, b: dd) -> bool {
    return a.x == b.x && a.y == b.y;
}

fn dd_ne(a: dd, b: dd) -> bool {
    return !dd_eq(a, b);
}

fn dd_abs(a: dd) -> dd {
    return select(a, -a, dd_lt(a, dd_f(0.0)));
}

fn dd_min(a: dd, b: dd) -> dd {
    return select(b, a, dd_lt(a, b));
}

fn dd_max(a: dd, b: dd) -> dd {
    return select(b, a, dd_gt(a, b));
}

fn dd_clamp(a: dd, lo: dd, hi: dd) -> dd {
    return dd_min(dd_max(a, lo), hi);
}

fn dd_sign(a: dd) -> dd {
    if (a.x > 0.0) {
        return dd_f(1.0);
    }
    if (a.x < 0.0) {
        return dd_f(-1.0);
    }
    return dd_f(0.0);
}

fn dd_floor(a: dd) -> dd {
    let f = floor(a.x);
    if (f != a.x) {
        return dd_f(f);
    }
    return dd_quick(f, floor(a.y));
}

fn dd_ceil(a: dd) -> dd {
    return -dd_floor(-a);
}

fn dd_trunc(a: dd) -> dd {
    return select(dd_floor(a), dd_ceil(a), a.x < 0.0);
}

fn dd_fract(a: dd) -> dd {
    return dd_sub(a, dd_floor(a));
}

// Rounds half away from zero (like Rust's f64::round, which the CPU renderer uses).
fn dd_round(a: dd) -> dd {
    let h = dd_add(dd_abs(a), dd_f(0.5));
    let r = dd_floor(h);
    return select(r, -r, a.x < 0.0);
}

// Truncated remainder like WGSL's % (and Rust's %).
fn dd_rem(a: dd, b: dd) -> dd {
    return dd_sub(a, dd_mul(b, dd_trunc(dd_div(a, b))));
}

fn dd_sqrt(a: dd) -> dd {
    if (a.x <= 0.0) {
        return dd_f(sqrt(a.x));
    }
    let q = sqrt(a.x) * dd_one;
    let r = dd_sub(a, dd_two_prod(q, q));
    return dd_quick(q, r.x / (2.0 * q));
}

fn dd_inverse_sqrt(a: dd) -> dd {
    return dd_div(dd_f(1.0), dd_sqrt(a));
}

const DD_LN2: dd = dd(0.6931471824645996, -1.904654323148236e-9);
const DD_PI: dd = dd(3.1415927410125732, -8.742278012618954e-8);
const DD_PI2: dd = dd(1.5707963705062866, -4.371139006309477e-8);

fn dd_ldexp(a: dd, e: i32) -> dd {
    return dd(ldexp(a.x, e), ldexp(a.y, e));
}

fn dd_exp(a: dd) -> dd {
    if (a.x > 88.0) {
        return dd_f(exp(a.x));
    }
    if (a.x < -87.0) {
        return dd_f(0.0);
    }
    // a = k ln 2 + r, r / 16 by a Taylor series, then squared four times.
    let k = round(a.x / DD_LN2.x);
    let r = dd_mulf(dd_sub(a, dd_mulf(DD_LN2, k)), 0.0625);
    var term = r;
    var sum = r;
    for (var i = 2; i <= 9; i++) {
        term = dd_divf(dd_mul(term, r), f32(i));
        sum = dd_add(sum, term);
    }
    // e^r - 1 squared up: (1 + s)^2 - 1 = s (2 + s), keeps the small part exact.
    for (var i = 0; i < 4; i++) {
        sum = dd_mul(sum, dd_add(sum, dd_f(2.0)));
    }
    return dd_ldexp(dd_add(sum, dd_f(1.0)), i32(k));
}

fn dd_log(a: dd) -> dd {
    if (a.x <= 0.0) {
        return dd_f(log(a.x));
    }
    // Newton on exp: y + a e^-y - 1.
    var y = dd_f(log(a.x));
    y = dd_sub(dd_add(y, dd_mul(a, dd_exp(-y))), dd_f(1.0));
    return y;
}

// sin and cos of k pi / 128 for k = 0..32 (high and low parts).
const DD_SINCOS_TABLE: array<vec4<f32>, 33> = array<vec4<f32>, 33>(
    vec4<f32>(0.0, 0.0, 1.0, 0.0),
    vec4<f32>(0.024541229009628296, -4.867160008359406e-10, 0.99969881772995, 9.66254298795377e-10),
    vec4<f32>(0.049067676067352295, -1.7399343077428853e-09, 0.9987954497337341, 6.471438496191695e-09),
    vec4<f32>(0.0735645666718483, -3.0721809540779077e-09, 0.9972904324531555, 2.4225535355526517e-08),
    vec4<f32>(0.0980171412229538, -8.933931927579408e-10, 0.9951847195625305, 7.1096666331982306e-09),
    vec4<f32>(0.12241067737340927, -2.1741930478214044e-09, 0.9924795627593994, -2.8160689780065695e-08),
    vec4<f32>(0.1467304676771164, 6.778245520422388e-09, 0.9891765117645264, -1.7997453527485163e-09),
    vec4<f32>(0.1709618866443634, 2.115937869362483e-09, 0.9852776527404785, -1.0351537405028921e-08),
    vec4<f32>(0.19509032368659973, -1.6704715388726754e-09, 0.9807852506637573, 2.9739473106360492e-08),
    vec4<f32>(0.21910123527050018, 4.88636953122068e-09, 0.9757021069526672, 2.3085862110860944e-08),
    vec4<f32>(0.24298018217086792, -2.267604104488896e-09, 0.9700312614440918, -8.249547711614014e-09),
    vec4<f32>(0.2667127549648285, 2.5100699296842777e-09, 0.9637760519981384, 1.3797301079421231e-08),
    vec4<f32>(0.290284663438797, 1.3815665056426951e-08, 0.9569403529167175, -1.7184508038781132e-08),
    vec4<f32>(0.3136817514896393, -1.1090747875641682e-08, 0.949528157711029, 2.288200739997137e-08),
    vec4<f32>(0.3368898630142212, -9.622000973763534e-09, 0.9415440559387207, 9.244300436250796e-09),
    vec4<f32>(0.3598950505256653, -1.3990677061315182e-08, 0.9329928159713745, -1.7136635221959295e-08),
    vec4<f32>(0.3826834261417389, 6.2233507236442165e-09, 0.9238795042037964, 2.830748968563057e-08),
    vec4<f32>(0.40524131059646606, 3.4085236855219136e-09, 0.91420978307724, -2.7373708633149363e-08),
    vec4<f32>(0.4275550842285156, 9.201766459909777e-09, 0.903989315032959, -2.1909515979245953e-08),
    vec4<f32>(0.4496113359928131, -6.338206404166158e-09, 0.89322429895401, 2.241505203670613e-09),
    vec4<f32>(0.4713967442512512, -7.425253745196869e-09, 0.8819212913513184, -2.7002963420841297e-08),
    vec4<f32>(0.49289819598197937, -3.75219544324068e-09, 0.8700869679450989, 2.3163613249721493e-08),
    vec4<f32>(0.5141027569770813, -1.2783859304477119e-08, 0.8577286005020142, 9.498258179974073e-09),
    vec4<f32>(0.5349976420402527, -2.2153155754267573e-08, 0.8448535799980164, -1.4748309240530943e-08),
    vec4<f32>(0.5555702447891235, -1.1769521357507529e-08, 0.8314695954322815, 1.687026340846387e-08),
    vec4<f32>(0.5758081674575806, 2.396026488327152e-08, 0.8175848126411438, 5.104399125599457e-10),
    vec4<f32>(0.5956993103027344, -5.810301129116624e-09, 0.803207516670227, 1.4810417781063734e-08),
    vec4<f32>(0.6152315735816956, 1.6998932039768988e-08, 0.7883464097976685, 1.7828938325692434e-08),
    vec4<f32>(0.6343932747840881, 9.37955757507325e-09, 0.7730104327201843, 2.0642552556182636e-08),
    vec4<f32>(0.6531728506088257, -7.655049039101414e-09, 0.7572088241577148, 2.2348769945779168e-08),
    vec4<f32>(0.6715589761734009, -2.1326382437791835e-08, 0.7409511208534241, 4.501535144640911e-09),
    vec4<f32>(0.6895405650138855, -2.027681844651852e-08, 0.7242470979690552, -1.5017588950172467e-08),
    vec4<f32>(0.7071067690849304, 1.2101617485882343e-08, 0.7071067690849304, 1.2101617485882343e-08),
);
const DD_PI128: dd = dd(0.02454369328916073, -6.829904419802801e-10);

// sin and cos of a (x, y): quadrant, then a table entry k pi / 128, then short series for the
// rest (|s| <= pi / 256: terms up to s^7 and s^6 reach the double-float precision).
fn dd_sincos(a: dd) -> array<dd, 2> {
    let k = round(a.x / DD_PI2.x);
    let r = dd_sub(a, dd_mulf(DD_PI2, k));
    let j = round(r.x / DD_PI128.x);
    let s = dd_sub(r, dd_mulf(DD_PI128, j));
    let s2 = dd_sqr(s);
    // sin s = s (1 - s^2/6 (1 - s^2/20 (1 - s^2/42))), cos s = 1 - s^2/2 (1 - s^2/12 (1 - s^2/30)).
    let sn = dd_mul(s, dd_sub(dd_f(1.0), dd_mul(dd_divf(s2, 6.0), dd_sub(dd_f(1.0), dd_mul(dd_divf(s2, 20.0), dd_sub(dd_f(1.0), dd_divf(s2, 42.0)))))));
    let cs = dd_sub(dd_f(1.0), dd_mul(dd_divf(s2, 2.0), dd_sub(dd_f(1.0), dd_mul(dd_divf(s2, 12.0), dd_sub(dd_f(1.0), dd_divf(s2, 30.0))))));
    let table = DD_SINCOS_TABLE;
    let e = table[u32(abs(j))];
    let sj = dd(e.x, e.y) * sign(j + 0.5);
    let cj = dd(e.z, e.w);
    let sv = dd_add(dd_mul(sj, cs), dd_mul(cj, sn));
    let cv = dd_sub(dd_mul(cj, cs), dd_mul(sj, sn));
    let q = ((i32(k) % 4) + 4) % 4;
    if (q == 0) {
        return array<dd, 2>(sv, cv);
    }
    if (q == 1) {
        return array<dd, 2>(cv, -sv);
    }
    if (q == 2) {
        return array<dd, 2>(-sv, -cv);
    }
    return array<dd, 2>(-cv, sv);
}

fn dd_sin(a: dd) -> dd {
    return dd_sincos(a)[0];
}

fn dd_cos(a: dd) -> dd {
    return dd_sincos(a)[1];
}

fn dd_tan(a: dd) -> dd {
    let sc = dd_sincos(a);
    return dd_div(sc[0], sc[1]);
}

fn dd_atan2(y: dd, x: dd) -> dd {
    if (x.x == 0.0 && y.x == 0.0) {
        return dd_f(atan2(y.x, x.x));
    }
    // f32 start, then a Newton step on the angle (converges cubically).
    var z = dd_f(atan2(y.x, x.x));
    let sc = dd_sincos(z);
    let num = dd_sub(dd_mul(y, sc[1]), dd_mul(x, sc[0]));
    let den = dd_add(dd_mul(x, sc[1]), dd_mul(y, sc[0]));
    return dd_add(z, dd_div(num, den));
}

fn dd_atan(a: dd) -> dd {
    return dd_atan2(a, dd_f(1.0));
}

fn dd_asin(a: dd) -> dd {
    return dd_atan2(a, dd_sqrt(dd_sub(dd_f(1.0), dd_sqr(a))));
}

fn dd_acos(a: dd) -> dd {
    return dd_atan2(dd_sqrt(dd_sub(dd_f(1.0), dd_sqr(a))), a);
}

// Integer exponents up to 64 by squaring (negative bases allowed), else exp(b log a).
fn dd_pow(a: dd, b: dd) -> dd {
    let bi = round(b.x);
    if (bi == b.x && b.y == 0.0 && abs(bi) <= 64.0) {
        var e = u32(abs(bi));
        var base = a;
        var r = dd_f(1.0);
        loop {
            if (e == 0u) {
                break;
            }
            if ((e & 1u) == 1u) {
                r = dd_mul(r, base);
            }
            e = e >> 1u;
            if (e > 0u) {
                base = dd_sqr(base);
            }
        }
        if (bi < 0.0) {
            r = dd_div(dd_f(1.0), r);
        }
        return r;
    }
    if (a.x == 0.0) {
        return dd_f(pow(a.x, b.x));
    }
    if (a.x < 0.0) {
        return dd_f(pow(a.x, b.x));
    }
    return dd_exp(dd_mul(b, dd_log(a)));
}

fn dd_sinh(a: dd) -> dd {
    let e = dd_exp(a);
    return dd_mulf(dd_sub(e, dd_div(dd_f(1.0), e)), 0.5);
}

fn dd_cosh(a: dd) -> dd {
    let e = dd_exp(a);
    return dd_mulf(dd_add(e, dd_div(dd_f(1.0), e)), 0.5);
}

fn dd_tanh(a: dd) -> dd {
    if (abs(a.x) > 20.0) {
        return dd_f(sign(a.x));
    }
    let e = dd_exp(dd_mulf(a, 2.0));
    return dd_div(dd_sub(e, dd_f(1.0)), dd_add(e, dd_f(1.0)));
}

fn dd_mix(a: dd, b: dd, t: dd) -> dd {
    return dd_add(a, dd_mul(dd_sub(b, a), t));
}

fn dd_step(edge: dd, x: dd) -> dd {
    return select(dd_f(1.0), dd_f(0.0), dd_lt(x, edge));
}

fn dd_smoothstep(e0: dd, e1: dd, x: dd) -> dd {
    let t = dd_clamp(dd_div(dd_sub(x, e0), dd_sub(e1, e0)), dd_f(0.0), dd_f(1.0));
    return dd_mul(dd_sqr(t), dd_sub(dd_f(3.0), dd_mulf(t, 2.0)));
}
