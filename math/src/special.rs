//! The error function and the gamma function. `erf` sums its Taylor series below 1/2; above,
//! `erfc` is the Taylor series of `erfc` about the nearest quarter (its derivatives are
//! Hermite polynomials times `e^(-x^2)`, its values at the quarters from 1/2 to 6 a table of
//! double-doubles) and Laplace's continued fraction beyond 6. `lgamma` and `tgamma` shift the
//! argument above 12 by the recurrence, take Stirling's series there in double-doubles, and
//! use the series of `lgamma` about 1 and 2 (whose coefficients are values of Riemann's zeta
//! function) near its zeros; negative arguments go through the reflection formula.

use crate::double_double::{add, divide, multiply, two_product, two_sum};
use crate::exponential::{exp_parts, horner, inverse_factorials, log, log_double_double};
use crate::manipulation::{copysign, fabs, is_integer, power_of_two};
use crate::trigonometric::sin_pi;

const TWO_OVER_SQRT_PI: (f64, f64) = (
    f64::from_bits(0x3ff2_0dd7_5042_9b6d),
    f64::from_bits(0x3c71_ae3a_914f_ed80),
);
const HALF_LN_2PI: (f64, f64) = (
    f64::from_bits(0x3fed_67f1_c864_beb5),
    f64::from_bits(0xbc86_5b5a_1b7f_f5df),
);
const LN_PI: (f64, f64) = (
    f64::from_bits(0x3ff2_50d0_48e7_a1bd),
    f64::from_bits(0x3c67_abf2_ad8d_5088),
);
const ERF_SERIES_LIMIT: f64 = 0.5;
const ERFC_TABLE_LIMIT: f64 = 6.0;
const ERFC_UNDERFLOW: f64 = 27.3;
const ERFC_TAYLOR_TERMS: usize = 30;
const ERFC_FRACTION_TERMS: u32 = 60;
const TWO_POW_MINUS_28: f64 = power_of_two(-28);
const TWO_POW_MINUS_56: f64 = power_of_two(-56);
const TWO_POW_MINUS_60: f64 = power_of_two(-60);
const TWO_POW_60: f64 = power_of_two(60);
const STIRLING_START: f64 = 12.0;
const TGAMMA_OVERFLOW: f64 = 171.7;
const TGAMMA_UNDERFLOW: f64 = -184.0;

/// `erfc(k/4)` for `k` from 2 to 24, as double-doubles.
const ERFC_QUARTERS: [(f64, f64); 23] = [
    (
        f64::from_bits(0x3fde_b021_47ce_245c),
        f64::from_bits(0xbc75_e809_f1a3_1a28),
    ),
    (
        f64::from_bits(0x3fd2_7c6d_14c5_e341),
        f64::from_bits(0x3c63_af34_34d0_eeab),
    ),
    (
        f64::from_bits(0x3fc4_2261_62fb_ddd5),
        f64::from_bits(0xbc4b_4044_3f6e_c34a),
    ),
    (
        f64::from_bits(0x3fb3_bcd1_33aa_0ffc),
        f64::from_bits(0xbc18_9da8_2345_938b),
    ),
    (
        f64::from_bits(0x3fa1_5aaa_8ec8_5205),
        f64::from_bits(0xbc2e_86ee_834d_a4ce),
    ),
    (
        f64::from_bits(0x3f8b_4be2_01ca_a4b4),
        f64::from_bits(0xbc26_abde_927f_9cdd),
    ),
    (
        f64::from_bits(0x3f73_28f5_ec35_0e67),
        f64::from_bits(0xbc1c_a006_412e_68d0),
    ),
    (
        f64::from_bits(0x3f57_f713_f9cc_9784),
        f64::from_bits(0xbbf4_2071_4320_2515),
    ),
    (
        f64::from_bits(0x3f3a_ab85_9b20_ac9e),
        f64::from_bits(0x3bd8_8f4f_f748_376b),
    ),
    (
        f64::from_bits(0x3f1a_609f_7584_d32b),
        f64::from_bits(0x3bbd_92f3_f7ab_9ef5),
    ),
    (
        f64::from_bits(0x3ef7_29df_6503_422a),
        f64::from_bits(0x3b67_84ca_4c42_9a15),
    ),
    (
        f64::from_bits(0x3ed2_0c13_0355_0f0e),
        f64::from_bits(0xbb62_0ee8_0d2c_8d09),
    ),
    (
        f64::from_bits(0x3ea8_ef2a_9a18_d857),
        f64::from_bits(0xbb42_d76d_c03e_80a5),
    ),
    (
        f64::from_bits(0x3e7e_8747_0e4f_4246),
        f64::from_bits(0xbadc_afa3_aa5b_4314),
    ),
    (
        f64::from_bits(0x3e50_8ddd_13bd_35e7),
        f64::from_bits(0xbaf6_15db_4031_9381),
    ),
    (
        f64::from_bits(0x3e1f_cae9_3fb7_323c),
        f64::from_bits(0x3ab3_5a35_9669_2c9c),
    ),
    (
        f64::from_bits(0x3deb_05cf_e2e9_9435),
        f64::from_bits(0xba8a_071e_d805_52c6),
    ),
    (
        f64::from_bits(0x3db4_5314_1082_302a),
        f64::from_bits(0xba51_a101_007d_8742),
    ),
    (
        f64::from_bits(0x3d7b_0c1a_759f_7739),
        f64::from_bits(0xba1b_2856_8855_c7a1),
    ),
    (
        f64::from_bits(0x3d3f_d5f0_8ad2_b29a),
        f64::from_bits(0x39ce_5b1d_0c5b_f934),
    ),
    (
        f64::from_bits(0x3d00_9182_b326_b229),
        f64::from_bits(0xb992_2e85_b13a_8e1d),
    ),
    (
        f64::from_bits(0x3cbe_7eea_02e4_ed88),
        f64::from_bits(0xb958_24cc_1e23_39a2),
    ),
    (
        f64::from_bits(0x3c78_cf81_557d_20b6),
        f64::from_bits(0x38fa_7fff_0cc7_32c0),
    ),
];

