//! The looks' equation language (see `looks`): small, and close to MilkDrop's, so looks
//! are easy to write and classic presets easy to convert.
//!
//! A program is assignments, one per line or separated by `;`:
//!
//! ```text
//! zoom = 1 + 0.05 * bass_att      // comments run to the end of the line
//! rot = if(beat_phase < 0.5, 0.2, -0.2)
//! q1 = sin(time * 0.3)
//! ```
//!
//! - **Values** are numbers; comparisons and `&&`, `||`, `!` give 1 or 0, and anything
//!   but 0 counts as true. `if(c, a, b)` picks `a` when `c` is true.
//! - **Maths** is safe: dividing by 0 gives 0, and `sqrt`, `log` and `pow` guard their
//!   domains (see `eval::math`), the same on the CPU and the GPU.
//! - **Names** are case-insensitive. The inputs (`INPUTS`) are read-only. Per frame, a
//!   program sets the motion (`MOTION`), the frame outputs (`FRAME_OUTPUTS`), `q1` to
//!   `q8` (shared with the per-pixel program), solver parameters as `param_<key>`, and
//!   its own variables, which keep their values from frame to frame. Per pixel, it also
//!   reads `x`, `y`, `rad` and `ang`, and sets the motion and its own temporaries. A
//!   name that's read but never assigned, and isn't one of these, is an error (a typo).
//!
//! Two back ends: the interpreter (`eval`) runs per-frame programs, and the WGSL
//! generator (`wgsl`) turns per-pixel ones into shader code.

pub mod eval;
pub mod wgsl;

use std::fmt;

/// Where a program runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Frame,
    Pixel,
}

/// The read-only inputs, set before every frame: the clock, the music (MilkDrop's
/// `bass`, `mid`, `treb` and their `_att`; the five bands; hits and beats as pulses that
/// fall from 1; the song's sections: novelty, build-up, and changes as pulses) and the
/// picture.
pub const INPUTS: &[&str] = &[
    "time", "frame", "fps", "dt", "aspect", "bass", "mid", "treb", "bass_att", "mid_att", "treb_att", "band_sub", "band_bass",
    "band_lowmid", "band_mid", "band_high", "level", "brightness", "loudness", "kick", "snare", "hat", "beat", "beat_phase",
    "bar_phase", "bpm", "beating", "silent", "novelty", "buildup", "section", "drop", "breakdown",
];
/// The warp, per second: zoom (1 none), rotation (radians), shift (`dx`, `dy`: screen
/// heights, y up), the centre of zoom and rotation (`cx`, `cy`: 0..1, y up), stretch
/// (`sx`, `sy`: 1 none) and the wobble's amount.
pub const MOTION: &[&str] = &["zoom", "rot", "dx", "dy", "cx", "cy", "sx", "sy", "warp"];
/// Shared from the per-frame program to the per-pixel one: `q1` to `q8`.
pub const QS: usize = 8;
/// The block of values both stages see, in this order: inputs, motion, the q's. It's
/// what the per-pixel shader reads.
pub const SHARED: usize = INPUTS.len() + MOTION.len() + QS;
/// What only a per-frame program sets: the feedback's other controls, the waves and
/// shapes, gravity's tilt (degrees) and the reactions' intensity (a multiple).
pub const FRAME_OUTPUTS: &[&str] = &[
    "trail", "warp_speed", "flow", "echo", "echo_zoom", "wave_scale", "wave_size", "wave_y", "wave_thickness", "wave_brightness",
    "wave_push", "shape1_radius", "shape1_x", "shape1_y", "shape1_spin", "shape1_fill", "shape2_radius", "shape2_x", "shape2_y",
    "shape2_spin", "shape2_fill", "gravity", "intensity",
];
/// Only per pixel: the position (0..1, y up), the distance from the centre (1 at the
/// top and bottom edges) and the angle (radians, anticlockwise from the right).
pub const PIXEL_INPUTS: &[&str] = &["x", "y", "rad", "ang"];
/// Solver parameters are set as `param_<key>`.
pub const PARAM_PREFIX: &str = "param_";

