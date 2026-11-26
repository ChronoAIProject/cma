use serde_json::json;

use super::*;

#[test]
fn every_result_kind_is_the_original_node() {
    for result in [
        Value::Null,
        json!(false),
        json!(17),
        json!(1.5),
        json!("synthetic"),
        json!([1, "synthetic"]),
        json!({"value": "synthetic"}),
    ] {
        let frame = json!({"id": 1, "result": result});
        let NativeResponseView::Result(view) = inspect_response(&frame, 1).unwrap() else {
            panic!("expected result");
        };
        assert!(std::ptr::eq(view, frame.get("result").unwrap()));
    }
}

#[test]
fn zero_and_maximum_expected_ids_match() {
    for id in [0, 1, u64::MAX] {
        let frame = json!({"id": id, "result": null});
        assert_eq!(
            inspect_response(&frame, id),
            Ok(NativeResponseView::Result(&Value::Null))
        );
    }
}

#[test]
fn matching_result_precedes_error_and_notification() {
    let frame = json!({
        "id": 1, "result": false, "error": {"message": "synthetic"},
        "method": "account/login/completed",
        "params": {"loginId": "synthetic", "success": true}
    });
    assert_eq!(
        inspect_response(&frame, 1),
        Ok(NativeResponseView::Result(&frame["result"]))
    );
}

#[test]
fn matching_missing_result_errors_before_notification() {
    for frame in [
        json!({"id": 1}),
        json!({"id": 1, "error": {"message": "synthetic"}}),
        json!({"id": 1, "method": "account/login/completed",
            "params": {"loginId": "synthetic", "success": true}}),
    ] {
        assert_eq!(
            inspect_response(&frame, 1),
            Err(NativeFrameError::Unavailable)
        );
    }
}

#[test]
fn only_unsigned_integer_ids_match() {
    for id in [json!(2), json!(-1), json!("1"), json!(1.0), Value::Null] {
        let frame = json!({"id": id, "result": false});
        assert_eq!(inspect_response(&frame, 1), Ok(NativeResponseView::Ignore));
    }
    let exponent: Value = serde_json::from_str(r#"{"id":1e0,"result":false}"#).unwrap();
    assert_eq!(
        inspect_response(&exponent, 1),
        Ok(NativeResponseView::Ignore)
    );
}

#[test]
fn nonmatching_ids_can_carry_completion() {
    for id in [json!(2), json!(-1), json!("1"), json!(1.0), Value::Null] {
        let frame = json!({"id": id, "method": "account/login/completed",
            "params": {"loginId": "synthetic", "success": false}});
        assert_eq!(
            inspect_response(&frame, 1),
            Ok(NativeResponseView::Completed {
                login_id: "synthetic",
                success: false
            })
        );
    }
}

#[test]
fn completion_preserves_empty_id_and_both_boolean_values() {
    for login_id in ["", "synthetic"] {
        for success in [false, true] {
            let frame = json!({"method": "account/login/completed",
                "params": {"loginId": login_id, "success": success}});
            assert_eq!(
                inspect_response(&frame, 1),
                Ok(NativeResponseView::Completed { login_id, success })
            );
        }
    }
}

#[test]
fn escaped_completion_id_borrows_original_storage() {
    let frame: Value = serde_json::from_str(
        r#"{"method":"account/login/completed","params":{"loginId":"a\u0000\n\uD83D\uDE00","success":true}}"#,
    )
    .unwrap();
    let original = frame["params"]["loginId"].as_str().unwrap();
    let NativeResponseView::Completed { login_id, success } = inspect_response(&frame, 1).unwrap()
    else {
        panic!("expected completion");
    };
    assert_eq!(login_id, "a\0\n😀");
    assert_eq!(login_id.as_ptr(), original.as_ptr());
    assert_eq!(login_id.len(), original.len());
    assert!(success);
}

#[test]
fn wrong_shaped_params_and_fields_are_ignored() {
    for params in [
        Value::Null,
        json!(false),
        json!(1),
        json!("synthetic"),
        json!([{"loginId": "synthetic", "success": true}]),
        json!({}),
        json!({"loginId": 1, "success": true}),
        json!({"loginId": "synthetic", "success": "true"}),
        json!({"loginId": "synthetic"}),
        json!({"success": true}),
    ] {
        let frame = json!({"method": "account/login/completed", "params": params});
        assert_eq!(inspect_response(&frame, 1), Ok(NativeResponseView::Ignore));
    }
}

#[test]
fn unrelated_methods_and_nonobject_frames_are_ignored() {
    for frame in [
        Value::Null,
        json!(false),
        json!(1),
        json!("synthetic"),
        json!([]),
        json!({}),
        json!({"result": true}),
        json!({"method": "account/login/completed"}),
        json!({"method": "other", "params": {"loginId": "synthetic", "success": true}}),
        json!({"method": true, "params": {"loginId": "synthetic", "success": true}}),
    ] {
        assert_eq!(inspect_response(&frame, 1), Ok(NativeResponseView::Ignore));
    }
}

#[test]
fn parser_duplicates_preserve_last_value_and_insertion_order() {
    let frame: Value =
        serde_json::from_str(r#"{"id":9,"id":1,"result":false,"result":{"z":1,"a":2,"z":3}}"#)
            .unwrap();
    let NativeResponseView::Result(result) = inspect_response(&frame, 1).unwrap() else {
        panic!("expected result");
    };
    assert_eq!(serde_json::to_string(result).unwrap(), r#"{"z":3,"a":2}"#);
    assert!(std::ptr::eq(result, frame.get("result").unwrap()));
}

#[test]
fn parsed_raw_marker_values_are_inspected_without_reinterpretation() {
    let frame: Value =
        serde_json::from_str(r#"{"$serde_json::private::RawValue":"{\"id\":1,\"result\":[1,2]}"}"#)
            .unwrap();
    assert_eq!(
        inspect_response(&frame, 1),
        Ok(NativeResponseView::Result(&frame["result"]))
    );
    let ordinary = json!({"$serde_json::private::RawValue": "{\"id\":1,\"result\":true}"});
    assert_eq!(
        inspect_response(&ordinary, 1),
        Ok(NativeResponseView::Ignore)
    );
}
