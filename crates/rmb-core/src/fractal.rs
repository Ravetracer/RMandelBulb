// SPDX-License-Identifier: LGPL-2.1-or-later

//! The hybrid formula chain prepared for rendering, and its CPU evaluation.
//!
//! A `Program` is what both renderers iterate: the active slots with their formulas and
//! constants. The GPU renderer generates its shader from `signature()`-equal programs once and
//! uploads `constants` whenever parameters change. With a DE combination the program has a
//! second part (its own slots, iteration limit and estimate); the two estimates are combined.

use std::sync::Arc;

use crate::formula::lang::{STATE_AUX, STATE_COUNT, STATE_DR, STATE_ITER, STATE_JULIA, STATE_PX, STATE_ROLD, STATE_ROUT, uses_state};
use crate::formula::{DeKind, Formula};
use crate::math::V3;
use crate::scene::{CombineMode, Cutting, DeMode, Fractal, Slot};

pub struct ProgramSlot {
    pub formula: Arc<Formula>,
    pub iterations: u32,
    /// Offset of this slot's constants in `Program::constants`.
    pub offset: usize,
    pub transform: bool,
    /// Mandelbulber per-slot settings (see `scene::Slot`): iteration range (`stop` u32::MAX:
    /// no end), weight, add C, escape test.
    pub start: u32,
    pub stop: u32,
    pub weight: f64,
    pub add_c: bool,
    pub check_bailout: bool,
}

/// Value of a constant expression over a slot's parameters and `init` constants.
fn const_value(e: &crate::formula::lang::Expr, consts: &[f64]) -> Option<f64> {
    use crate::formula::lang::{BinOp, Expr, Func, VarRef};
    Some(match e {
        Expr::Num(v) => *v,
        Expr::Var(VarRef::Param(k)) => *consts.get(*k)?,
        Expr::Neg(a) => -const_value(a, consts)?,
        Expr::Bin(op, a, b) => {
            let (a, b) = (const_value(a, consts)?, const_value(b, consts)?);
            match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                _ => return None,
            }
        }
        Expr::Call(Func::Round, a) => const_value(&a[0], consts)?.round(),
        Expr::Call(Func::Floor, a) => const_value(&a[0], consts)?.floor(),
        _ => return None,
    })
}

/// Smallest numeric gradient step relative to the point in f64 and double-float arithmetic.
pub const NUMERIC_STEP_F64: f64 = 1e-13;

pub struct Program {
    pub slots: Vec<ProgramSlot>,
    /// Slot the chain continues with after the last one (index into `slots`).
    pub repeat_from: usize,
    pub max_iterations: u32,
    /// No escape before this many iterations.
    pub min_iterations: u32,
    /// Factor of the constant per axis (1 = unchanged).
    pub c_factor: [f64; 3],
    pub bailout2: f64,
    pub julia: bool,
    pub julia_c: [f64; 4],
    pub w_slice: f64,
    pub de_kind: DeKind,
    pub numeric: bool,
    /// Distance-estimated IFS hybrid (any dIFS formula in it).
    pub difs: bool,
    /// DE multiplier of this part's formulas (weighted by their iteration counts).
    pub de_scale: f64,
    /// DE multiplier of the scene (applied once, after combining).
    pub scene_de_scale: f64,
    /// Parameters and `init` constants of all slots (both parts), concatenated.
    pub constants: Vec<f64>,
    /// Cutting planes: (axis, position, keep_above).
    pub cuts: Vec<(usize, f64, bool)>,
    /// Formulas of the scene that are not in the library.
    pub missing: Vec<String>,
    /// Start point rotation into 4D (rows of a 4x4 matrix), if any formula uses w.
    pub rotation_4d: Option<[[f64; 4]; 4]>,
    /// Second chain of a DE combination.
    pub part2: Option<Box<Program>>,
    pub combine_mode: CombineMode,
    pub combine_smooth: f64,
    /// Mix mode: distance divisor per iteration of the first chain.
    pub mix_pow: f64,
    /// Interpolation of the first two slots (weights), see `Fractal::blend`.
    pub blend: Option<[f64; 2]>,
    /// The GPU shader includes the reflection loop (set by the renderer; shaders without it
    /// compile faster).
    pub reflections: bool,
    /// Deep zoom on the GPU (set by the renderer): the shader iterates the main fractal in
    /// double-float arithmetic at this reference point plus the local position (all other
    /// positions are relative to it, see `render::shifted_scene`).
    pub deep: Option<[f64; 3]>,
    /// Inside rendering (the estimate is inverted around the surface threshold).
    pub inside: bool,
    /// Objects combined with this fractal (scene composition), in order.
    pub objects: Vec<ObjectProgram>,
    /// Placement of the main fractal (`Fractal::transform`) when it is not the identity:
    /// inverse rotation rows, position, scale per own axis and the smallest of those.
    pub main_transform: Option<MainTransform>,
    /// Some material has texture layers / bump layers (the GPU shader includes their code only
    /// then: it compiles faster without).
    pub layers: bool,
    pub bump: bool,
    /// Mandelbulb3D's second color choice (`SecondChoice::index`) and its multiplier.
    pub color2: (u32, f64),
    /// Mandelbulb3D's color on iteration (-1 off).
    pub color_on_it: i32,
}