/// The slot of a shared name (input, motion or q).
pub fn shared_slot(name: &str) -> Option<usize> {
    if let Some(i) = INPUTS.iter().position(|n| *n == name) {
        return Some(i);
    }
    if let Some(i) = MOTION.iter().position(|n| *n == name) {
        return Some(INPUTS.len() + i);
    }
    let q = name.strip_prefix('q')?.parse::<usize>().ok()?;
    (1..=QS).contains(&q).then(|| INPUTS.len() + MOTION.len() + q - 1)
}

/// The slot of an input or motion name, for the caller filling the shared block.
pub fn slot(name: &str) -> usize {
    shared_slot(name).unwrap_or_else(|| panic!("no shared name {name}"))
}

/// A place a program reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ref {
    /// The shared block (`SHARED` slots).
    Shared(usize),
    /// A frame output (`FRAME_OUTPUTS` index).
    Output(usize),
    /// A solver parameter (`Program::params` index).
    Param(usize),
    /// The program's own variable (`Program::locals` index).
    Local(usize),
    /// A per-pixel input (`PIXEL_INPUTS` index).
    Pixel(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binary {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Abs,
    Sqrt,
    Pow,
    Exp,
    Log,
    Log10,
    Min,
    Max,
    Floor,
    Ceil,
    Round,
    Fract,
    Sign,
    Clamp,
    Sigmoid,
    If,
    Above,
    Below,
    Equal,
    Rand,
}

/// The functions: name, arity.
const FUNCS: &[(&str, Func, usize)] = &[
    ("sin", Func::Sin, 1),
    ("cos", Func::Cos, 1),
    ("tan", Func::Tan, 1),
    ("asin", Func::Asin, 1),
    ("acos", Func::Acos, 1),
    ("atan", Func::Atan, 1),
    ("atan2", Func::Atan2, 2),
    ("abs", Func::Abs, 1),
    ("sqrt", Func::Sqrt, 1),
    ("pow", Func::Pow, 2),
    ("exp", Func::Exp, 1),
    ("log", Func::Log, 1),
    ("log10", Func::Log10, 1),
    ("min", Func::Min, 2),
    ("max", Func::Max, 2),
    ("floor", Func::Floor, 1),
    ("ceil", Func::Ceil, 1),
    ("round", Func::Round, 1),
    ("fract", Func::Fract, 1),
    ("sign", Func::Sign, 1),
    ("clamp", Func::Clamp, 3),
    ("sigmoid", Func::Sigmoid, 2),
    ("if", Func::If, 3),
    ("above", Func::Above, 2),
    ("below", Func::Below, 2),
    ("equal", Func::Equal, 2),
    ("rand", Func::Rand, 0),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f32),
    Ref(Ref),
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub target: Ref,
    pub expr: Expr,
}

/// A program, its names resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub stage: Stage,
    pub stmts: Vec<Stmt>,
    /// Its own variables (per frame: kept between frames; per pixel: temporaries).
    pub locals: Vec<String>,
    /// The solver parameters it sets (keys, without the prefix).
    pub params: Vec<String>,
}

impl Program {
    /// No statements.
    pub fn empty(stage: Stage) -> Self {
        Self { stage, stmts: Vec::new(), locals: Vec::new(), params: Vec::new() }
    }
}

/// An error, at a byte offset into the source (and its line and column, from 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for Error {}

