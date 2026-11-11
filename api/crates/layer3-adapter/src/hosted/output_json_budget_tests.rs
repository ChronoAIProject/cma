use super::*;

#[test]
fn accepted_values_cover_scalars_escaped_utf8_arrays_and_maps() {
    for value in [
        "null",
        "true",
        "false",
        "-12",
        "18446744073709551615",
        "2.5e12",
        r#""plain""#,
        r#""\u20ac\n\"\\""#,
        r#"{"clé":[null,true,{},[1,2,3]],"\u0061":"\ud83d\ude00"}"#,
    ] {
        assert_eq!(validate(value.as_bytes()), Ok(()), "{value}");
    }
}

#[test]
fn nodes_include_every_key_value_and_container_at_the_exact_boundary() {
    let value = br#"{"a":[0,null],"b":{}}"#;
    // Root, a, array, 0, null, b, empty object.
    assert_eq!(validate_with_limits(value, value.len(), 7, 2), Ok(()));
    assert_eq!(
        validate_with_limits(value, value.len(), 6, 2),
        Err(Refusal::Nodes)
    );
    assert_eq!(
        validate_with_limits(value, value.len() - 1, 7, 2),
        Err(Refusal::Bytes)
    );
    assert_eq!(
        validate_with_limits(value, value.len(), 7, 1),
        Err(Refusal::Depth)
    );
}

#[test]
fn duplicate_keys_cannot_bypass_the_structural_budget() {
    let value = br#"{"a":0,"a":0,"a":0}"#;
    assert_eq!(validate_with_limits(value, 100, 7, 1), Ok(()));
    assert_eq!(validate_with_limits(value, 100, 6, 1), Err(Refusal::Nodes));
}

#[test]
fn shallow_amplification_is_rejected_before_owned_decoding() {
    let at_limit = format!("[{}]", vec!["0"; MAX_NODES - 1].join(","));
    assert_eq!(validate(at_limit.as_bytes()), Ok(()));
    let over_limit = format!("[{}]", vec!["0"; MAX_NODES].join(","));
    assert_eq!(validate(over_limit.as_bytes()), Err(Refusal::Nodes));
}

#[test]
fn container_depth_is_bounded_below_the_parser_recursion_limit() {
    for (depth, expected) in [(MAX_DEPTH, Ok(())), (MAX_DEPTH + 1, Err(Refusal::Depth))] {
        let value = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(validate(value.as_bytes()), expected);
    }
}

#[test]
fn wire_size_is_preserved_for_a_large_low_structure_value() {
    let value = format!("\"{}\"", "x".repeat(MAX_BYTES - 2));
    assert_eq!(validate(value.as_bytes()), Ok(()));
    let too_large = format!("{value} ");
    assert_eq!(validate(too_large.as_bytes()), Err(Refusal::Bytes));
}

#[test]
fn malformed_trailing_and_invalid_utf8_are_refused_without_payload_diagnostics() {
    for value in [
        b"".as_slice(),
        b"{",
        b"[1,]",
        b"null false",
        b"01",
        b"1e400",
        b"\"unterminated",
        b"\"\xff\"",
        b"{1:2}",
        br#""\ud800""#,
    ] {
        assert_eq!(validate(value), Err(Refusal::InvalidJson));
    }
    let message = Refusal::InvalidJson.adapter_error().to_string();
    assert_eq!(
        message,
        crate::AdapterError::Rejected("runtime output JSON is malformed".into()).to_string()
    );
}

#[test]
fn embedded_parser_keys_cannot_bypass_the_opt_in_structural_budget() {
    let inner = format!("[{}]", vec!["0"; 8192].join(","));
    let payload =
        serde_json::to_vec(&serde_json::json!({"$serde_json::private::RawValue":inner})).unwrap();
    // Document why a valid three-node outer document is not safe to decode.
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&payload)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        8192
    );
    assert_eq!(validate(&payload), Err(Refusal::PrivateKey));
    assert_eq!(
        validate(br#"{"\u0024serde_json::private::RawValue":"[]"}"#),
        Err(Refusal::PrivateKey)
    );
    assert_eq!(
        validate(br#"{"value":"$serde_json::private::RawValue"}"#),
        Ok(())
    );
}
