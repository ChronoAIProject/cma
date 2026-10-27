use super::*;

fn message() -> OutboundMessage {
    serde_json::from_value(json!({"blocks":[{"type":"form","id":"business-form","fields":[
        {"id":"reason","label":"Reason","required":true,"kind":{"type":"text"}},
        {"id":"amount","label":"Amount","required":false,"kind":{"type":"number"}},
        {"id":"day","label":"Day","required":false,"kind":{"type":"date"}},
        {"id":"scope","label":"Scope","required":false,"kind":{"type":"choice","options":[
            {"id":"private-read","label":"Read","selected":true},{"id":"private-write","label":"Write"}]}}
    ]}]})).unwrap()
}

#[test]
fn native_lark_form_renders_opaque_json2_inputs_and_one_submit() {
    let (card, count) = render(&message(), "30d4d6fe46f64e2db382123456789abc", 1).unwrap();
    assert!(count > 0);
    assert_eq!(card["tag"], "form");
    let elements = card["elements"].as_array().unwrap();
    assert_eq!(elements[1]["tag"], "input");
    assert_eq!(elements[1]["name"], "f0");
    assert_eq!(elements[1]["max_length"], 1000);
    assert_eq!(elements[3]["input_type"], "text");
    assert_eq!(elements[5]["tag"], "date_picker");
    assert_eq!(elements[7]["tag"], "select_static");
    assert_eq!(elements[7]["initial_option"], "0");
    assert_eq!(elements[7]["options"][1]["value"], "1");
    assert_eq!(elements[8]["form_action_type"], "submit");
    assert_eq!(
        elements[8]["behaviors"][0]["value"]["cma_action"],
        "clf1:30d4d6fe46f64e2db382123456789abc:1:0"
    );
    assert!(!card.to_string().contains("private-"));
    assert!(!card.to_string().contains("business-form"));
}

#[test]
fn native_lark_form_canonicalizes_types_and_refuses_unknown_or_invalid_fields() {
    let message = message();
    let input = json!({"f3":"1","f0":"理由","f1":"-12.5","f2":"2024-02-29 +0800"});
    let result = submitted(&message, 0, input.as_object().unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({"scope":"private-write","reason":"理由","amount":-12.5,"day":"2024-02-29"})
    );
    assert_eq!(
        submitted(
            &message,
            0,
            json!({"f0":"ok","f1":"","f2":null}).as_object().unwrap()
        )
        .unwrap(),
        BTreeMap::from([("reason".into(), json!("ok"))])
    );
    for invalid in [
        json!({}),
        json!({"f0":null}),
        json!({"f0":""}),
        json!({"f0":1}),
        json!({"f0":"ok","f01":"1"}),
        json!({"f0":"ok","reason":"override"}),
        json!({"f0":"ok","f1":"NaN"}),
        json!({"f0":"ok","f1":"1e400"}),
        json!({"f0":"ok","f1":" 1"}),
        json!({"f0":"ok","f1":1}),
        json!({"f0":"ok","f2":"2023-02-29"}),
        json!({"f0":"ok","f2":"2024-01-01 +2460"}),
        json!({"f0":"ok","f3":"01"}),
        json!({"f0":"ok","f3":"private-write"}),
        json!({"f0":"ok","f3":["1"]}),
        json!({"f0":"x".repeat(1001)}),
    ] {
        assert!(
            submitted(&message, 0, invalid.as_object().unwrap()).is_err(),
            "{invalid}"
        );
    }
    assert!(submitted(&message, 1, input.as_object().unwrap()).is_err());
}

#[test]
fn native_lark_form_refuses_unsupported_schema_before_issuance() {
    let original = serde_json::to_value(message()).unwrap();
    for case in [
        "duplicate_field",
        "duplicate_choice",
        "empty_choice",
        "many_choices",
        "defaults",
        "style",
        "two_forms",
        "button",
    ] {
        let mut value = original.clone();
        let options = &mut value["blocks"][0]["fields"][3]["kind"]["options"];
        match case {
            "duplicate_choice" => options[1]["id"] = "private-read".into(),
            "empty_choice" => *options = json!([]),
            "many_choices" => {
                *options = json!((0..21)
                    .map(|i| json!({"id":format!("o{i}"),"label":"Option"}))
                    .collect::<Vec<_>>())
            }
            "defaults" => options[1]["selected"] = true.into(),
            "style" => options[1]["style"] = "danger".into(),
            "duplicate_field" => value["blocks"][0]["fields"][1]["id"] = "reason".into(),
            "two_forms" => {
                let second = value["blocks"][0].clone();
                value["blocks"].as_array_mut().unwrap().push(second);
            }
            _ => value["blocks"].as_array_mut().unwrap().push(
                json!({"type":"buttons","id":"button","options":[{"id":"yes","label":"Yes"}]}),
            ),
        }
        let message = serde_json::from_value(value).unwrap();
        assert!(
            render(&message, "30d4d6fe46f64e2db382123456789abc", 1).is_err(),
            "{case}"
        );
    }
}
