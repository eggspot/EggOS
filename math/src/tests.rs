use crate::*;

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn any_finite(&mut self) -> f64 {
        loop {
            let value = f64::from_bits(self.next());
            if value.is_finite() {
                return value;
            }
        }
    }

    fn magnitudes(&mut self, low_exponent: i32, high_exponent: i32, count: usize) -> Vec<f64> {
        (0..count)
            .map(|_| {
                let exponent = self.between(f64::from(low_exponent), f64::from(high_exponent));
                let sign = if self.next() & 1 == 0 { 1.0 } else { -1.0 };
                sign * exponent.exp2() * self.between(1.0, 2.0)
            })
            .collect()
    }

    fn uniform(&mut self, low: f64, high: f64, count: usize) -> Vec<f64> {
        (0..count).map(|_| self.between(low, high)).collect()
    }
}

const SPECIALS: [f64; 12] = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.5,
    f64::INFINITY,
    f64::NEG_INFINITY,
    f64::NAN,
    f64::MIN_POSITIVE,
    5e-324,
    f64::MAX,
    -f64::MAX,
];

fn ordered(x: f64) -> i128 {
    let bits = x.to_bits() as i64;
    i128::from(if bits < 0 { i64::MIN - bits } else { bits })
}

fn ulps(ours: f64, host: f64) -> u128 {
    if ours.is_nan() && host.is_nan() {
        return 0;
    }
    if ours.is_nan() || host.is_nan() {
        return u128::MAX;
    }
    (ordered(ours) - ordered(host)).unsigned_abs()
}

fn single_distance(ours: f32, host: f32) -> u128 {
    if ours.is_nan() || host.is_nan() {
        return u128::MAX;
    }
    let ordered = |x: f32| {
        let bits = x.to_bits() as i32;
        i64::from(if bits < 0 { i32::MIN - bits } else { bits })
    };
    u128::from((ordered(ours) - ordered(host)).unsigned_abs())
}

fn check(name: &str, ours: fn(f64) -> f64, host: fn(f64) -> f64, inputs: &[f64], bound: u128) {
    for &x in inputs.iter().chain(&SPECIALS) {
        let (got, wanted) = (ours(x), host(x));
        let distance = ulps(got, wanted);
        assert!(
            distance <= bound,
            "{name}({x:e}) = {got:e}, the host's {wanted:e}: {distance} ulps"
        );
    }
}

fn check_two(
    name: &str,
    ours: fn(f64, f64) -> f64,
    host: fn(f64, f64) -> f64,
    inputs: &[(f64, f64)],
    bound: u128,
) {
    let specials = SPECIALS.iter().flat_map(|&x| SPECIALS.iter().map(move |&y| (x, y)));
    for (x, y) in inputs.iter().copied().chain(specials) {
        let (got, wanted) = (ours(x, y), host(x, y));
        let distance = ulps(got, wanted);
        assert!(
            distance <= bound,
            "{name}({x:e}, {y:e}) = {got:e}, the host's {wanted:e}: {distance} ulps"
        );
    }
}

fn wide_inputs(seed: u64) -> Vec<f64> {
    let mut random = Random(seed);
    let mut inputs = random.magnitudes(-1074, 1023, 4000);
    inputs.extend((0..2000).map(|_| random.any_finite()));
    inputs
}