fn error(src: &str, offset: usize, message: String) -> Error {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    Error { offset, line, column, message }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Num(f32),
    Name(String),
    Op(&'static str),
    End,
}

const OPS: &[&str] = &["==", "!=", "<=", ">=", "&&", "||", "+", "-", "*", "/", "%", "(", ")", ",", ";", "=", "<", ">", "!"];

fn lex(src: &str) -> Result<Vec<(Token, usize)>, Error> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if src[i..].starts_with("//") {
            i = src[i..].find('\n').map_or(bytes.len(), |n| i + n);
        } else if c.is_ascii_digit() || (c == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                let mut j = i + 1;
                if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j].is_ascii_digit() {
                    i = j;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text = &src[start..i];
            let value: f32 = text.parse().ok().filter(|v: &f32| v.is_finite()).ok_or_else(|| error(src, start, format!("`{text}` isn't a number this can hold")))?;
            tokens.push((Token::Num(value), start));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push((Token::Name(src[start..i].to_ascii_lowercase()), start));
        } else if let Some(op) = OPS.iter().find(|op| src[i..].starts_with(**op)) {
            tokens.push((Token::Op(op), i));
            i += op.len();
        } else {
            let ch = src[i..].chars().next().unwrap_or('?');
            let hint = if ch == '^' { " (for powers, use pow(a, b))" } else { "" };
            return Err(error(src, i, format!("unexpected `{ch}`{hint}")));
        }
    }
    tokens.push((Token::End, src.len()));
    Ok(tokens)
}

/// The parse tree before names are resolved.
enum Raw {
    Num(f32),
    Name(String, usize),
    Unary(Unary, Box<Raw>),
    Binary(Binary, Box<Raw>, Box<Raw>),
    Call(Func, Vec<Raw>),
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<(Token, usize)>,
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.tokens[self.at].0
    }

    fn offset(&self) -> usize {
        self.tokens[self.at].1
    }

    fn next(&mut self) -> (Token, usize) {
        let t = self.tokens[self.at].clone();
        if self.at + 1 < self.tokens.len() {
            self.at += 1;
        }
        t
    }

    fn fail<T>(&self, message: String) -> Result<T, Error> {
        Err(error(self.src, self.offset(), message))
    }

    fn describe(t: &Token) -> String {
        match t {
            Token::Num(n) => format!("the number {n}"),
            Token::Name(n) => format!("`{n}`"),
            Token::Op(op) => format!("`{op}`"),
            Token::End => "the end".into(),
        }
    }

    fn expect(&mut self, op: &str) -> Result<(), Error> {
        if *self.peek() == Token::Op(match OPS.iter().find(|o| **o == op) {
            Some(o) => o,
            None => unreachable!(),
        }) {
            self.next();
            Ok(())
        } else {
            self.fail(format!("expected `{op}`, found {}", Self::describe(self.peek())))
        }
    }

    fn statements(&mut self) -> Result<Vec<(String, usize, Raw)>, Error> {
        let mut out = Vec::new();
        loop {
            while *self.peek() == Token::Op(";") {
                self.next();
            }
            match self.next() {
                (Token::End, _) => return Ok(out),
                (Token::Name(name), at) => {
                    if *self.peek() != Token::Op("=") {
                        return self.fail(format!("expected `=` after `{name}`, found {}", Self::describe(self.peek())));
                    }
                    self.next();
                    let expr = self.expr(0)?;
                    match self.peek() {
                        Token::Op(";") | Token::End | Token::Name(_) => {}
                        t => return self.fail(format!("unexpected {} after the expression", Self::describe(t))),
                    }
                    out.push((name, at, expr));
                }
                (t, at) => return Err(error(self.src, at, format!("expected an assignment (`name = ...`), found {}", Self::describe(&t)))),
            }
        }
    }

    fn expr(&mut self, min: u8) -> Result<Raw, Error> {
        let mut left = self.prefix()?;
        loop {
            let (op, power) = match self.peek() {
                Token::Op("||") => (Binary::Or, 1),
                Token::Op("&&") => (Binary::And, 3),
                Token::Op("==") => (Binary::Eq, 5),
                Token::Op("!=") => (Binary::Ne, 5),
                Token::Op("<") => (Binary::Lt, 7),
                Token::Op("<=") => (Binary::Le, 7),
                Token::Op(">") => (Binary::Gt, 7),
                Token::Op(">=") => (Binary::Ge, 7),
                Token::Op("+") => (Binary::Add, 9),
                Token::Op("-") => (Binary::Sub, 9),
                Token::Op("*") => (Binary::Mul, 11),
                Token::Op("/") => (Binary::Div, 11),
                Token::Op("%") => (Binary::Mod, 11),
                _ => return Ok(left),
            };
            if power < min {
                return Ok(left);
            }
            self.next();
            let right = self.expr(power + 1)?;
            left = Raw::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn prefix(&mut self) -> Result<Raw, Error> {
        match self.next() {
            (Token::Num(n), _) => Ok(Raw::Num(n)),
            (Token::Op("-"), _) => Ok(Raw::Unary(Unary::Neg, Box::new(self.expr(13)?))),
            (Token::Op("+"), _) => self.expr(13),
            (Token::Op("!"), _) => Ok(Raw::Unary(Unary::Not, Box::new(self.expr(13)?))),
            (Token::Op("("), _) => {
                let e = self.expr(0)?;
                self.expect(")")?;
                Ok(e)
            }
            (Token::Name(name), at) if *self.peek() == Token::Op("(") => {
                let Some(&(_, func, arity)) = FUNCS.iter().find(|f| f.0 == name) else {
                    return Err(error(self.src, at, format!("no function `{name}`{}", suggest(&name, FUNCS.iter().map(|f| f.0)))));
                };
                self.next();
                let mut args = Vec::new();
                if *self.peek() != Token::Op(")") {
                    loop {
                        args.push(self.expr(0)?);
                        if *self.peek() == Token::Op(",") {
                            self.next();
                        } else {
                            break;
                        }
                    }
                }
                self.expect(")")?;
                if args.len() != arity {
                    let s = if arity == 1 { "" } else { "s" };
                    return Err(error(self.src, at, format!("`{name}` takes {arity} argument{s}, not {}", args.len())));
                }
                Ok(Raw::Call(func, args))
            }
            (Token::Name(name), at) => Ok(Raw::Name(name, at)),
            (t, at) => Err(error(self.src, at, format!("expected a value, found {}", Self::describe(&t)))),
        }
    }
}

/// ", did you mean `x`?" for the closest of `names` to `name`, if one is close.
fn suggest<'a>(name: &str, names: impl Iterator<Item = &'a str>) -> String {
    let distance = |a: &str, b: &str| {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, &cb) in b.iter().enumerate() {
                let next = (prev + usize::from(ca != cb)).min(row[j] + 1).min(row[j + 1] + 1);
                prev = row[j + 1];
                row[j + 1] = next;
            }
        }
        row[b.len()]
    };
    names
        .map(|n| (distance(name, n), n))
        .filter(|(d, n)| *d <= 2.max(n.len() / 4))
        .min()
        .map_or(String::new(), |(_, n)| format!(", did you mean `{n}`?"))
}

