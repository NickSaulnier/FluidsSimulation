//! The WGSL generator: a per-pixel program as shader statements, for the feedback
//! shader's warp (see `render::feedback`).
//!
//! The generated code expects, in scope: `v(i)` reading shared slot `i` (inputs and
//! q's, from the frame's uniforms), the motion as variables `m_<name>` (set to the
//! frame's motion before it runs, read after), the pixel inputs as `p_<name>`, and the
//! helpers in `MATH`.

use super::{Binary, Expr, Func, INPUTS, MOTION, PIXEL_INPUTS, Program, Ref, Stage, Unary};
use std::fmt::Write;

/// The maths helpers the generated code calls.
pub const MATH: &str = include_str!("math.wgsl");

/// The variable a motion name lives in.
pub fn motion_var(name: &str) -> String {
    format!("m_{name}")
}

/// The variable a pixel input lives in.
pub fn pixel_var(name: &str) -> String {
    format!("p_{name}")
}

/// A per-pixel program as WGSL statements.
pub fn pixel(p: &Program) -> String {
    assert_eq!(p.stage, Stage::Pixel, "only per-pixel programs become shader code");
    let mut out = String::new();
    for local in &p.locals {
        let _ = writeln!(out, "    var u_{local} = 0.0;");
    }
    for s in &p.stmts {
        let _ = writeln!(out, "    {} = {};", place(p, s.target), expr(p, &s.expr));
    }
    out
}

fn place(p: &Program, r: Ref) -> String {
    match r {
        Ref::Shared(i) if (INPUTS.len()..INPUTS.len() + MOTION.len()).contains(&i) => motion_var(MOTION[i - INPUTS.len()]),
        Ref::Shared(i) => format!("v({i}u)"),
        Ref::Pixel(i) => pixel_var(PIXEL_INPUTS[i]),
        Ref::Local(i) => format!("u_{}", p.locals[i]),
        Ref::Output(_) | Ref::Param(_) => unreachable!("per-frame only, refused by the compiler"),
    }
}

fn number(n: f32) -> String {
    // Debug formatting always has a point or an exponent, as WGSL's floats need.
    let s = format!("{n:?}");
    if s.contains(['.', 'e']) { s } else { format!("{s}.0") }
}