#[test]
fn rounding_and_bit_functions_are_exact() {
    let inputs = wide_inputs(1);
    let mut random = Random(2);
    let mut small = random.uniform(-1e6, 1e6, 4000);
    small.extend([0.5, 1.5, 2.5, -0.5, -2.5, 4503599627370495.5, 0.49999999999999994]);
    for set in [&inputs, &small] {
        check("floor", floor, f64::floor, set, 0);
        check("ceil", ceil, f64::ceil, set, 0);
        check("trunc", trunc, f64::trunc, set, 0);
        check("round", round, f64::round, set, 0);
        check("rint", rint, f64::round_ties_even, set, 0);
        check("sqrt", sqrt, f64::sqrt, set, 0);
        check("fabs", fabs, f64::abs, set, 0);
    }
    for &x in inputs.iter().chain(&SPECIALS) {
        let (fraction, exponent) = frexp(x);
        if x.is_finite() && x != 0.0 {
            assert!((0.5..1.0).contains(&fraction.abs()), "frexp({x:e})");
            assert_eq!(ldexp(fraction, exponent), x, "ldexp(frexp({x:e}))");
            assert_eq!(ilogb(x), exponent - 1, "ilogb({x:e})");
        }
        assert_eq!(nextafter(x, f64::INFINITY).to_bits(), x.next_up().to_bits(), "{x:e}");
        assert_eq!(nextafter(x, f64::NEG_INFINITY).to_bits(), x.next_down().to_bits());
        let (fraction, whole) = modf(x);
        if x.is_finite() {
            assert_eq!(whole, x.trunc());
            assert_eq!(fraction, x - x.trunc());
        }
    }
    assert_eq!(ilogb(0.0), ILOGB_ZERO);
    assert_eq!(ilogb(f64::NAN), ILOGB_NAN);
    assert_eq!(logb(0.0), f64::NEG_INFINITY);
    assert_eq!(ldexp(1.0, -1074), 5e-324);
    assert_eq!(ldexp(1.0, 1024), f64::INFINITY);
    assert_eq!(ldexp(0.75, -1074), 5e-324);
    assert_eq!(llround(2.5), 3);
    assert_eq!(llrint(2.5), 2);
    assert_eq!(llrint(f64::NAN), i64::MIN);
}

#[test]
fn fmod_and_remainder_are_exact() {
    let inputs = wide_inputs(3);
    let mut random = Random(4);
    let pairs: Vec<(f64, f64)> = (0..6000)
        .map(|index| (inputs[index], inputs[(index * 7 + 3) % inputs.len()]))
        .chain((0..2000).map(|_| (random.between(-1e4, 1e4), random.between(-50.0, 50.0))))
        .collect();
    check_two("fmod", fmod, |x, y| x % y, &pairs, 0);
    for &(x, y) in &pairs {
        let (rest, quotient) = remquo(x, y);
        if !x.is_finite() || !y.is_finite() || y == 0.0 {
            continue;
        }
        assert_eq!(rest, remainder(x, y));
        assert!(rest.abs() <= y.abs() / 2.0, "remainder({x:e}, {y:e}) = {rest:e}");
        let host_fmod = x % y;
        let difference = (rest - host_fmod).abs();
        assert!(
            difference == 0.0 || difference == y.abs(),
            "remainder({x:e}, {y:e}) = {rest:e}, fmod {host_fmod:e}"
        );
        let ratio = x / y;
        let nearest = ratio.round_ties_even();
        if ratio.abs() < 1e9 && ((nearest - ratio).abs() - 0.5).abs() > 1e-6 {
            assert_eq!(i64::from(quotient), nearest as i64, "remquo({x:e}, {y:e})");
        }
    }
    assert_eq!(remainder(5.0, 2.0), 1.0);
    assert_eq!(remainder(7.0, 2.0), -1.0);
    assert_eq!(remquo(7.0, 2.0), (-1.0, 4));
    assert_eq!(remquo(-7.0, 2.0), (1.0, -4));
    assert_eq!(fmod(-0.0, 3.0).to_bits(), (-0.0f64).to_bits());
}

#[test]
fn trigonometric_functions_are_within_their_bounds() {
    let mut random = Random(5);
    let mut inputs = random.uniform(-10.0, 10.0, 20000);
    inputs.extend(random.magnitudes(-30, 1023, 20000));
    inputs.extend((1..2000).map(|k| f64::from(k) * std::f64::consts::FRAC_PI_2));
    inputs.extend([1e22, 6381956970095103.0 * 2f64.powi(797), 5.319372648326541e255]);
    check("sin", sin, f64::sin, &inputs, 2);
    check("cos", cos, f64::cos, &inputs, 2);
    check("tan", tan, f64::tan, &inputs, 3);
    let mut unit = random.uniform(-1.0, 1.0, 20000);
    unit.extend(random.magnitudes(-60, 0, 5000));
    check("asin", asin, f64::asin, &unit, 3);
    check("acos", acos, f64::acos, &unit, 3);
    let wide = wide_inputs(6);
    check("atan", atan, f64::atan, &wide, 2);
    let mut pairs: Vec<(f64, f64)> = (0..20000)
        .map(|index| (wide[index % wide.len()], random.any_finite()))
        .collect();
    pairs.extend((0..20000).map(|_| (random.between(-5.0, 5.0), random.between(-5.0, 5.0))));
    check_two("atan2", atan2, f64::atan2, &pairs, 3);
}

