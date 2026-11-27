pub(super) fn scaled_half_even(value: f64, scale: u32) -> Option<u128> {
    if !value.is_finite() || value < 0.0 || scale == 0 {
        return None;
    }
    let bits = value.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let fraction = bits & ((1_u64 << 52) - 1);
    let (significand, exponent) = if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1_u64 << 52), biased - 1023 - 52)
    };
    let numerator = u128::from(significand).checked_mul(u128::from(scale))?;
    if numerator == 0 {
        return Some(0);
    }
    if exponent >= 0 {
        let shift = exponent as u32;
        if shift >= 128 || numerator > (u128::MAX >> shift) {
            return None;
        }
        return Some(numerator << shift);
    }
    let shift = (-exponent) as u32;
    // A 53-bit significand times a u32 scale has at most 85 bits.
    if shift >= 128 {
        return Some(0);
    }
    let integer = numerator >> shift;
    let remainder = numerator & ((1_u128 << shift) - 1);
    let half = 1_u128 << (shift - 1);
    if remainder > half || (remainder == half && integer % 2 == 1) {
        integer.checked_add(1)
    } else {
        Some(integer)
    }
}
