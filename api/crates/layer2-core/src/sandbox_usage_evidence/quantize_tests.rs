use super::quantize::scaled_half_even;

#[test]
fn exact_binary_halfway_values_round_to_the_even_integer() {
    for (value, expected) in [(0.5, 0), (1.5, 2), (2.5, 2), (3.5, 4), (4.5, 4)] {
        assert_eq!(scaled_half_even(value, 1), Some(expected));
    }
    assert_eq!(scaled_half_even(0.0625, 1000), Some(62));
    assert_eq!(scaled_half_even(0.1875, 1000), Some(188));
    assert_eq!(scaled_half_even(0.0625, 1_000_000_000), Some(62_500_000));
}

#[test]
fn neighboring_values_do_not_inherit_the_halfway_tie() {
    let halfway = 2.5_f64.to_bits();
    assert_eq!(scaled_half_even(f64::from_bits(halfway - 1), 1), Some(2));
    assert_eq!(scaled_half_even(f64::from_bits(halfway + 1), 1), Some(3));
    let halfway = 3.5_f64.to_bits();
    assert_eq!(scaled_half_even(f64::from_bits(halfway - 1), 1), Some(3));
    assert_eq!(scaled_half_even(f64::from_bits(halfway + 1), 1), Some(4));
}

#[test]
fn scaling_preserves_integer_precision_beyond_floating_point_products() {
    let value = 4_503_599_627_370_497_f64;
    let expected = 4_503_599_627_370_497_u128 * 1_000_000_000;
    assert_eq!(scaled_half_even(value, 1_000_000_000), Some(expected));
    assert_ne!((value * 1_000_000_000.0) as u128, expected);
    assert_eq!(
        scaled_half_even(9_007_199_254_740_992.0, 1000),
        Some(9_007_199_254_740_992_000)
    );
}

#[test]
fn exact_source_representation_controls_decimal_looking_ties() {
    // The binary value of 0.15 is below 3/20, even though f64 multiplication
    // rounds 0.15 * 10 to the exact halfway value 1.5.
    assert_eq!(0.15_f64 * 10.0, 1.5);
    assert_eq!(scaled_half_even(0.15, 10), Some(1));
    assert_eq!(scaled_half_even(0.25, 10), Some(2));
}

#[test]
fn invalid_inputs_and_overflow_are_refused_before_encoding() {
    for value in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        -1.0,
        -f64::MIN_POSITIVE,
    ] {
        assert_eq!(scaled_half_even(value, 1000), None);
    }
    assert_eq!(scaled_half_even(1.0, 0), None);
    assert_eq!(scaled_half_even(f64::MAX, 1), None);
    assert_eq!(scaled_half_even(2_f64.powi(128), 1), None);
    assert_eq!(scaled_half_even(2_f64.powi(127), 2), None);
    assert_eq!(scaled_half_even(2_f64.powi(127), 1), Some(1_u128 << 127));
}

#[test]
fn zero_subnormal_and_tiny_values_round_without_lossy_casts() {
    assert_eq!(scaled_half_even(0.0, 1000), Some(0));
    assert_eq!(scaled_half_even(-0.0, 1000), Some(0));
    assert_eq!(scaled_half_even(f64::from_bits(1), u32::MAX), Some(0));
    assert_eq!(scaled_half_even(f64::MIN_POSITIVE, u32::MAX), Some(0));
    assert_eq!(scaled_half_even(2_f64.powi(-128), 1), Some(0));
}