/// The coefficients of `lgamma(1 + e)` in powers of `e` from the first: `-γ`, then
/// `(-1)^k ζ(k) / k`.
const LGAMMA_ABOUT_ONE: [f64; 29] = [
    f64::from_bits(0xbfe2_788c_fc6f_b619),
    f64::from_bits(0x3fea_51a6_6253_07d3),
    f64::from_bits(0xbfd9_a4d5_5bea_b2d7),
    f64::from_bits(0x3fd1_5132_2ac7_d848),
    f64::from_bits(0xbfca_8b9c_17aa_6149),
    f64::from_bits(0x3fc5_b40c_b100_c306),
    f64::from_bits(0xbfc2_703a_1dce_a3ae),
    f64::from_bits(0x3fc0_10b3_6af8_6397),
    f64::from_bits(0xbfbc_8067_06d5_7db4),
    f64::from_bits(0x3fb9_a01e_385d_5f8f),
    f64::from_bits(0xbfb7_48c3_3114_c6d6),
    f64::from_bits(0x3fb5_56ad_6324_3bc4),
    f64::from_bits(0xbfb3_b1d9_71fc_5985),
    f64::from_bits(0x3fb2_496d_f832_0c5f),
    f64::from_bits(0xbfb1_1133_476e_7fe0),
    f64::from_bits(0x3fb0_0010_064c_deb2),
    f64::from_bits(0xbfae_1e2d_311e_8abd),
    f64::from_bits(0x3fac_71ce_3a20_b419),
    f64::from_bits(0xbfaa_f28a_1b56_88a0),
    f64::from_bits(0x3fa9_999b_3352_d5ba),
    f64::from_bits(0xbfa8_6186_db77_bfbf),
    f64::from_bits(0x3fa7_45d1_d177_8df9),
    f64::from_bits(0xbfa6_42c8_8591_b66d),
    f64::from_bits(0x3fa5_5555_6aaa_fdcd),
    f64::from_bits(0xbfa4_7ae1_51eb_9fb7),
    f64::from_bits(0x3fa3_b13b_189d_925e),
    f64::from_bits(0xbfa2_f684_c000_02bc),
    f64::from_bits(0x3fa2_4924_936d_b7bc),
    f64::from_bits(0xbfa1_a7b9_61a7_b9aa),
];