#[test]
fn exponentials_and_logarithms_are_within_their_bounds() {
    let mut random = Random(7);
    let mut exponents = random.uniform(-746.0, 710.0, 20000);
    exponents.extend(random.uniform(-1.0, 1.0, 10000));
    exponents.extend(random.magnitudes(-80, 0, 5000));
    check("exp", exp, f64::exp, &exponents, 2);
    check("expm1", expm1, f64::exp_m1, &exponents, 2);
    let mut twos = random.uniform(-1076.0, 1025.0, 20000);
    twos.extend((-1074..1024).map(f64::from));
    check("exp2", exp2, f64::exp2, &twos, 2);
    let wide = wide_inputs(8);
    let mut near_one = random.uniform(0.5, 2.0, 20000);
    near_one.extend(random.uniform(-0.999, 3.0, 10000));
    for set in [&wide, &near_one] {
        check("log", log, f64::ln, set, 2);
        check("log2", log2, f64::log2, set, 2);
        check("log10", log10, f64::log10, set, 2);
        check("log1p", log1p, f64::ln_1p, set, 2);
        check("cbrt", cbrt, f64::cbrt, set, 2);
    }
    assert_eq!(log2(1024.0), 10.0);
    assert_eq!(log10(1000.0), 3.0);
    assert_eq!(exp2(10.0), 1024.0);
}

#[test]
fn pow_follows_c_and_is_within_its_bound() {
    let mut random = Random(9);
    let mut pairs: Vec<(f64, f64)> = Vec::new();
    for _ in 0..20000 {
        pairs.push((random.between(0.0, 100.0), random.between(-150.0, 150.0)));
        pairs.push((random.between(0.9, 1.1), random.between(-5000.0, 5000.0)));
        pairs.push((random.any_finite().abs(), random.between(-3.0, 3.0)));
    }
    for _ in 0..5000 {
        pairs.push((random.between(-10.0, 0.0), f64::from(random.next() as u8) - 128.0));
    }
    pairs.extend([(-8.0, 1.0 / 3.0), (2.0, 0.5), (-2.0, 3.0), (10.0, 308.0), (2.0, -1074.0)]);
    check_two("pow", pow, f64::powf, &pairs, 3);
    let mut hypotenuses: Vec<(f64, f64)> = Vec::new();
    for _ in 0..20000 {
        hypotenuses.push((random.any_finite(), random.any_finite()));
        hypotenuses.push((random.between(-10.0, 10.0), random.between(-10.0, 10.0)));
    }
    check_two("hypot", hypot, f64::hypot, &hypotenuses, 2);
    let products: Vec<(f64, f64)> = (0..5000)
        .map(|_| (random.between(-1e3, 1e3), random.between(-1e3, 1e3)))
        .collect();
    for &(x, y) in &products {
        let z = random.between(-1e6, 1e6);
        let distance = ulps(fma(x, y, z), x.mul_add(y, z));
        assert!(distance <= 2, "fma({x}, {y}, {z})");
    }
}

