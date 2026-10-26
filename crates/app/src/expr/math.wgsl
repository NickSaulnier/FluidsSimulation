// The equation language's maths on the GPU: each function here is the twin of one in
// expr/eval.rs (`math`), so per-pixel code computes what the interpreter would.

fn x_bool(b: bool) -> f32 {
    return select(0.0, 1.0, b);
}

fn x_div(a: f32, b: f32) -> f32 {
    return select(a / b, 0.0, b == 0.0);
}

fn x_mod(a: f32, b: f32) -> f32 {
    return select(a - b * floor(a / b), 0.0, b == 0.0);
}

fn x_sqrt(a: f32) -> f32 {
    return sqrt(abs(a));
}

// Of the size of a, keeping its sign for whole odd powers.
fn x_pow(a: f32, b: f32) -> f32 {
    let p = select(pow(abs(a), b), 1.0, b == 0.0);
    let odd = fract(b) == 0.0 && abs(b) % 2.0 == 1.0;
    return select(p, -p, a < 0.0 && odd);
}

fn x_log(a: f32) -> f32 {
    return log(max(a, 1e-30));
}

fn x_log10(a: f32) -> f32 {
    return log(max(a, 1e-30)) * 0.43429448;
}

fn x_asin(a: f32) -> f32 {
    return asin(clamp(a, -1.0, 1.0));
}

fn x_acos(a: f32) -> f32 {
    return acos(clamp(a, -1.0, 1.0));
}

fn x_sigmoid(a: f32, b: f32) -> f32 {
    return 1.0 / (1.0 + exp(-a * b));
}

// Halves round away from zero.
fn x_round(a: f32) -> f32 {
    return sign(a) * floor(abs(a) + 0.5);
}