/// The coefficients of `lgamma(2 + e)` in powers of `e` from the first: `1 - γ`, then
/// `(-1)^k (ζ(k) - 1) / k`.
const LGAMMA_ABOUT_TWO: [f64; 29] = [
    f64::from_bits(0x3fdb_0ee6_0720_93ce),
    f64::from_bits(0x3fd4_a34c_c4a6_0fa6),
    f64::from_bits(0xbfb1_3e00_1a55_7607),
    f64::from_bits(0x3f95_1322_ac7d_8483),
    f64::from_bits(0xbf7e_404f_c218_f5f2),
    f64::from_bits(0x3f67_add6_eadb_6c30),
    f64::from_bits(0xbf53_8ac5_c2bf_8e08),
    f64::from_bits(0x3f40_b36a_f863_96e9),
    f64::from_bits(0xbf2d_3fd4_c76d_2fc8),
    f64::from_bits(0x3f1a_127b_0f17_d65a),
    f64::from_bits(0xbf07_8de5_bd7c_81ef),
    f64::from_bits(0x3ef5_80dc_ee66_eb02),
    f64::from_bits(0xbee3_cbc9_63ce_2243),
    f64::from_bits(0x3ed2_597a_39f3_4aac),
    f64::from_bits(0xbec1_1b2e_b767_9541),
    f64::from_bits(0x3eb0_064c_deb2_2f0f),
    f64::from_bits(0xbe9e_2600_d93c_fd2f),
    f64::from_bits(0x3e8c_76bb_b3f0_7a4d),
    f64::from_bits(0xbe7a_f5a6_cbbf_8a97),
    f64::from_bits(0x3e69_9b93_c207_0b0f),
    f64::from_bits(0xbe58_62c7_34df_3eac),
    f64::from_bits(0x3e47_469d_accf_adcd),
    f64::from_bits(0xbe36_434a_8447_aead),
    f64::from_bits(0x3e25_55a8_77ff_d2c3),
    f64::from_bits(0xbe14_7b16_7925_8d0e),
    f64::from_bits(0x3e03_b15d_2b2f_c10c),
    f64::from_bits(0xbdf2_f69a_9fab_e3e0),
    f64::from_bits(0x3de2_4932_a337_434c),
    f64::from_bits(0xbdd1_a7c2_6ec2_523c),
];

/// Stirling's series after its `1/(12 z)` term, in powers of `1/z^2` from `1/z^3`:
/// `B(2k) / (2k (2k - 1))` for `k` from 2 to 8.
const STIRLING_TAIL: [f64; 7] = [
    -1.0 / 360.0,
    1.0 / 1260.0,
    -1.0 / 1680.0,
    1.0 / 1188.0,
    -691.0 / 360_360.0,
    1.0 / 156.0,
    -3617.0 / 122_400.0,
];

const ERF_SERIES: [f64; 12] = erf_series();

const fn erf_series<const N: usize>() -> [f64; N] {
    let factorials: [f64; N] = inverse_factorials(1);
    let mut table = [0.0; N];
    let mut index = 0;
    let mut sign = -1.0;
    while index < N {
        table[index] = sign * factorials[index] / (2 * index + 3) as f64;
        sign = -sign;
        index += 1;
    }
    table
}

/// `erf`: the error function.
pub fn erf(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let magnitude = fabs(x);
    if magnitude < TWO_POW_MINUS_28 {
        return x * TWO_OVER_SQRT_PI.0;
    }
    if magnitude < ERF_SERIES_LIMIT {
        let square = magnitude * magnitude;
        let sum = magnitude + magnitude * square * horner(square, &ERF_SERIES);
        let (product, error) = two_product(sum, TWO_OVER_SQRT_PI.0);
        return copysign(product + (error + sum * TWO_OVER_SQRT_PI.1), x);
    }
    let value = if magnitude < ERFC_TABLE_LIMIT {
        let (high, low) = erfc_by_table(magnitude);
        (1.0 - high) - low
    } else {
        1.0
    };
    copysign(value, x)
}

/// `erfc`: `1 - erf(x)`, accurate where it is small.
pub fn erfc(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x < ERF_SERIES_LIMIT {
        if x > -ERF_SERIES_LIMIT {
            return 1.0 - erf(x);
        }
        let (high, low) = erfc_positive(-x);
        return (2.0 - high) - low;
    }
    let (high, low) = erfc_positive(x);
    high + low
}

fn erfc_positive(x: f64) -> (f64, f64) {
    if x < ERFC_TABLE_LIMIT {
        erfc_by_table(x)
    } else if x < ERFC_UNDERFLOW {
        (erfc_by_fraction(x), 0.0)
    } else {
        (0.0, 0.0)
    }
}

fn erfc_by_table(x: f64) -> (f64, f64) {
    let index = (x * 4.0 + 0.5) as usize;
    let center = index as f64 * 0.25;
    let step = x - center;
    let doubled = 2.0 * center;
    let (mut previous, mut current) = (0.0, 1.0);
    let mut power = step;
    let mut sum = 0.0;
    let mut sign = 1.0;
    for n in 0..ERFC_TAYLOR_TERMS {
        sum += sign * current * power;
        let next = doubled * current - 2.0 * n as f64 * previous;
        previous = current;
        current = next;
        power *= step / (n + 2) as f64;
        sign = -sign;
    }
    let scale = TWO_OVER_SQRT_PI.0 * exp_parts(-(center * center), 0.0);
    let (high, low) = ERFC_QUARTERS[index - 2];
    add((high, low), (-(scale * sum), 0.0))
}