#[test]
fn hyperbolic_functions_are_within_their_bounds() {
    let mut random = Random(10);
    let mut inputs = random.uniform(-30.0, 30.0, 20000);
    inputs.extend(random.uniform(-712.0, 712.0, 5000));
    inputs.extend(random.magnitudes(-60, 3, 10000));
    check("sinh", sinh, f64::sinh, &inputs, 3);
    check("cosh", cosh, f64::cosh, &inputs, 3);
    check("tanh", tanh, f64::tanh, &inputs, 3);
    check("asinh", asinh, f64::asinh, &wide_inputs(11), 3);
    let mut above_one = random.uniform(1.0, 100.0, 20000);
    above_one.extend(random.magnitudes(0, 1023, 5000).iter().map(|x| x.abs() + 1.0));
    check("acosh", acosh, f64::acosh, &above_one, 3);
    let unit = random.uniform(-1.0, 1.0, 20000);
    check("atanh", atanh, f64::atanh, &unit, 3);
}

const ERF_VECTORS: [(u64, u64); 16] = [
    (0x3f50624dd2f1a9fc, 0x3f527cc3804d374c),
    (0x3fb999999999999a, 0x3fbcca5ea24fb334),
    (0x3fd3333333333333, 0x3fd50838881dea0f),
    (0x3fdf5c28f5c28f5c, 0x3fe05f961e0696ac),
    (0x3fe6666666666666, 0x3fe5b08c21171646),
    (0x3ff0000000000000, 0x3feaf767a741088b),
    (0x3ff4cccccccccccd, 0x3fede364a1372d3a),
    (0x3ffb333333333333, 0x3fef7b3620b8747b),
    (0x4000000000000000, 0x3fefd9ae142795e3),
    (0x4004cccccccccccd, 0x3feffe10fffccf10),
    (0x400a666666666666, 0x3feffff9966790c8),
    (0x4010000000000000, 0x3feffffff7b91176),
    (0x4014000000000000, 0x3fefffffffffc9e8),
    (0x4018cccccccccccd, 0x3ff0000000000000),
    (0xbfd999999999999a, 0xbfdb6ac7c4b20a05),
    (0xc004000000000000, 0xbfeffcaa8f4c9bea),
];

const ERFC_VECTORS: [(u64, u64); 20] = [
    (0x3f50624dd2f1a9fc, 0x3feff6c19e3fd964),
    (0x3fb999999999999a, 0x3fec66b42bb60999),
    (0x3fd3333333333333, 0x3fe57be3bbf10af8),
    (0x3fdf5c28f5c28f5c, 0x3fdf40d3c3f2d2a7),
    (0x3fe6666666666666, 0x3fd49ee7bdd1d374),
    (0x3ff0000000000000, 0x3fc4226162fbddd5),
    (0x3ff4cccccccccccd, 0x3fb0e4daf646962e),
    (0x3ffb333333333333, 0x3f90993be8f17094),
    (0x4000000000000000, 0x3f7328f5ec350e67),
    (0x4004cccccccccccd, 0x3f2ef000330f060c),
    (0x400a666666666666, 0x3ec9a661bcdec4af),
    (0x4010000000000000, 0x3e508ddd13bd35e7),
    (0x4014000000000000, 0x3d7b0c1a759f7739),
    (0x4018cccccccccccd, 0x3c40c181079f1f35),
    (0x4020000000000000, 0x39ec74fc41217dfb),
    (0x4028000000000000, 0x32ac90f21d2d475f),
    (0x4034000000000000, 0x1b8b54f244df93df),
    (0x403a000000000000, 0x02a284bfe1cdea24),
    (0xbff0000000000000, 0x3ffd7bb3d3a08445),
    (0xbfc999999999999a, 0x3ff3903097164be3),
];

const TGAMMA_VECTORS: [(u64, u64); 17] = [
    (0x3f50624dd2f1a9fc, 0x408f3b63e2d41242),
    (0x3fb999999999999a, 0x402306ea7b280d87),
    (0x3fe0000000000000, 0x3ffc5bf891b4ef6b),
    (0x3ff8000000000000, 0x3fec5bf891b4ef6b),
    (0x4004000000000000, 0x3ff544fa6d47b390),
    (0x400d99999999999a, 0x4010aebf5759a454),
    (0x401d333333333333, 0x4093ddb1cd05efe4),
    (0x4025000000000000, 0x41314ade639225ca),
    (0x403419999999999a, 0x43822e67b318c563),
    (0x404bc00000000000, 0x4eeeef99fe6b6816),
    (0x4059100000000000, 0x606b7d035f47bc48),
    (0x4065500000000000, 0x7f69589f849167a8),
    (0xbfe0000000000000, 0xc00c5bf891b4ef6b),
    (0xbff8000000000000, 0x4002e7fb0bcdf4f2),
    (0xc002666666666666, 0xbff7275a1542102a),
    (0xc025666666666666, 0xbe8b103f726666da),
    (0x3bc79ca10c924223, 0x4415af1d78b58c40),
];