fn expr(p: &Program, e: &Expr) -> String {
    match e {
        Expr::Num(n) => number(*n),
        Expr::Ref(r) => place(p, *r),
        Expr::Unary(Unary::Neg, a) => format!("(-{})", expr(p, a)),
        Expr::Unary(Unary::Not, a) => format!("x_bool({} == 0.0)", expr(p, a)),
        Expr::Binary(op, a, b) => {
            let (a, b) = (expr(p, a), expr(p, b));
            match op {
                Binary::Add => format!("({a} + {b})"),
                Binary::Sub => format!("({a} - {b})"),
                Binary::Mul => format!("({a} * {b})"),
                Binary::Div => format!("x_div({a}, {b})"),
                Binary::Mod => format!("x_mod({a}, {b})"),
                Binary::Lt => format!("x_bool({a} < {b})"),
                Binary::Le => format!("x_bool({a} <= {b})"),
                Binary::Gt => format!("x_bool({a} > {b})"),
                Binary::Ge => format!("x_bool({a} >= {b})"),
                Binary::Eq => format!("x_bool({a} == {b})"),
                Binary::Ne => format!("x_bool({a} != {b})"),
                Binary::And => format!("x_bool({a} != 0.0 && {b} != 0.0)"),
                Binary::Or => format!("x_bool({a} != 0.0 || {b} != 0.0)"),
            }
        }
        Expr::Call(f, args) => {
            let a: Vec<String> = args.iter().map(|a| expr(p, a)).collect();
            let name = match f {
                Func::Sin => "sin",
                Func::Cos => "cos",
                Func::Tan => "tan",
                Func::Asin => "x_asin",
                Func::Acos => "x_acos",
                Func::Atan => "atan",
                Func::Atan2 => "atan2",
                Func::Abs => "abs",
                Func::Sqrt => "x_sqrt",
                Func::Pow => "x_pow",
                Func::Exp => "exp",
                Func::Log => "x_log",
                Func::Log10 => "x_log10",
                Func::Min => "min",
                Func::Max => "max",
                Func::Floor => "floor",
                Func::Ceil => "ceil",
                Func::Round => "x_round",
                Func::Fract => "fract",
                Func::Sign => "sign",
                Func::Sigmoid => "x_sigmoid",
                Func::Clamp => return format!("min(max({}, {}), {})", a[0], a[1], a[2]),
                Func::If => return format!("select({}, {}, {} != 0.0)", a[2], a[1], a[0]),
                Func::Above => return format!("x_bool({} > {})", a[0], a[1]),
                Func::Below => return format!("x_bool({} < {})", a[0], a[1]),
                Func::Equal => return format!("x_bool({} == {})", a[0], a[1]),
                Func::Sqr => return format!("({0} * {0})", a[0]),
                Func::Int => "trunc",
                Func::Band => return format!("x_bool({} != 0.0 && {} != 0.0)", a[0], a[1]),
                Func::Bor => return format!("x_bool({} != 0.0 || {} != 0.0)", a[0], a[1]),
                Func::Bnot => return format!("x_bool({} == 0.0)", a[0]),
                Func::Rand | Func::RandInt => unreachable!("per-frame only, refused by the compiler"),
            };
            format!("{name}({})", a.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SHARED, compile, eval::Machine};
    use super::*;
    use utils::rng::SplitMix64;

    /// A compute shader evaluating `src` per pixel at each of `inputs` (the shared block
    /// and the pixel inputs), writing `zoom` and `rot`.
    fn harness(src: &str) -> String {
        let p = compile(src, Stage::Pixel).unwrap();
        let motion: String = MOTION.iter().enumerate().map(|(i, m)| format!("    var {} = v({}u);\n", motion_var(m), INPUTS.len() + i)).collect();
        let inputs: String =
            PIXEL_INPUTS.iter().enumerate().map(|(i, n)| format!("    let {} = data[base + {}u];\n", pixel_var(n), SHARED + i)).collect();
        format!(
            "{MATH}
@group(0) @binding(0) var<storage, read> data: array<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<f32>;
var<private> base: u32;
fn v(i: u32) -> f32 {{ return data[base + i]; }}
@compute @workgroup_size(1)
fn main(@builtin(global_invocation_id) id: vec3u) {{
    base = id.x * {stride}u;
{inputs}{motion}{body}    out[id.x * 2u] = m_zoom;
    out[id.x * 2u + 1u] = m_rot;
}}",
            stride = SHARED + PIXEL_INPUTS.len(),
            body = pixel(&p),
        )
    }

    fn validate(source: &str) -> Result<(), String> {
        let module = wgpu::naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .map(|_| ())
            .map_err(|e| e.emit_to_string(source))
    }

    const PROGRAMS: &[&str] = &[
        "zoom = zoom + 0.1 * sin(ang * 3 + time) * rad; rot = rot * (1 - rad)",
        "t = x * 2 - 1; zoom = if(t > 0 && y < 0.5, pow(t, 3), -sqrt(t)); rot = x_mod_test; x_mod_test = 0",
        "zoom = log(rad) + log10(x + 1) + exp(-y) + atan2(y - 0.5, x - 0.5); rot = clamp(q1 * 10, -1, 1) + sigmoid(rad, 4)",
        "zoom = 7 % (x * 3) + 1 / (y - y) + round(x * 5 - 2.5) + fract(-x) + sign(y - 0.5); rot = !(x < 0.5) || equal(y, 0)",
        "zoom = asin(x * 3) + acos(-y) + abs(ang) + floor(rad * 4) / ceil(rad + 0.1) + min(x, y) * max(x, y); rot = pow(-2, 3) + pow(-x, 2)",
    ];

    /// Every generated program is valid WGSL.
    #[test]
    fn generated_code_validates() {
        for src in PROGRAMS {
            if let Err(e) = validate(&harness(src)) {
                panic!("{src}:\n{e}\n{}", harness(src));
            }
        }
    }

    /// On a headless GPU, the generated code computes what the interpreter does, at
    /// sample inputs.
    #[test]
    fn the_gpu_agrees_with_the_interpreter() {
        let gpu = match sim3d::GpuContext::new() {
            Ok(g) => g,
            Err(e) => return eprintln!("skipping: {e:#}"),
        };
        let (device, queue) = (&gpu.device, &gpu.queue);
        let mut rng = SplitMix64::new(5);
        let stride = SHARED + PIXEL_INPUTS.len();
        let samples = 64;
        for src in PROGRAMS {
            let p = compile(src, Stage::Pixel).unwrap();
            let mut machine = Machine::new(&p);
            // Inputs: the shared block (motion 1-ish, the rest -1..1) and the pixel's.
            let mut data = vec![0.0f32; samples * stride];
            let mut want = Vec::new();
            for k in 0..samples {
                let row = &mut data[k * stride..(k + 1) * stride];
                for v in row.iter_mut() {
                    *v = rng.signed();
                }
                for (i, _) in MOTION.iter().enumerate() {
                    row[INPUTS.len() + i] = 1.0 + 0.2 * rng.signed();
                }
                let (x, y) = (rng.next_f32(), rng.next_f32());
                let (dx, dy) = (x - 0.5, y - 0.5);
                row[SHARED..].copy_from_slice(&[x, y, 2.0 * (dx * dx + dy * dy).sqrt(), dy.atan2(dx)]);
                let mut block = vec![0.0; machine.layout.len];
                block[..SHARED].copy_from_slice(&row[..SHARED]);
                block[machine.layout.pixel..].copy_from_slice(&row[SHARED..]);
                machine.run(&mut block, &mut rng);
                want.push((block[crate::expr::slot("zoom")], block[crate::expr::slot("rot")]));
            }
            let got = run_compute(device, queue, &harness(src), &data, samples);
            for (k, (&(wz, wr), g)) in want.iter().zip(got.chunks(2)).enumerate() {
                let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * (1.0 + a.abs());
                assert!(close(wz, g[0]) && close(wr, g[1]), "{src}: sample {k}: CPU ({wz}, {wr}), GPU ({}, {})", g[0], g[1]);
            }
        }
    }

    fn run_compute(device: &wgpu::Device, queue: &wgpu::Queue, source: &str, data: &[f32], samples: usize) -> Vec<f32> {
        use wgpu::util::DeviceExt;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(source.into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(data), usage: wgpu::BufferUsages::STORAGE });
        let size = (samples * 2 * 4) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(samples as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, size);
        queue.submit([encoder.finish()]);
        read.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let view = read.slice(..).get_mapped_range().expect("mapped");
        bytemuck::cast_slice(&view).to_vec()
    }
}