/// An object of the scene composition prepared for evaluation.
pub struct ObjectProgram {
    pub shape: ObjectShape,
    /// Inverse rotation rows, position, scale per own axis (`Transform::size`) and the
    /// smallest of those (distances are multiplied by it: a lower bound when stretched).
    pub inv: [[f64; 3]; 3],
    pub pos: V3,
    pub size: V3,
    pub scale: f64,
    /// `BoolOp::index`.
    pub op: u32,
    pub smooth: f64,
    /// Material: 0 the main one, k the k-th entry of `Scene::objects` (1-based).
    pub material: usize,
    /// Repetition (`SceneObject::repeat`, `repeat_limit`).
    pub repeat: [f64; 3],
    pub repeat_limit: [u32; 3],
}

impl ObjectProgram {
    /// World point `p` in the object's own coordinates, folded into one copy when repeated.
    pub fn local(&self, p: V3) -> V3 {
        crate::compose::repeat_fold(crate::compose::to_local(&self.inv, self.pos, self.size, p), self.repeat, self.repeat_limit)
    }
}

/// See [`Program::main_transform`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MainTransform {
    pub inv: [[f64; 3]; 3],
    pub pos: V3,
    pub size: V3,
    pub scale: f64,
}

impl MainTransform {
    pub fn of(t: &crate::compose::Transform) -> Option<MainTransform> {
        (!t.is_identity()).then(|| {
            let size = t.size();
            MainTransform { inv: t.inverse_rotation(), pos: t.position, size, scale: size.x.min(size.y).min(size.z) }
        })
    }

    /// World point `p` in the fractal's own coordinates.
    pub fn local(&self, p: V3) -> V3 {
        crate::compose::to_local(&self.inv, self.pos, self.size, p)
    }
}

pub enum ObjectShape {
    /// A fractal chain; its constants are part of the main program's.
    Fractal(Box<Program>),
    /// `Primitive::index` and parameters.
    Primitive(u32, [f64; 4]),
    /// A baked 3D model (None while it bakes or when it does not load: no surface).
    Model(Option<crate::mesh::Model>),
}

/// Result of iterating one point.
#[derive(Clone, Copy, Debug, Default)]
pub struct Orbit {
    pub de: f64,
    pub iterations: u32,
    pub r2: f64,
    /// Squared radius before the last iteration.
    pub r2_old: f64,
    pub dr: f64,
    /// Smallest squared radius along the orbit.
    pub trap: f64,
    /// Mandelbulb3D's second color choice of the orbit (0..1, see [`color2_value`]).
    pub color2: f64,
    pub escaped: bool,
    /// The object that forms the surface here: 0 the main fractal, k the k-th object.
    pub object: u32,
}

/// Mandelbulb3D's 4D start rotation: Z(Y(X)) with X in the xw, Y in the yw, Z in the zw plane.
fn rotation_matrix_4d(deg: [f64; 3]) -> [[f64; 4]; 4] {
    let mut m = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    for (axis, a) in deg.iter().enumerate() {
        let (s, c) = a.to_radians().sin_cos();
        let mut r = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        r[axis][axis] = c;
        r[3][3] = c;
        r[axis][3] = -s;
        r[3][axis] = s;
        let mut out = [[0.0; 4]; 4];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..4).map(|k| r[i][k] * m[k][j]).sum();
            }
        }
        m = out;
    }
    m
}