const LGAMMA_VECTORS: [(u64, u64); 16] = [
    (0x3f50624dd2f1a9fc, 0x401ba0f3807161ac),
    (0x3fb999999999999a, 0x4002058e35f3deee),
    (0x3fe0000000000000, 0x3fe250d048e7a1bd),
    (0x3feccccccccccccd, 0x3fb0fe0882e465b1),
    (0x3ff199999999999a, 0xbfa988e1705d89ea),
    (0x3ff762b6ae7d566d, 0xbfbf19b9ba9dd633),
    (0x3ffe666666666666, 0xbfa3f5bf3c4bb75a),
    (0x4000cccccccccccd, 0x3fa7439d821b03af),
    (0x4004000000000000, 0x3fd2383e809a67e8),
    (0x400d99999999999a, 0x3ff6d9625e359b87),
    (0x4025000000000000, 0x402be199a0f64394),
    (0x4059100000000000, 0x4076848d8e681f58),
    (0x408f440000000000, 0x40b714ac96c9d5d3),
    (0x412e848000000000, 0x416871921236750d),
    (0xbfe0000000000000, 0x3ff43f89a3f0edd6),
    (0xc00d99999999999a, 0xbff6136a268d07a3),
];

fn check_vectors(name: &str, function: fn(f64) -> f64, vectors: &[(u64, u64)], bound: u128) {
    for &(input, expected) in vectors {
        let (x, wanted) = (f64::from_bits(input), f64::from_bits(expected));
        let got = function(x);
        let distance = ulps(got, wanted);
        assert!(distance <= bound, "{name}({x:e}) = {got:e}, not {wanted:e}: {distance} ulps");
    }
}

#[test]
fn the_error_and_gamma_functions_match_values_computed_to_80_digits() {
    check_vectors("erf", erf, &ERF_VECTORS, 4);
    check_vectors("erfc", erfc, &ERFC_VECTORS, 4);
    check_vectors("tgamma", tgamma, &TGAMMA_VECTORS, 4);
    check_vectors("lgamma", lgamma, &LGAMMA_VECTORS, 4);
    let mut factorial = 1.0;
    for n in 1..=20 {
        assert_eq!(tgamma(f64::from(n)), factorial, "tgamma({n})");
        factorial *= f64::from(n);
    }
    assert_eq!(lgamma(1.0), 0.0);
    assert_eq!(lgamma(2.0), 0.0);
    assert_eq!(lgamma_r(-0.5).1, -1);
    assert_eq!(lgamma_r(-1.5).1, 1);
    assert!(tgamma(-3.0).is_nan());
    assert_eq!(tgamma(-0.0), f64::NEG_INFINITY);
    assert_eq!(lgamma(-2.0), f64::INFINITY);
    assert_eq!(erf(f64::INFINITY), 1.0);
    assert_eq!(erfc(f64::NEG_INFINITY), 2.0);
    assert_eq!(erfc(30.0), 0.0);
}

