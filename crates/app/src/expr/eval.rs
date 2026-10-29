//! The interpreter: programs compiled to a small stack machine, run on a flat block of
//! values with no allocation per run.
//!
//! The block's layout (`Layout`): the shared values (`SHARED`), the frame outputs, the
//! solver parameters, the program's own variables, and (per pixel) the pixel inputs.

use super::{Binary, Expr, FRAME_OUTPUTS, Func, PIXEL_INPUTS, Program, Ref, SHARED, Unary};
use utils::rng::SplitMix64;

/// The maths both back ends share: each function here has a WGSL twin in `math.wgsl`,
/// so the CPU and the GPU agree.
pub mod math {
    pub fn div(a: f32, b: f32) -> f32 {
        if b == 0.0 { 0.0 } else { a / b }
    }

    pub fn modulo(a: f32, b: f32) -> f32 {
        if b == 0.0 { 0.0 } else { a - b * (a / b).floor() }
    }

    pub fn sqrt(a: f32) -> f32 {
        a.abs().sqrt()
    }

    /// Of the size of `a`, keeping its sign for whole powers (so pow(-2, 3) is -8).
    pub fn pow(a: f32, b: f32) -> f32 {
        let p = a.abs().powf(b);
        if a < 0.0 && b.fract() == 0.0 && (b.abs() % 2.0) == 1.0 { -p } else { p }
    }

    pub fn log(a: f32) -> f32 {
        a.max(1e-30).ln()
    }

    pub fn log10(a: f32) -> f32 {
        a.max(1e-30).log10()
    }

    pub fn asin(a: f32) -> f32 {
        a.clamp(-1.0, 1.0).asin()
    }

    pub fn acos(a: f32) -> f32 {
        a.clamp(-1.0, 1.0).acos()
    }

    pub fn sigmoid(a: f32, b: f32) -> f32 {
        1.0 / (1.0 + (-a * b).exp())
    }

    /// Halves round away from zero, as WGSL's `round` doesn't (it rounds to even).
    pub fn round(a: f32) -> f32 {
        (a.abs() + 0.5).floor().copysign(a)
    }

    pub fn truth(a: f32) -> bool {
        a != 0.0
    }

    pub fn bool(b: bool) -> f32 {
        if b { 1.0 } else { 0.0 }
    }
}

/// Where each kind of place lives in a program's block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub outputs: usize,
    pub params: usize,
    pub locals: usize,
    pub pixel: usize,
    pub len: usize,
}

impl Layout {
    pub fn of(p: &Program) -> Self {
        let outputs = SHARED;
        let params = outputs + FRAME_OUTPUTS.len();
        let locals = params + p.params.len();
        let pixel = locals + p.locals.len();
        Self { outputs, params, locals, pixel, len: pixel + PIXEL_INPUTS.len() }
    }