/// Every name a program can use at `stage`, for suggestions.
fn known(stage: Stage) -> impl Iterator<Item = String> {
    let q = (1..=QS).map(|i| format!("q{i}"));
    let base = INPUTS.iter().chain(MOTION).map(|s| s.to_string()).chain(q);
    let staged: Vec<String> = match stage {
        Stage::Frame => FRAME_OUTPUTS.iter().map(|s| s.to_string()).collect(),
        Stage::Pixel => PIXEL_INPUTS.iter().map(|s| s.to_string()).collect(),
    };
    base.chain(staged)
}

/// Parse and resolve a program for `stage`.
pub fn compile(src: &str, stage: Stage) -> Result<Program, Error> {
    let mut parser = Parser { src, tokens: lex(src)?, at: 0 };
    let raw = parser.statements()?;
    let mut program = Program { stage, stmts: Vec::new(), locals: Vec::new(), params: Vec::new() };
    // Names assigned anywhere are the program's own (or outputs), whatever the order.
    let assigned: Vec<String> = raw.iter().map(|(n, _, _)| n.clone()).collect();
    let mut resolve = |program: &mut Program, name: &str, at: usize, writing: bool| -> Result<Ref, Error> {
        let fail = |message: String| Err(error(src, at, message));
        if let Some(i) = INPUTS.iter().position(|n| *n == name) {
            return if writing { fail(format!("`{name}` is an input: it can't be set")) } else { Ok(Ref::Shared(i)) };
        }
        if let Some(i) = PIXEL_INPUTS.iter().position(|n| *n == name) {
            return match (stage, writing) {
                (Stage::Frame, _) => fail(format!("`{name}` only exists per pixel")),
                (Stage::Pixel, true) => fail(format!("`{name}` is an input: it can't be set")),
                (Stage::Pixel, false) => Ok(Ref::Pixel(i)),
            };
        }
        if let Some(slot) = shared_slot(name) {
            let is_q = slot >= INPUTS.len() + MOTION.len();
            return if is_q && writing && stage == Stage::Pixel {
                fail(format!("`{name}` is set per frame and only read per pixel"))
            } else {
                Ok(Ref::Shared(slot))
            };
        }
        if let Some(i) = FRAME_OUTPUTS.iter().position(|n| *n == name) {
            return match stage {
                Stage::Frame => Ok(Ref::Output(i)),
                Stage::Pixel => fail(format!("`{name}` can only be used per frame")),
            };
        }
        if let Some(key) = name.strip_prefix(PARAM_PREFIX) {
            if stage == Stage::Pixel {
                return fail(format!("solver parameters (`{name}`) can only be set per frame"));
            }
            if key.is_empty() {
                return fail(format!("`{PARAM_PREFIX}` needs a parameter's key after it"));
            }
            let i = program.params.iter().position(|p| p == key).unwrap_or_else(|| {
                program.params.push(key.to_string());
                program.params.len() - 1
            });
            return Ok(Ref::Param(i));
        }
        if !writing && !assigned.iter().any(|a| a == name) {
            return fail(format!("unknown name `{name}`{}", suggest(name, known(stage).collect::<Vec<_>>().iter().map(String::as_str))));
        }
        let i = program.locals.iter().position(|l| l == name).unwrap_or_else(|| {
            program.locals.push(name.to_string());
            program.locals.len() - 1
        });
        Ok(Ref::Local(i))
    };
    /// Resolves a name (at an offset) for reading or (`true`) writing.
    type Resolve<'a> = dyn FnMut(&mut Program, &str, usize, bool) -> Result<Ref, Error> + 'a;
    fn lower(raw: Raw, program: &mut Program, resolve: &mut Resolve<'_>) -> Result<Expr, Error> {
        Ok(match raw {
            Raw::Num(n) => Expr::Num(n),
            Raw::Name(name, at) => Expr::Ref(resolve(program, &name, at, false)?),
            Raw::Unary(op, e) => Expr::Unary(op, Box::new(lower(*e, program, resolve)?)),
            Raw::Binary(op, a, b) => Expr::Binary(op, Box::new(lower(*a, program, resolve)?), Box::new(lower(*b, program, resolve)?)),
            Raw::Call(f, args) => Expr::Call(f, args.into_iter().map(|a| lower(a, program, resolve)).collect::<Result<_, _>>()?),
        })
    }
    for (name, at, expr) in raw {
        let target = resolve(&mut program, &name, at, true)?;
        let expr = lower(expr, &mut program, &mut resolve)?;
        if stage == Stage::Pixel && uses_rand(&expr) {
            return Err(error(src, at, "`rand()` can only be used per frame".into()));
        }
        program.stmts.push(Stmt { target, expr });
    }
    Ok(program)
}

