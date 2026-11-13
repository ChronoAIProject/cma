use serde::{Deserialize, Deserializer, de::Error};
/// Observational number or exact decimal integer beyond browser precision.
/// Keep decimal values as strings until choosing a presentation conversion.
#[derive(Clone, Debug, PartialEq)]
pub enum Quantity {
    /// Finite, nonnegative JSON number within the safe integer contract.
    Number(f64),
    /// Canonical unsigned decimal integer greater than 2^53 - 1.
    Decimal(String),
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Wire {
    Number(f64),
    Decimal(String),
}
impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Wire::deserialize(d)? {
            Wire::Number(v)
                if v.is_finite() && v >= 0. && (v.fract() != 0. || v <= 9_007_199_254_740_991.) =>
            {
                Ok(Self::Number(v))
            }
            Wire::Decimal(s) if valid_decimal(&s) => Ok(Self::Decimal(s)),
            _ => Err(D::Error::custom("invalid measured quantity")),
        }
    }
}
fn valid_decimal(s: &str) -> bool {
    (16..=309).contains(&s.len())
        && !s.starts_with('0')
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s.len() > 16 || s > "9007199254740991")
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Integer {
    Number(u64),
    Decimal(String),
}
pub(super) fn integer<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    match Integer::deserialize(d)? {
        Integer::Number(v) if v <= 9_007_199_254_740_991 => Ok(v),
        Integer::Decimal(s) if valid_decimal(&s) => s.parse().map_err(D::Error::custom),
        _ => Err(D::Error::custom("invalid unsigned integer")),
    }
}
pub(super) fn optional_integer<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    struct Parsed(#[serde(deserialize_with = "integer")] u64);
    Option::<Parsed>::deserialize(d).map(|p| p.map(|v| v.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metrics_quantities_preserve_large_integers_and_reject_lossy_json_numbers() {
        for decimal in ["9007199254740992", "18446744073709551615"] {
            assert_eq!(
                serde_json::from_value::<Quantity>(serde_json::json!(decimal)).unwrap(),
                Quantity::Decimal(decimal.into())
            );
        }
        assert_eq!(
            serde_json::from_value::<Quantity>(serde_json::json!(0.003737125)).unwrap(),
            Quantity::Number(0.003737125)
        );
        for bad in [
            serde_json::json!(9007199254740992_u64),
            serde_json::json!(-1),
            serde_json::json!("100"),
            serde_json::json!("01"),
            serde_json::json!("1e30"),
        ] {
            assert!(serde_json::from_value::<Quantity>(bad).is_err());
        }
        #[derive(Deserialize)]
        struct Count(#[serde(deserialize_with = "integer")] u64);
        assert_eq!(
            serde_json::from_str::<Count>("\"18446744073709551615\"")
                .unwrap()
                .0,
            u64::MAX
        );
        assert!(serde_json::from_str::<Count>("\"18446744073709551616\"").is_err());
        assert!(serde_json::from_str::<Count>("9007199254740992").is_err());
    }
}