    pub fn index(&self, r: Ref) -> usize {
        match r {
            Ref::Shared(i) => i,
            Ref::Output(i) => self.outputs + i,
            Ref::Param(i) => self.params + i,
            Ref::Local(i) => self.locals + i,
            Ref::Pixel(i) => self.pixel + i,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Num(f32),
    Load(u32),
    Store(u32),
    Unary(Unary),
    Binary(Binary),
    Call(Func),
}

/// A program ready to run.
#[derive(Clone, Debug)]
pub struct Machine {
    ops: Vec<Op>,
    stack: Vec<f32>,
    pub layout: Layout,
}

impl Machine {
    pub fn new(p: &Program) -> Self {
        let layout = Layout::of(p);
        let mut ops = Vec::new();
        let mut depth = (0usize, 0usize);
        fn emit(e: &Expr, layout: &Layout, ops: &mut Vec<Op>, depth: &mut (usize, usize)) {
            let push = |ops: &mut Vec<Op>, op: Op, change: isize, depth: &mut (usize, usize)| {
                ops.push(op);
                depth.0 = (depth.0 as isize + change) as usize;
                depth.1 = depth.1.max(depth.0);
            };
            match e {
                Expr::Num(n) => push(ops, Op::Num(*n), 1, depth),
                Expr::Ref(r) => push(ops, Op::Load(layout.index(*r) as u32), 1, depth),
                Expr::Unary(op, a) => {
                    emit(a, layout, ops, depth);
                    push(ops, Op::Unary(*op), 0, depth);
                }
                Expr::Binary(op, a, b) => {
                    emit(a, layout, ops, depth);
                    emit(b, layout, ops, depth);
                    push(ops, Op::Binary(*op), -1, depth);
                }
                Expr::Call(f, args) => {
                    for a in args {
                        emit(a, layout, ops, depth);
                    }
                    push(ops, Op::Call(*f), 1 - args.len() as isize, depth);
                }
            }
        }
        for s in &p.stmts {
            emit(&s.expr, &layout, &mut ops, &mut depth);
            ops.push(Op::Store(layout.index(s.target) as u32));
            depth.0 -= 1;
        }
        Self { ops, stack: Vec::with_capacity(depth.1.max(1)), layout }
    }

    /// Run on `block` (at least `layout.len` values).
    pub fn run(&mut self, block: &mut [f32], rng: &mut SplitMix64) {
        use math::*;
        let s = &mut self.stack;
        s.clear();
        for &op in &self.ops {
            match op {
                Op::Num(n) => s.push(n),
                Op::Load(i) => s.push(block[i as usize]),
                Op::Store(i) => block[i as usize] = s.pop().unwrap_or(0.0),
                Op::Unary(u) => {
                    let a = s.pop().unwrap_or(0.0);
                    s.push(match u {
                        Unary::Neg => -a,
                        Unary::Not => bool(!truth(a)),
                    });
                }
                Op::Binary(b) => {
                    let y = s.pop().unwrap_or(0.0);
                    let x = s.pop().unwrap_or(0.0);
                    s.push(match b {
                        Binary::Add => x + y,
                        Binary::Sub => x - y,
                        Binary::Mul => x * y,
                        Binary::Div => div(x, y),
                        Binary::Mod => modulo(x, y),
                        Binary::Lt => bool(x < y),
                        Binary::Le => bool(x <= y),
                        Binary::Gt => bool(x > y),
                        Binary::Ge => bool(x >= y),
                        Binary::Eq => bool(x == y),
                        Binary::Ne => bool(x != y),
                        Binary::And => bool(truth(x) && truth(y)),
                        Binary::Or => bool(truth(x) || truth(y)),
                    });
                }
                Op::Call(f) => {
                    let mut arg = || s.pop().unwrap_or(0.0);
                    let v = match f {
                        Func::Rand => rng.next_f32(),
                        Func::RandInt => (rng.next_f32() * arg().max(0.0)).floor(),
                        Func::Sqr => {
                            let a = arg();
                            a * a
                        }
                        Func::Int => arg().trunc(),
                        Func::Bnot => bool(!truth(arg())),
                        Func::Sin => arg().sin(),
                        Func::Cos => arg().cos(),
                        Func::Tan => arg().tan(),
                        Func::Asin => asin(arg()),
                        Func::Acos => acos(arg()),
                        Func::Atan => arg().atan(),
                        Func::Abs => arg().abs(),
                        Func::Sqrt => sqrt(arg()),
                        Func::Exp => arg().exp(),
                        Func::Log => log(arg()),
                        Func::Log10 => log10(arg()),
                        Func::Floor => arg().floor(),
                        Func::Ceil => arg().ceil(),
                        Func::Round => round(arg()),
                        Func::Fract => {
                            let a = arg();
                            a - a.floor()
                        }
                        Func::Sign => {
                            let a = arg();
                            if a > 0.0 { 1.0 } else if a < 0.0 { -1.0 } else { 0.0 }
                        }
                        Func::Atan2 | Func::Pow | Func::Min | Func::Max | Func::Sigmoid | Func::Above | Func::Below | Func::Equal | Func::Band | Func::Bor => {
                            let (b, a) = (arg(), arg());
                            match f {
                                Func::Atan2 => a.atan2(b),
                                Func::Pow => pow(a, b),
                                Func::Min => a.min(b),
                                Func::Max => a.max(b),
                                Func::Sigmoid => sigmoid(a, b),
                                Func::Above => bool(a > b),
                                Func::Below => bool(a < b),
                                Func::Band => bool(truth(a) && truth(b)),
                                Func::Bor => bool(truth(a) || truth(b)),
                                _ => bool(a == b),
                            }
                        }
                        Func::Clamp => {
                            let (hi, lo, a) = (arg(), arg(), arg());
                            a.max(lo).min(hi)
                        }
                        Func::If => {
                            let (no, yes, c) = (arg(), arg(), arg());
                            if truth(c) { yes } else { no }
                        }
                    };
                    s.push(v);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Stage, compile, slot};
    use super::*;

    fn run(src: &str) -> (Machine, Vec<f32>) {
        let p = compile(src, Stage::Frame).unwrap();
        let mut m = Machine::new(&p);
        let mut block = vec![0.0; m.layout.len];
        m.run(&mut block, &mut SplitMix64::new(1));
        (m, block)
    }

    fn value(src: &str) -> f32 {
        let (m, block) = run(&format!("v = {src}"));
        block[m.layout.locals]
    }

    #[test]
    fn arithmetic_and_logic() {
        assert_eq!(value("1 + 2 * 3"), 7.0);
        assert_eq!(value("(1 + 2) * 3"), 9.0);
        assert_eq!(value("10 - 4 - 3"), 3.0);
        assert_eq!(value("-2 * -3"), 6.0);
        assert_eq!(value("7 % 3"), 1.0);
        assert_eq!(value("-1 % 3"), 2.0);
        assert_eq!(value("1 / 0"), 0.0);
        assert_eq!(value("2 < 3"), 1.0);
        assert_eq!(value("2 >= 3"), 0.0);
        assert_eq!(value("1 && 0 || 2"), 1.0);
        assert_eq!(value("!0"), 1.0);
        assert_eq!(value("if(0.5, 10, 20)"), 10.0);
        assert_eq!(value("if(0, 10, 20)"), 20.0);
    }

    #[test]
    fn functions_guard_their_domains() {
        assert_eq!(value("sqrt(-4)"), 2.0);
        assert_eq!(value("pow(-2, 3)"), -8.0);
        assert_eq!(value("pow(-4, 0.5)"), 2.0);
        assert!(value("log(0)").is_finite());
        assert!((value("asin(2)") - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(value("clamp(5, 0, 1)"), 1.0);
        assert_eq!(value("round(-2.5)"), -3.0);
        assert_eq!(value("fract(-0.25)"), 0.75);
        assert_eq!(value("min(3, 2) + max(3, 2)"), 5.0);
        assert!((value("sigmoid(0, 5)") - 0.5).abs() < 1e-6);
        assert_eq!(value("above(2, 1) + below(2, 1) + equal(2, 2)"), 2.0);
        let r = value("rand()");
        assert!((0.0..1.0).contains(&r));
    }

    /// MilkDrop's functions, for imported presets.
    #[test]
    fn milkdrop_functions() {
        assert_eq!(value("sqr(-3)"), 9.0);
        assert_eq!(value("int(-2.7) + int(2.7)"), 0.0);
        assert_eq!(value("band(1, 0.5) + bor(0, 0) + bnot(0)"), 2.0);
        let r = value("rand(10)");
        assert!(r == r.floor() && (0.0..10.0).contains(&r), "{r}");
    }

    /// Variables keep their values between runs; the shared block is read and written.
    #[test]
    fn programs_keep_state_and_set_shared_values() {
        let p = compile("n = n + 1; zoom = 1 + n * bass", Stage::Frame).unwrap();
        let mut m = Machine::new(&p);
        let mut block = vec![0.0; m.layout.len];
        block[slot("bass")] = 0.5;
        let mut rng = SplitMix64::new(1);
        for _ in 0..3 {
            m.run(&mut block, &mut rng);
        }
        assert_eq!(block[m.layout.locals], 3.0);
        assert_eq!(block[slot("zoom")], 2.5);
    }
}