fn uses_rand(e: &Expr) -> bool {
    match e {
        Expr::Call(Func::Rand, _) => true,
        Expr::Call(_, args) => args.iter().any(uses_rand),
        Expr::Unary(_, a) => uses_rand(a),
        Expr::Binary(_, a, b) => uses_rand(a) || uses_rand(b),
        Expr::Num(_) | Expr::Ref(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(src: &str) -> Program {
        compile(src, Stage::Frame).unwrap_or_else(|e| panic!("{src:?}: {e}"))
    }

    fn fails(src: &str, stage: Stage) -> Error {
        compile(src, stage).err().unwrap_or_else(|| panic!("{src:?} compiled"))
    }

    #[test]
    fn precedence_and_associativity() {
        let p = frame("a = 1 + 2 * 3 - 4 / 2 - 1");
        // ((1 + (2 * 3)) - (4 / 2)) - 1
        let Expr::Binary(Binary::Sub, left, one) = &p.stmts[0].expr else { panic!("{:?}", p.stmts[0].expr) };
        assert_eq!(**one, Expr::Num(1.0));
        assert!(matches!(**left, Expr::Binary(Binary::Sub, _, _)));
        let p = frame("a = -2 * 3 < 1 && !0 || 0");
        assert!(matches!(p.stmts[0].expr, Expr::Binary(Binary::Or, _, _)));
    }

    #[test]
    fn statements_split_on_lines_and_semicolons() {
        let p = frame("zoom = 1.1; rot = 0.2\n// a comment\nq1 = sin(time)\n\nmine = mine + 1;");
        assert_eq!(p.stmts.len(), 4);
        assert_eq!(p.stmts[0].target, Ref::Shared(slot("zoom")));
        assert_eq!(p.stmts[2].target, Ref::Shared(slot("q1")));
        assert_eq!(p.locals, ["mine"]);
        // An expression may run over a line break.
        assert_eq!(frame("zoom = 1 +\n 0.1").stmts.len(), 1);
    }

    #[test]
    fn names_are_case_insensitive_and_resolve_by_kind() {
        let p = frame("ZOOM = Bass_Att; trail = 0.5; param_buoyancy = 0.2; param_buoyancy = 0.3");
        assert_eq!(p.stmts[0].target, Ref::Shared(slot("zoom")));
        assert_eq!(p.stmts[0].expr, Expr::Ref(Ref::Shared(slot("bass_att"))));
        assert_eq!(p.stmts[1].target, Ref::Output(0));
        assert_eq!(p.params, ["buoyancy"]);
        let px = compile("rot = rot * rad + q2; t = x; zoom = t", Stage::Pixel).unwrap();
        assert_eq!(px.locals, ["t"]);
    }

    #[test]
    fn errors_say_where_and_what() {
        let e = fails("zoom = 1\nrot = bas * 2", Stage::Frame);
        assert_eq!((e.line, e.column), (2, 7));
        assert!(e.message.contains("unknown name `bas`") && e.message.contains("did you mean `bass`"), "{}", e.message);
        assert!(fails("bass = 1", Stage::Frame).message.contains("input"));
        assert!(fails("zoom = 2 ^ 3", Stage::Frame).message.contains("pow"));
        assert!(fails("zoom = sin(1, 2)", Stage::Frame).message.contains("takes 1 argument"));
        assert!(fails("zoom = sine(1)", Stage::Frame).message.contains("did you mean `sin`"));
        assert!(fails("zoom = (1 + 2", Stage::Frame).message.contains("expected `)`"));
        assert!(fails("zoom 1", Stage::Frame).message.contains("expected `=`"));
        assert!(fails("zoom = x", Stage::Frame).message.contains("per pixel"));
        assert!(fails("trail = 0.5", Stage::Pixel).message.contains("per frame"));
        assert!(fails("q1 = 1", Stage::Pixel).message.contains("only read per pixel"));
        assert!(fails("zoom = rand()", Stage::Pixel).message.contains("per frame"));
        assert!(fails("param_ = 1", Stage::Frame).message.contains("key"));
    }

    #[test]
    fn numbers_lex_in_every_form() {
        for (src, want) in [("1", 1.0), ("2.5", 2.5), (".5", 0.5), ("1e-3", 1e-3), ("3E2", 300.0)] {
            assert_eq!(frame(&format!("zoom = {src}")).stmts[0].expr, Expr::Num(want), "{src}");
        }
    }
}