impl Program {
    /// Numbers of the Mandelbulb3D maps the formulas read (both chains). A map number that is
    /// not a constant expression counts every whole parameter value of its slot.
    pub fn map_numbers(&self) -> Vec<i32> {
        let mut out = Vec::new();
        for s in self.slots.iter().chain(self.part2.iter().flat_map(|p| p.slots.iter())) {
            let args = crate::formula::lang::map_number_args(&s.formula.def.iter);
            if args.is_empty() {
                continue;
            }
            let n = s.formula.def.constant_count();
            let consts = &self.constants[s.offset.min(self.constants.len())..(s.offset + n).min(self.constants.len())];
            for a in &args {
                match const_value(a, consts) {
                    Some(v) => out.push(v.round() as i32),
                    None => out.extend(consts.iter().filter(|v| v.fract() == 0.0 && (0.0..100_000.0).contains(*v)).map(|v| *v as i32)),
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn new(f: &Fractal, cut: &Cutting) -> Program {
        Self::new_at(f, cut, 0)
    }

    /// [`Program::new`] whose constants start at `base` of the scene's constants (object
    /// fractals follow the main fractal's).
    fn new_at(f: &Fractal, cut: &Cutting, base: usize) -> Program {
        let split = f.combine.as_ref().map(|c| c.start.clamp(1, f.slots.len().max(1))).filter(|s| *s < f.slots.len());
        let end1 = split.unwrap_or(f.slots.len());
        let mut p = Self::build(f, &f.slots[..end1], 0, f.repeat_from, f.max_iterations, base);
        if let (Some(start), Some(c)) = (split, f.combine.as_ref()) {
            let mut p2 = Self::build(f, &f.slots[start..], start, c.repeat_from, c.max_iterations, base + p.constants.len());
            p.constants.append(&mut p2.constants);
            p.missing.append(&mut p2.missing);
            p.combine_mode = c.mode;
            p.combine_smooth = c.smooth;
            p.mix_pow = c.mix_pow;
            if !p2.slots.is_empty() {
                p.part2 = Some(Box::new(p2));
            }
        }
        // Interpolation of two formulas (not with a DE combination, transforms or dIFS).
        if let Some(w) = f.blend
            && p.part2.is_none()
            && p.slots.len() >= 2
            && !p.difs
            && !p.slots[0].transform
            && !p.slots[1].transform
        {
            p.blend = Some(w);
        }
        p.cuts = cut.planes().iter().enumerate().filter(|(_, pl)| pl.enabled).map(|(a, pl)| (a, pl.position, pl.keep_above)).collect();
        let uses_w = |prog: &Program| prog.slots.iter().any(|s| uses_state(&s.formula.def.iter, 3));
        if f.rotation_4d.iter().any(|a| *a != 0.0) && (uses_w(&p) || p.part2.as_deref().is_some_and(uses_w)) {
            p.rotation_4d = Some(rotation_matrix_4d(f.rotation_4d));
        }
        p
    }

    /// One chain: `slots` start at absolute slot index `first`; `repeat_from` is absolute.
    fn build(f: &Fractal, slots_in: &[Slot], first: usize, repeat_abs: usize, max_iterations: u32, const_base: usize) -> Program {
        let mut slots = Vec::new();
        let mut constants = Vec::new();
        let mut missing = Vec::new();
        let mut repeat_from = None;
        for (i, s) in slots_in.iter().enumerate() {
            if s.iterations == 0 {
                continue;
            }
            let Some(formula) = crate::formula::find(&s.formula) else {
                missing.push(s.formula.clone());
                continue;
            };
            if first + i >= repeat_abs && repeat_from.is_none() {
                repeat_from = Some(slots.len());
            }
            let values: Vec<f64> = formula.def.params.iter().map(|p| s.value(p)).collect();
            let offset = const_base + constants.len();
            constants.extend(formula.cpu.constants(&values));
            slots.push(ProgramSlot {
                transform: formula.def.transform,
                formula,
                iterations: s.iterations,
                offset,
                start: s.start,
                stop: s.stop.unwrap_or(u32::MAX),
                weight: s.weight,
                add_c: s.add_c,
                check_bailout: s.check_bailout,
            });
        }
        let difs = slots.iter().any(|s| s.formula.def.de == DeKind::Difs);
        let kinds: Vec<DeKind> = slots.iter().filter(|s| !s.transform && s.formula.def.de != DeKind::Difs).map(|s| s.formula.def.de).collect();
        let auto_kind = if kinds.iter().all(|k| *k == DeKind::Log) && !kinds.is_empty() { DeKind::Log } else { DeKind::Linear };
        let mixed = kinds.iter().any(|k| *k != kinds[0]) || kinds.contains(&DeKind::Numeric);
        let (de_kind, numeric) = match f.de_mode {
            DeMode::Auto => (if kinds.contains(&DeKind::Numeric) { DeKind::Log } else { auto_kind }, mixed),
            DeMode::Log => (DeKind::Log, false),
            DeMode::Linear => (DeKind::Linear, false),
            DeMode::NumericLog => (DeKind::Log, true),
            DeMode::NumericLinear => (DeKind::Linear, true),
        };
        // Per-formula DE scales, weighted by their iteration counts (as Mandelbulb3D does).
        let (mut wsum, mut wn) = (0.0, 0.0);
        for s in slots.iter().filter(|s| !s.transform) {
            wsum += s.formula.def.de_scale * s.iterations as f64;
            wn += s.iterations as f64;
        }
        Program {
            repeat_from: repeat_from.unwrap_or(0).min(slots.len().saturating_sub(1)),
            slots,
            max_iterations: max_iterations.max(1),
            min_iterations: f.min_iterations,
            c_factor: f.c_factor,
            bailout2: f.bailout * f.bailout,
            julia: f.julia,
            julia_c: f.julia_c,
            w_slice: f.w_slice,
            de_kind,
            numeric: numeric && !difs,
            difs,
            de_scale: if wn > 0.0 { wsum / wn } else { 1.0 },
            scene_de_scale: f.de_scale,
            constants,
            cuts: Vec::new(),
            missing,
            rotation_4d: None,
            part2: None,
            combine_mode: CombineMode::Min,
            combine_smooth: 0.0,
            mix_pow: 2.0,
            blend: None,
            reflections: false,
            deep: None,
            color2: (0, 1.0),
            color_on_it: -1,
            inside: f.inside,
            objects: Vec::new(),
            main_transform: None,
            layers: false,
            bump: false,
        }
    }

    /// The program of a whole scene: its fractal and the enabled objects of its composition.
    pub fn for_scene(scene: &crate::Scene) -> Program {
        use crate::compose::{MAX_OBJECTS, Shape};
        let mut p = Program::new(&scene.fractal, &scene.cut);
        p.main_transform = MainTransform::of(&scene.fractal.transform);
        let m = &scene.material;
        p.color2 = (m.second_choice.index(), m.lli_multiplier);
        p.color_on_it = m.color_on_iteration;
        for (i, o) in scene.objects.iter().enumerate().take(MAX_OBJECTS) {
            if !o.enabled {
                continue;
            }
            let shape = match &o.shape {
                Shape::Fractal(f) => {
                    // With DE combination and interpolation; no 4D rotation (the GPU has one
                    // rotation for the main fractal only).
                    let mut op = Self::new_at(f, &crate::scene::Cutting::default(), p.constants.len());
                    op.rotation_4d = None;
                    // The Mandelbulber per-slot settings belong to the main fractal (the GPU
                    // keeps only iteration counts per object slot).
                    op.clear_extras();
                    op.color2 = p.color2;
                    p.constants.append(&mut op.constants);
                    p.missing.append(&mut op.missing);
                    ObjectShape::Fractal(Box::new(op))
                }
                Shape::Primitive(pr) => ObjectShape::Primitive(pr.index(), pr.params()),
                Shape::Model(m) => match crate::mesh::request(&m.path, &m.options) {
                    crate::mesh::Status::Ready(v) => ObjectShape::Model(Some(crate::mesh::Model::new(v, m))),
                    _ => ObjectShape::Model(None),
                },
            };
            p.objects.push(ObjectProgram {
                shape,
                inv: o.transform.inverse_rotation(),
                pos: o.transform.position,
                size: o.transform.size(),
                scale: o.transform.size().x.min(o.transform.size().y).min(o.transform.size().z),
                op: o.op.index(),
                smooth: o.smooth.max(0.0),
                material: if o.material.own { i + 1 } else { 0 },
                repeat: o.repeat.map(|c| c.max(0.0)),
                repeat_limit: o.repeat_limit,
            });
        }
        let used = std::iter::once(&scene.material.layers).chain(p.objects.iter().filter(|o| o.material > 0).map(|o| &scene.objects[o.material - 1].material.layers));
        for l in used.flatten().filter(|l| l.enabled) {
            p.layers = true;
            p.bump |= l.target.bends();
        }
        p
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Identifies the generated shader: formulas per slot and the DE variant of each part, and
    /// whether the 4D rotation is used. Parameter values are not part of it.
    /// Mix combination: the second chain is a dIFS measured where the first chain ends.
    pub fn is_mix(&self) -> bool {
        self.combine_mode == CombineMode::Mix && self.part2.as_deref().is_some_and(|p| p.difs)
    }

    /// Any slot (of either chain) uses the Mandelbulber per-slot settings, or the minimum
    /// iterations or constant factor are set: the shader then includes that code.
    pub fn slot_extras(&self) -> bool {
        let uses = |p: &Program| {
            p.min_iterations > 0 || p.c_factor != [1.0; 3] || p.slots.iter().any(|s| s.start > 0 || s.stop != u32::MAX || s.weight != 1.0 || !s.add_c || !s.check_bailout)
        };
        uses(self) || self.part2.as_deref().is_some_and(uses)
    }

    /// Drops the per-slot settings, the minimum iterations and the constant factor.
    pub fn clear_extras(&mut self) {
        let clear = |p: &mut Program| {
            p.min_iterations = 0;
            p.c_factor = [1.0; 3];
            for s in &mut p.slots {
                (s.start, s.stop, s.weight, s.add_c, s.check_bailout) = (0, u32::MAX, 1.0, true, true);
            }
        };
        clear(self);
        if let Some(p2) = self.part2.as_deref_mut() {
            clear(p2);
        }
    }

    pub fn signature(&self) -> String {
        let part = |p: &Program| -> String {
            let mut s: String = p.slots.iter().map(|sl| format!("{}|{:x}|", sl.formula.def.name, sl.formula.text_hash())).collect();
            s.push_str(&format!("{:?}{}{}", p.de_kind, p.numeric, p.difs));
            s
        };
        let mut s = part(self);
        if let Some(p2) = &self.part2 {
            s.push_str("||");
            s.push_str(&part(p2));
        }
        s.push_str(if self.rotation_4d.is_some() { "|r4" } else { "" });
        s.push_str(if self.blend.is_some() { "|blend" } else { "" });
        s.push_str(if self.reflections { "|refl" } else { "" });
        s.push_str(if self.deep.is_some() { "|deep" } else { "" });
        s.push_str(if self.main_transform.is_some() { "|mt" } else { "" });
        s.push_str(if self.color_on_it >= 0 { "|coi" } else { "" });
        s.push_str(if self.is_mix() { "|mix" } else { "" });
        s.push_str(if self.slot_extras() { "|extras" } else { "" });
        s.push_str(if self.layers { "|layers" } else { "" });
        s.push_str(if self.bump { "|bump" } else { "" });
        for o in &self.objects {
            match &o.shape {
                ObjectShape::Fractal(f) => {
                    s.push_str("|obj:");
                    s.push_str(&part(f));
                }
                ObjectShape::Primitive(k, _) => s.push_str(&format!("|prim:{k}")),
                ObjectShape::Model(_) => s.push_str("|model"),
            }
        }
        s
    }

    pub fn evaluator(&self) -> Evaluator<'_> {
        let regs = |p: &Program| -> Vec<Vec<f64>> {
            p.slots
                .iter()
                .map(|s| {
                    let mut r = vec![0.0; s.formula.cpu.iter_regs];
                    let n = s.formula.def.constant_count();
                    r[STATE_COUNT..STATE_COUNT + n].copy_from_slice(&self.constants[s.offset..s.offset + n]);
                    r
                })
                .collect()
        };
        let regs_obj = self
            .objects
            .iter()
            .map(|o| match &o.shape {
                ObjectShape::Fractal(f) => (regs(f), f.part2.as_deref().map(regs).unwrap_or_default()),
                ObjectShape::Primitive(..) | ObjectShape::Model(_) => (Vec::new(), Vec::new()),
            })
            .collect();
        Evaluator { prog: self, regs: regs(self), regs2: self.part2.as_deref().map(regs).unwrap_or_default(), regs_obj, flip: false }
    }
}

/// Register files of a chain's slots.
type Regs = Vec<Vec<f64>>;

/// Per-thread evaluation state (register files of the slots).
pub struct Evaluator<'a> {
    prog: &'a Program,
    regs: Vec<Vec<f64>>,
    regs2: Vec<Vec<f64>>,
    /// Per object: the register files of its chains (first, second).
    regs_obj: Vec<(Regs, Regs)>,
    /// Swaps inside and outside rendering (rays travelling inside glass).
    pub flip: bool,
}

struct Iterated {
    state: [f64; 4],
    rold: f64,
    dr: f64,
    n: u32,
    trap: f64,
    escaped: bool,
    /// dIFS: smallest distance over the iterations.
    difs_de: f64,
    /// The constant (Julia seed or start point).
    c: [f64; 3],
}

/// Mandelbulb3D's second color choice (CalcThread / Calc.RMdoColor) as 0..1: the 15-bit value
/// divided by 32767. `option` is `SecondChoice::index`, `mul` the last-length-increase
/// multiplier, `s` the final vector, `c` the constant, `r2` / `rold` the last two squared
/// lengths, `trap` the smallest squared length (march.wgsl `color2_value`).
pub fn color2_value(option: u32, mul: f64, s: [f64; 4], c: [f64; 3], r2: f64, rold: f64, trap: f64) -> f64 {
    let pi = std::f64::consts::PI;
    let v = match option {
        1 => (r2 / (rold + 1.0)).ln() * mul * 512.0,
        2 => ((s[1] - c[1]).atan2(s[0] - c[0]) + pi) * 5200.0,
        3 => ((s[2] - c[2]).atan2(s[0] - c[0]) + pi) * 5200.0,
        4 => ((s[2] - c[2]).atan2(s[1] - c[1]) + pi) * 5200.0,
        5 => (s[0].atan2(s[1]) + pi) * 5215.0,
        _ => trap * 4096.0,
    };
    if v.is_finite() { (v / 32767.0).clamp(0.0, 1.0) } else { 0.0 }
}

/// Iterates `p` through the chain `prog`. `top` holds the shared settings (Julia, 4D rotation).
fn iterate(top: &Program, prog: &Program, regs: &mut [Vec<f64>], p: V3, fixed: Option<u32>) -> Iterated {
    iterate_from(top, prog, regs, p, fixed, None, false)
}

/// [`iterate`] that starts from `start` instead of the point (the second chain of a mix).
/// [`iterate`] from `start` (a mix's second chain). With `fixed` the count is fixed and there is
/// no bailout, unless `color_bail`: then the bailout is raised to `bailout² · 64` (Mandelbulb3D's
/// color on iteration, march.wgsl `color_bail`).
fn iterate_from(top: &Program, prog: &Program, regs: &mut [Vec<f64>], p: V3, fixed: Option<u32>, start: Option<[f64; 4]>, color_bail: bool) -> Iterated {
    let bail2 = if color_bail { prog.bailout2 * prog.bailout2 * 64.0 } else { prog.bailout2 };
    let check = fixed.is_none() || color_bail;
    let mut st = [0.0; STATE_COUNT];
    let mut v = [p.x, p.y, p.z, top.w_slice];
    if let Some(m) = &top.rotation_4d {
        let src = [p.x, p.y, p.z, 0.0];
        for (i, row) in m.iter().enumerate() {
            v[i] = (0..4).map(|k| row[k] * src[k]).sum();
        }
    }
    st[..9].copy_from_slice(&[v[0], v[1], v[2], v[3], v[0], v[1], v[2], v[3], 1.0]);
    if top.julia {
        st[4..8].copy_from_slice(&top.julia_c);
    }
    for k in 0..3 {
        st[4 + k] *= prog.c_factor[k];
    }
    // A mix continues from the first chain's point; the constant stays the original one.
    if let Some(s) = start {
        v = s;
        st[..4].copy_from_slice(&s);
    }
    st[STATE_ROUT] = v.iter().map(|x| x * x).sum();
    st[STATE_PX..STATE_PX + 3].copy_from_slice(&[p.x, p.y, p.z]);
    debug_assert_eq!(STATE_AUX + 9, STATE_COUNT);
    st[STATE_JULIA] = if top.julia { 1.0 } else { 0.0 };
    let mut out = Iterated { state: [0.0; 4], rold: 0.0, dr: 1.0, n: 0, trap: f64::MAX, escaped: false, difs_de: 65535.0, c: [st[4], st[5], st[6]] };
    if prog.difs {
        // VaryScale starts at 1; the shapes' distances are divided by it.
        st[STATE_AUX] = 1.0;
    }
    if prog.slots.is_empty() {
        return out;
    }
    let limit = fixed.unwrap_or(prog.max_iterations);
    if let Some([w1, w2]) = prog.blend {
        // Interpolation hybrid (doInterpolHybridPas): both formulas from the same point, the
        // results blended by direction and length; the derivative from absolute values.
        while out.n < limit {
            st[STATE_ROLD] = st[STATE_ROUT];
            st[STATE_ITER] = out.n as f64;
            let input = st;
            regs[0][..STATE_COUNT].copy_from_slice(&st);
            (prog.slots[0].formula.cpu.iter)(&mut regs[0]);
            let a: [f64; STATE_COUNT] = regs[0][..STATE_COUNT].try_into().unwrap_or(st);
            let mut b = a;
            b[..4].copy_from_slice(&input[..4]);
            b[STATE_DR] = input[STATE_DR];
            regs[1][..STATE_COUNT].copy_from_slice(&b);
            (prog.slots[1].formula.cpu.iter)(&mut regs[1]);
            st.copy_from_slice(&regs[1][..STATE_COUNT]);
            let len = |v: &[f64]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let l = len(&a) * w1 + len(&st) * w2;
            for k in 0..4 {
                st[k] = a[k] * w1 + st[k] * w2;
            }
            st[STATE_DR] = a[STATE_DR].abs() * w1 + st[STATE_DR].abs() * w2;
            let f = l / (len(&st).powi(2) + 1e-40).sqrt();
            for v in st.iter_mut().take(3) {
                *v *= f;
            }
            out.n += 1;
            let r2 = l * l;
            st[STATE_ROUT] = r2;
            out.trap = out.trap.min(r2);
            // NaN counts as escaped.
            if check && (r2.is_nan() || r2 > bail2) {
                out.escaped = true;
                break;
            }
        }
        out.state.copy_from_slice(&st[..4]);
        out.rold = st[STATE_ROLD];
        out.dr = st[STATE_DR];
        return out;
    }
    let (mut slot, mut count) = (0usize, 0u32);
    // Transforms do not count as iterations; the step cap ends chains of only transforms.
    let cap = limit.saturating_mul(8).saturating_add(64);
    let mut steps = 0u32;
    // Slots skipped in a row (iteration ranges): all of them out of range ends the orbit.
    let mut skipped = 0usize;
    while out.n < limit && steps < cap {
        steps += 1;
        let s = &prog.slots[slot];
        if out.n < s.start || out.n > s.stop {
            count = 0;
            slot += 1;
            if slot >= prog.slots.len() {
                slot = prog.repeat_from;
            }
            skipped += 1;
            if skipped > prog.slots.len() {
                break;
            }
            continue;
        }
        skipped = 0;
        let regs = &mut regs[slot];
        let prev = st;
        st[STATE_ROLD] = st[STATE_ROUT];
        st[STATE_ITER] = out.n as f64;
        regs[..STATE_COUNT].copy_from_slice(&st);
        if !s.add_c {
            regs[4..8].fill(0.0);
        }
        (s.formula.cpu.iter)(regs);
        st.copy_from_slice(&regs[..STATE_COUNT]);
        if !s.add_c {
            st[4..8].copy_from_slice(&prev[4..8]);
        }
        if s.weight != 1.0 {
            // Blend with the point before the formula (Mandelbulber's formula weight).
            for k in 0..4 {
                st[k] = prev[k] + (st[k] - prev[k]) * s.weight;
            }
            st[STATE_DR] = prev[STATE_DR] + (st[STATE_DR] - prev[STATE_DR]) * s.weight;
        }
        if !s.transform {
            out.n += 1;
        }
        count += 1;
        if count >= s.iterations {
            count = 0;
            slot += 1;
            if slot >= prog.slots.len() {
                slot = prog.repeat_from;
            }
        }
        if prog.difs {
            if !s.transform {
                let de = st[STATE_ROUT] / st[STATE_AUX];
                if de < out.difs_de {
                    out.difs_de = de;
                    // Coloring: the value the shape left in Dfree1 (a2).
                    out.trap = st[STATE_AUX + 2] * st[STATE_AUX + 2];
                }
            }
            continue;
        }
        if !s.transform {
            let r2 = st[0] * st[0] + st[1] * st[1] + st[2] * st[2] + st[3] * st[3];
            out.trap = out.trap.min(r2);
            st[STATE_ROUT] = r2;
            // NaN counts as escaped (it never comes back): `inside` is false for NaN.
            let inside = r2 <= bail2;
            if check && !inside && s.check_bailout && out.n >= prog.min_iterations {
                out.escaped = true;
                // Overflow in the last step: the estimate uses the state before it.
                if !r2.is_finite() {
                    st = prev;
                }
                break;
            }
        }
    }
    out.state.copy_from_slice(&st[..4]);
    out.rold = st[STATE_ROLD];
    out.dr = st[STATE_DR];
    out
}

/// Distance estimate of one chain (before the scene's DE scale and the cutting planes).
/// The orbit of a fractal (main or object) with its DE combination.
fn fractal_orbit(prog: &Program, regs: &mut [Vec<f64>], regs2: &mut [Vec<f64>], p: V3, h: f64) -> Orbit {
    if let Some(p2) = prog.part2.as_deref().filter(|_| prog.is_mix()) {
        // Mix: the first chain transforms the point, the dIFS measures the distance there,
        // scaled back by the first chain's derivative (Calc.pas DEcomb types 6/7).
        let a = iterate(prog, prog, regs, p, None);
        let b = iterate_from(prog, p2, regs2, p, None, Some(a.state), false);
        let d = b.difs_de.max(0.0) * p2.de_scale;
        let de = if prog.de_kind == DeKind::Linear && !prog.difs { d / a.dr.abs().max(1e-30) } else { d * prog.mix_pow.abs().max(1e-30).powf(-1.0 - a.n as f64) };
        let b_r2 = b.state.iter().map(|v| v * v).sum();
        let color2 = color2_value(prog.color2.0, prog.color2.1, b.state, b.c, b_r2, b.rold, b.trap);
        return Orbit { de, iterations: b.n, r2: a.state.iter().map(|v| v * v).sum(), r2_old: a.rold, dr: a.dr, trap: b.trap, color2, escaped: true, object: 0 };
    }
    let o = chain_orbit(prog, prog, regs, p, h);
    match prog.part2.as_deref() {
        Some(p2) => combine(prog.combine_mode, prog.combine_smooth, o, chain_orbit(prog, p2, regs2, p, h)),
        None => o,
    }
}

fn chain_orbit(top: &Program, prog: &Program, regs: &mut [Vec<f64>], p: V3, h: f64) -> Orbit {
    if prog.slots.is_empty() {
        return Orbit { de: 1.0e10, ..Orbit::default() };
    }
    let it = iterate(top, prog, regs, p, None);
    let r2 = it.state.iter().map(|v| v * v).sum::<f64>();
    let r = r2.sqrt();
    let mut de = if prog.difs {
        it.difs_de.max(0.0)
    } else if !it.escaped {
        0.0
    } else if prog.numeric {
        // Gradient step: at least a little relative to the point, against cancellation in the
        // differences (f64; the single precision GPU shader uses 2e-6).
        let h = h.max(p.length() * NUMERIC_STEP_F64);
        let mut g = [0.0; 3];
        for (k, gk) in g.iter_mut().enumerate() {
            let mut q = p;
            match k {
                0 => q.x += h,
                1 => q.y += h,
                _ => q.z += h,
            }
            let o = iterate(top, prog, regs, q, Some(it.n));
            *gk = (o.state.iter().map(|v| v * v).sum::<f64>().sqrt() - r) / h;
        }
        let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt().max(1e-30);
        match prog.de_kind {
            DeKind::Linear => r / gl,
            _ => 0.5 * r * r.ln() / gl,
        }
    } else {
        match prog.de_kind {
            DeKind::Linear => r / it.dr.abs(),
            _ => 0.5 * r.ln() * r / it.dr,
        }
    };
    if !de.is_finite() {
        de = if it.escaped { 1.0e-3 } else { 0.0 };
    }
    let color2 = color2_value(top.color2.0, top.color2.1, it.state, it.c, r2, it.rold, it.trap);
    Orbit { de: de * prog.de_scale, iterations: it.n, r2, r2_old: it.rold, dr: it.dr, trap: it.trap, color2, escaped: it.escaped || prog.difs, object: 0 }
}

/// Combines the estimates of the two chains (Mandelbulb3D's DE combination).
pub fn combine(mode: CombineMode, smooth: f64, a: Orbit, b: Orbit) -> Orbit {
    let s = smooth.max(1e-12);
    match mode {
        // Mix without a dIFS second chain: a union.
        CombineMode::Min | CombineMode::Mix => {
            if b.de < a.de {
                b
            } else {
                a
            }
        }
        CombineMode::Max => {
            if b.de > a.de {
                b
            } else {
                a
            }
        }
        // Inside the second chain counts as outside: its estimate becomes the distance into it.
        CombineMode::Subtract => {
            let inv = if b.de <= 0.0 { s } else { -b.de };
            if inv > a.de { Orbit { de: inv, ..a } } else { a }
        }
        CombineMode::SmoothLinear => {
            let de = (b.de - (s - a.de).max(0.0)).min(a.de - (s - b.de).max(0.0));
            if b.de < a.de { Orbit { de, ..b } } else { Orbit { de, ..a } }
        }
        CombineMode::Smooth => {
            let x = (s - a.de).max(0.0);
            let y = (s - b.de).max(0.0);
            let de = (b.de - x * (s - y) / s).min(a.de - y * (s - x) / s);
            if b.de < a.de { Orbit { de, ..b } } else { Orbit { de, ..a } }
        }
    }
}

impl Evaluator<'_> {
    /// Iterates `p` and estimates its distance to the surface. `h` is the offset of the
    /// numerical gradient (about the surface threshold).
    /// Mandelbulb3D's color on iteration (`Program::color_on_it` ≥ 0) for a surface of the main
    /// fractal at `p` with orbit `o`: the color values of the orbit after that many iterations
    /// (no bailout; 0: the constant itself), the rest of `o` kept. None when it is off or does
    /// not apply (DE combinations, objects).
    pub fn color_orbit(&mut self, p: V3, o: &Orbit) -> Option<Orbit> {
        let prog = self.prog;
        if prog.color_on_it < 0 || o.object != 0 || prog.part2.is_some() || prog.slots.is_empty() {
            return None;
        }
        let p = prog.main_transform.as_ref().map_or(p, |t| t.local(p));
        let mut it = iterate_from(prog, prog, &mut self.regs, p, Some(prog.color_on_it as u32), None, true);
        if prog.color_on_it == 0 {
            it.state = [it.c[0], it.c[1], it.c[2], 0.0];
        }
        let r2 = it.state.iter().map(|v| v * v).sum::<f64>();
        let trap = if it.trap == f64::MAX { r2 } else { it.trap };
        let color2 = color2_value(prog.color2.0, prog.color2.1, it.state, it.c, r2, it.rold, trap);
        Some(Orbit { iterations: it.n, r2, r2_old: it.rold, trap, color2, escaped: false, ..*o })
    }

    pub fn orbit(&mut self, p_world: V3, h_world: f64) -> Orbit {
        let prog = self.prog;
        // The main fractal in its own coordinates (objects and cuts stay in the world).
        let (p, h, t_scale) = match &prog.main_transform {
            Some(t) => (t.local(p_world), h_world / t.scale, t.scale),
            None => (p_world, h_world, 1.0),
        };
        let mut o = fractal_orbit(prog, &mut self.regs, &mut self.regs2, p, h);
        let mut de = (o.de * prog.scene_de_scale * t_scale).max(0.0);
        let (p, h) = (p_world, h_world);
        // Scene composition: the objects in order, each in its own coordinates.
        for (k, ob) in prog.objects.iter().enumerate() {
            let q = ob.local(p);
            let (d, orbit) = match &ob.shape {
                ObjectShape::Fractal(f) => {
                    let (r1, r2) = &mut self.regs_obj[k];
                    let oo = fractal_orbit(f, r1, r2, q, h / ob.scale);
                    let mut d = (oo.de * f.scene_de_scale).max(0.0) * ob.scale;
                    // A fractal has no distance inside; cutting with it needs one.
                    if d <= 0.0 && matches!(ob.op, 1 | 4) {
                        d = -2.0 * h;
                    }
                    (d, oo)
                }
                ObjectShape::Primitive(kind, a) => (crate::compose::primitive_distance(*kind, *a, q) * ob.scale, Orbit { dr: 1.0, escaped: true, ..Orbit::default() }),
                ObjectShape::Model(m) => (m.as_ref().map_or(crate::compose::NO_MODEL, |m| m.vol.distance(q.to_array())) * ob.scale, Orbit { dr: 1.0, escaped: true, ..Orbit::default() }),
            };
            let (nd, wins) = crate::compose::combine(ob.op, de, d, ob.smooth);
            de = nd;
            if wins {
                o = Orbit { object: k as u32 + 1, ..orbit };
            }
        }
        if prog.inside != self.flip {
            // Mandelbulb3D's inside rendering: points in the set step on with four times the
            // surface threshold (h is half of it); outside, the estimate falls below the
            // threshold one threshold beyond the boundary.
            let s = 2.0 * h;
            de = if prog.difs { 2.0 * s - de } else { 4.0 * s - 3.0 * de }.max(0.0);
        }
        for &(axis, pos, above) in &prog.cuts {
            let v = [p.x, p.y, p.z][axis];
            de = de.max(if above { pos - v } else { v - pos });
        }
        o.de = de;
        o
    }

    pub fn de(&mut self, p: V3, h: f64) -> f64 {
        self.orbit(p, h).de
    }

    /// Whether the estimate is the inside one (inside rendering, or a ray inside glass).
    pub fn inside(&self) -> bool {
        self.prog.inside != self.flip
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::v3;
    use crate::scene::Scene;

    #[test]
    fn mandelbulb_distance_is_plausible() {
        let s = Scene::default();
        let prog = Program::for_scene(&s);
        assert!(prog.missing.is_empty());
        let mut ev = prog.evaluator();
        // Far away: roughly the distance to the bulb (radius about 1.1).
        let d = ev.de(v3(3.0, 0.0, 0.0), 1e-5);
        assert!(d > 1.0 && d < 2.2, "{d}");
        // The origin is inside.
        assert_eq!(ev.de(V3::ZERO, 1e-5), 0.0);
    }

    #[test]
    fn all_builtin_formulas_evaluate() {
        let lib = crate::formula::library();
        for f in lib.all() {
            let mut s = Scene::default();
            s.fractal.slots = vec![crate::scene::Slot::new(&f.def.name)];
            if f.def.transform {
                s.fractal.slots.push(crate::scene::Slot::new("Mandelbulb"));
            }
            let prog = Program::for_scene(&s);
            let mut ev = prog.evaluator();
            let d = ev.de(v3(2.5, 0.3, 0.2), 1e-5);
            assert!(d.is_finite() && d >= 0.0, "{}: {d}", f.def.name);
        }
    }
}