fn erfc_by_fraction(x: f64) -> f64 {
    let mut tail = 0.0;
    for n in (1..=ERFC_FRACTION_TERMS).rev() {
        tail = (0.5 * f64::from(n)) / (x + tail);
    }
    let (square, error) = two_product(x, x);
    exp_parts(-square, -error) * (0.5 * TWO_OVER_SQRT_PI.0 / (x + tail))
}

fn series_about(step: f64, coefficients: &[f64]) -> f64 {
    step * horner(step, coefficients)
}

/// `log(Γ(a))` for a positive double-double `a`, as a double-double.
fn log_gamma_positive(a: (f64, f64)) -> (f64, f64) {
    let mut shifted = a;
    let mut product = (1.0, 0.0);
    while shifted.0 < STIRLING_START {
        product = multiply(product, shifted);
        shifted = add(shifted, (1.0, 0.0));
    }
    let log_shifted = add(log_double_double(shifted.0), (shifted.1 / shifted.0, 0.0));
    let main = multiply(add(shifted, (-0.5, 0.0)), log_shifted);
    let main = add(add(main, (-shifted.0, -shifted.1)), HALF_LN_2PI);
    let inverse = divide((1.0, 0.0), shifted);
    let inverse_square = inverse.0 * inverse.0;
    let first = divide((1.0, 0.0), multiply((12.0, 0.0), shifted));
    let rest = inverse.0 * inverse_square * horner(inverse_square, &STIRLING_TAIL);
    let log_gamma = add(main, add(first, (rest, 0.0)));
    if product == (1.0, 0.0) {
        return log_gamma;
    }
    let log_product = add(log_double_double(product.0), (product.1 / product.0, 0.0));
    add(log_gamma, (-log_product.0, -log_product.1))
}

/// `lgamma_r`: `log|Γ(x)|` and the sign of `Γ(x)` (1 or -1).
pub fn lgamma_r(x: f64) -> (f64, i32) {
    if x.is_nan() {
        return (x, 1);
    }
    if x.is_infinite() || x == 0.0 || (x < 0.0 && is_integer(x)) {
        return (
            f64::INFINITY,
            if x.is_sign_negative() && x == 0.0 {
                -1
            } else {
                1
            },
        );
    }
    if x > 0.0 {
        return (log_gamma_of_positive(x), 1);
    }
    let sine = sin_pi(x);
    let sign = if sine < 0.0 { -1 } else { 1 };
    let magnitude = -x;
    if magnitude > TWO_POW_60 {
        return (-(magnitude * (log(magnitude) - 1.0)), sign);
    }
    let reflected = log_gamma_positive(two_sum(1.0, magnitude));
    let log_sine = log_double_double(fabs(sine));
    let value = add(
        add(LN_PI, (-log_sine.0, -log_sine.1)),
        (-reflected.0, -reflected.1),
    );
    (value.0 + value.1, sign)
}

fn log_gamma_of_positive(x: f64) -> f64 {
    if x < TWO_POW_MINUS_60 {
        return -log(x);
    }
    if fabs(x - 1.0) <= 0.25 {
        return series_about(x - 1.0, &LGAMMA_ABOUT_ONE);
    }
    if fabs(x - 2.0) <= 0.5 {
        return series_about(x - 2.0, &LGAMMA_ABOUT_TWO);
    }
    if x > TWO_POW_60 {
        return x * (log(x) - 1.0);
    }
    let (high, low) = log_gamma_positive((x, 0.0));
    high + low
}

/// `lgamma`: `log|Γ(x)|`.
pub fn lgamma(x: f64) -> f64 {
    lgamma_r(x).0
}

/// `tgamma`: the gamma function.
pub fn tgamma(x: f64) -> f64 {
    if x.is_nan() || x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return copysign(f64::INFINITY, x);
    }
    if x == f64::NEG_INFINITY || (x < 0.0 && is_integer(x)) {
        return f64::NAN;
    }
    if x > TGAMMA_OVERFLOW {
        return f64::INFINITY;
    }
    if x > 0.0 {
        if x < TWO_POW_MINUS_56 {
            return 1.0 / x;
        }
        let (high, low) = log_gamma_positive((x, 0.0));
        return exp_parts(high, low);
    }
    let sine = sin_pi(x);
    if x < TGAMMA_UNDERFLOW {
        return copysign(0.0, sine);
    }
    let reflected = log_gamma_positive(two_sum(1.0, -x));
    let log_sine = log_double_double(fabs(sine));
    let (high, low) = add(
        add(LN_PI, (-log_sine.0, -log_sine.1)),
        (-reflected.0, -reflected.1),
    );
    copysign(exp_parts(high, low), sine)
}
