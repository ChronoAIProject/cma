//! Explicit dimensional field names and lossless browser integer serialization.
use serde_json::Value;
pub(super) fn value(value: &impl serde::Serialize) -> Value {
    try_value(value).expect("usage report contains validated finite numbers")
}
pub(super) fn try_value(value: &impl serde::Serialize) -> serde_json::Result<Value> {
    let mut value = serde_json::to_value(value)?;
    dimensional(&mut value);
    safe_integers(&mut value);
    Ok(value)
}
fn dimensional(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(measured) = object.get_mut("measured").and_then(Value::as_object_mut) {
                for metric in ["cpu", "memory", "disk"] {
                    if let Some(reading) = measured.get_mut(metric).and_then(Value::as_object_mut) {
                        let unit = if metric == "cpu" { "cores" } else { "bytes" };
                        for field in ["latest", "mean", "max"] {
                            if let Some(v) = reading.remove(field) {
                                reading.insert(format!("{field}_{unit}"), v);
                            }
                        }
                        if let Some(v) = reading.remove("observed_hours") {
                            reading.insert(
                                if metric == "cpu" {
                                    "observed_core_hours"
                                } else {
                                    "observed_gib_hours"
                                }
                                .into(),
                                v,
                            );
                        }
                    }
                }
            }
            for value in object.values_mut() {
                dimensional(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                dimensional(value);
            }
        }
        _ => {}
    }
}
fn safe_integers(value: &mut Value) {
    match value {
        Value::Number(n) if n.as_u64().is_some_and(|v| v > 9_007_199_254_740_991) => {
            *value = Value::String(n.to_string())
        }
        Value::Number(n)
            if n.as_f64()
                .is_some_and(|v| v > 9_007_199_254_740_991. && v.fract() == 0.) =>
        {
            *value = Value::String(format!("{:.0}", n.as_f64().unwrap()))
        }
        Value::Array(values) => {
            for value in values {
                safe_integers(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                safe_integers(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn integers_above_browser_precision_are_decimal_strings() {
        let v = super::value(&serde_json::json!({"bytes":9007199254740992_f64,
            "seconds":u64::MAX,"fraction":0.125,"safe":9007199254740991_u64}));
        assert_eq!(v["bytes"], "9007199254740992");
        assert_eq!(v["seconds"], "18446744073709551615");
        assert_eq!(v["fraction"], 0.125);
        assert_eq!(v["safe"], 9007199254740991_u64);
    }
}
