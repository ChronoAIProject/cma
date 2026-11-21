use super::*;

fn message(kind: &str, count: usize) -> Value {
    let options: Vec<_> = (0..count)
        .map(|i| json!({"id":format!("action-{i}"),"label":format!("Option {i}")}))
        .collect();
    let block = if kind == "buttons" {
        json!({"type":"buttons","id":"decision","options":options})
    } else {
        json!({"type":"choice","id":"decision","prompt":"Pick one","multi":false,"options":options})
    };
    json!({"text":"Choose","blocks":[block]})
}
fn rendered(value: Value) -> Result<(Value, RenderingReport), Error> {
    render(
        &serde_json::from_value(value).unwrap(),
        "b23da7cb-2ec1-4f9f-9016-b99946f04223",
        1,
    )
}

#[test]
fn native_button_and_list_shapes_embed_locators_not_action_values() {
    for (kind, max, prefix) in [("buttons", 3, "cw1b:"), ("choice", 10, "cw1l:")] {
        let (body, report) = rendered(message(kind, max)).unwrap();
        assert_eq!(report.outcome, RenderingOutcome::Rendered);
        let values = if kind == "buttons" {
            &body["action"]["buttons"]
        } else {
            &body["action"]["sections"][0]["rows"]
        };
        assert_eq!(values.as_array().unwrap().len(), max);
        for (i, value) in values.as_array().unwrap().iter().enumerate() {
            let locator = if kind == "buttons" {
                value["reply"]["id"].as_str().unwrap()
            } else {
                value["id"].as_str().unwrap()
            };
            assert!(locator.starts_with(prefix));
            let (_, index, revision) = super::super::card::decode(locator).unwrap();
            assert_eq!((index, revision), (i, Some(1)));
            assert!(!locator.contains("action-"));
        }
        assert!(rendered(message(kind, max + 1)).is_err());
    }
}

#[test]
fn bounds_preserve_full_labels_and_refuse_ambiguous_or_mixed_controls() {
    for (kind, limit) in [("buttons", 20), ("choice", 24)] {
        let mut value = message(kind, 1);
        value["blocks"][0]["options"][0]["label"] = "é".repeat(limit).into();
        assert!(rendered(value.clone()).is_ok());
        value["blocks"][0]["options"][0]["label"] = "é".repeat(limit + 1).into();
        assert!(rendered(value).is_err());
    }
    for change in [0, 1, 2, 3, 4, 5, 6] {
        let mut value = message("buttons", 2);
        match change {
            0 => value["blocks"][0]["options"][1]["label"] = "Option 0".into(),
            1 => value["blocks"]
                .as_array_mut()
                .unwrap()
                .push(message("choice", 1)["blocks"][0].clone()),
            2 => value["blocks"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"link","url":"https://example.com"})),
            3 => value["text"] = "x".repeat(1025).into(),
            4 => value["thread_reply"] = true.into(),
            5 => {
                value["native_attachments"] =
                    json!([{"kind":"file","source":{"type":"base64","data":"YQ=="}}])
            }
            _ => value["blocks"][0]["options"][0]["label"] = "newline\nlabel".into(),
        }
        assert!(rendered(value).is_err(), "case {change}");
    }
    let mut value = message("choice", 2);
    value["blocks"][0]["multi"] = true.into();
    assert!(rendered(value).is_err());
    let mut value = message("buttons", 1);
    value["text"] = "x".repeat(1024).into();
    assert!(rendered(value).is_ok());
}

#[test]
fn option_styles_and_preselection_report_the_platform_degradation() {
    let mut value = message("buttons", 1);
    value["blocks"][0]["options"][0]["style"] = "danger".into();
    value["blocks"][0]["options"][0]["selected"] = true.into();
    let (body, report) = rendered(value).unwrap();
    assert_eq!(report.outcome, RenderingOutcome::Degraded);
    assert_eq!(body["action"]["buttons"][0]["reply"]["title"], "✓ Option 0");
}