#[test]
fn c_s_special_cases_hold() {
    assert_eq!(pow(f64::NAN, 0.0), 1.0);
    assert_eq!(pow(1.0, f64::NAN), 1.0);
    assert_eq!(pow(-1.0, f64::INFINITY), 1.0);
    assert_eq!(pow(-0.0, -3.0), f64::NEG_INFINITY);
    assert_eq!(pow(-0.0, 3.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(pow(f64::NEG_INFINITY, 3.0), f64::NEG_INFINITY);
    assert!(pow(-2.0, 0.5).is_nan());
    assert_eq!(atan2(0.0, -0.0), std::f64::consts::PI);
    assert_eq!(atan2(-0.0, 1.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(atan2(f64::INFINITY, f64::NEG_INFINITY), 3.0 * std::f64::consts::FRAC_PI_4);
    assert_eq!(hypot(f64::NAN, f64::INFINITY), f64::INFINITY);
    assert!(sin(f64::INFINITY).is_nan());
    assert_eq!(sin(-0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(cbrt(-27.0), -3.0);
    assert_eq!(atanh(1.0), f64::INFINITY);
    assert!(acosh(0.5).is_nan());
    assert!(log(-1.0).is_nan());
    assert_eq!(log1p(-1.0), f64::NEG_INFINITY);
    assert_eq!(fmin(f64::NAN, 2.0), 2.0);
    assert_eq!(fmax(1.0, f64::NAN), 1.0);
    assert_eq!(fdim(1.0, 3.0), 0.0);
    assert_eq!(copysign(3.0, -0.0), -3.0);
    assert_eq!(sinpi(1.0), 0.0);
    assert_eq!(sinpi(0.5), 1.0);
    assert_eq!(sinpi(-1.5), 1.0);
    assert_eq!(cospi(0.5), 0.0);
    assert_eq!(cospi(1.0), -1.0);
    assert_eq!(cospi(-2.0), 1.0);
    assert!(sinpi(f64::INFINITY).is_nan());
    let mut random = Random(13);
    for x in random.uniform(-4.0, 4.0, 5000) {
        let wanted = (std::f64::consts::PI * x).sin();
        assert!((sinpi(x) - wanted).abs() <= 1e-15, "sinpi({x})");
        let wanted = (std::f64::consts::PI * x).cos();
        assert!((cospi(x) - wanted).abs() <= 1e-15, "cospi({x})");
    }
}

#[test]
fn the_float_twins_are_within_one_unit() {
    type Pair = (fn(f32) -> f32, fn(f32) -> f32, &'static str);
    let unary: [Pair; 20] = [
        (sinf, f32::sin, "sinf"),
        (cosf, f32::cos, "cosf"),
        (tanf, f32::tan, "tanf"),
        (asinf, f32::asin, "asinf"),
        (acosf, f32::acos, "acosf"),
        (atanf, f32::atan, "atanf"),
        (sinhf, f32::sinh, "sinhf"),
        (coshf, f32::cosh, "coshf"),
        (tanhf, f32::tanh, "tanhf"),
        (expf, f32::exp, "expf"),
        (exp2f, f32::exp2, "exp2f"),
        (expm1f, f32::exp_m1, "expm1f"),
        (logf, f32::ln, "logf"),
        (log2f, f32::log2, "log2f"),
        (log10f, f32::log10, "log10f"),
        (log1pf, f32::ln_1p, "log1pf"),
        (sqrtf, f32::sqrt, "sqrtf"),
        (cbrtf, f32::cbrt, "cbrtf"),
        (floorf, f32::floor, "floorf"),
        (roundf, f32::round, "roundf"),
    ];
    let mut random = Random(12);
    let inputs: Vec<f32> = (0..20000)
        .map(|_| random.between(-20.0, 20.0) as f32)
        .chain((0..20000).map(|_| f32::from_bits(random.next() as u32)))
        .collect();
    for (ours, host, name) in unary {
        for &x in &inputs {
            let (got, wanted) = (ours(x), host(x));
            let distance = if got.is_nan() && wanted.is_nan() {
                0
            } else {
                single_distance(got, wanted)
            };
            assert!(distance <= 1, "{name}({x:e}) = {got:e}, the host's {wanted:e}");
        }
    }
    assert_eq!(powf(2.0, 10.0), 1024.0);
    assert_eq!(fmodf(7.5, 2.0), 1.5);
    assert_eq!(nextafterf(1.0, 2.0), 1.0f32.next_up());
    assert_eq!(nextafterf(0.0, -1.0), -f32::from_bits(1));
    assert_eq!(ldexpf(1.0, -149), f32::from_bits(1));
    assert_eq!(frexpf(8.0), (0.5, 4));
}
