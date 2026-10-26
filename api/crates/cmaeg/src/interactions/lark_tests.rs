use super::*;

fn message(value: Value) -> OutboundMessage {
    serde_json::from_value(value).unwrap()
}

#[test]
fn native_controls_use_json2_behaviors_and_plain_text_without_provider_values() {
    let message = message(json!({"text":"<at id=all>all</at>","blocks":[
        {"type":"card","title":"Summary","body":"Done","fields":[{"label":"Result","value":"Pass"}]},
        {"type":"buttons","id":"confirm","options":[{"id":"yes","label":"Yes","style":"primary"}]},
        {"type":"choice","id":"scope","prompt":"Scope","multi":true,"options":[{"id":"read","label":"Read","selected":true}]},
        {"type":"link","url":"https://example.com/login","label":"Login"}]}));
    let id = "30d4d6fe-46f6-4e2d-b382-123456789abc";
    let (card, report) = render(&message, id, 7).unwrap();
    assert_eq!(card["schema"], "2.0");
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(
        elements[0],
        json!({"tag":"div","text":{"tag":"plain_text","content":"<at id=all>all</at>"}})
    );
    let actions: Vec<_> = elements
        .iter()
        .filter_map(|e| e.pointer("/behaviors/0/value/cma_action"))
        .collect();
    assert_eq!(
        actions,
        vec![
            &json!("cl1:30d4d6fe46f64e2db382123456789abc:7:0"),
            &json!("cl1:30d4d6fe46f64e2db382123456789abc:7:1")
        ]
    );
    assert!(elements.iter().any(|e| e["text"]["content"] == "✓ Read"));
    assert_eq!(
        elements.last().unwrap()["behaviors"][0]["default_url"],
        "https://example.com/login"
    );
    assert_eq!(report.outcome, RenderingOutcome::Rendered);
}

#[test]
fn native_cards_refuse_media_threads_and_oversize_without_truncation() {
    for value in [
        json!({"text":"x".repeat(4001)}),
        json!({"text":"x".repeat(3995),"blocks":[{"type":"link","url":"https://example.com","label":"Open link"}]}),
        json!({"text":"hello","thread_reply":true}),
        json!({"native_attachments":[{"kind":"file","source":{"type":"base64","data":"YQ=="}}]}),
    ] {
        assert!(render(&message(value), "30d4d6fe46f64e2db382123456789abc", 1).is_err());
    }
    let options: Vec<_> = (0..8)
        .map(|i| json!({"id":format!("o{i}"),"label":"Pick"}))
        .collect();
    let blocks: Vec<_> = (0..13)
        .map(|i| json!({"type":"buttons","id":format!("b{i}"),"options":options}))
        .collect();
    assert!(render(
        &message(json!({"blocks":blocks})),
        "30d4d6fe46f64e2db382123456789abc",
        1
    )
    .is_err());
}

#[test]
fn unsupported_forms_and_card_images_report_their_actual_degradation() {
    let message = message(json!({"blocks":[
        {"type":"card","title":"Chart","image":"https://example.com/chart.png"},
        {"type":"form","id":"form","fields":[{"id":"reason","label":"Reason","required":true,"kind":{"type":"text"}}]}]}));
    let (card, report) = render(&message, "30d4d6fe46f64e2db382123456789abc", 1).unwrap();
    assert_eq!(report.outcome, RenderingOutcome::Degraded);
    assert_eq!(
        report.blocks[0].outcome,
        BlockRenderingOutcome::Degraded {
            how: "image_link".into()
        }
    );
    assert_eq!(
        report.blocks[1].outcome,
        BlockRenderingOutcome::Degraded {
            how: "plain_text_form".into()
        }
    );
    assert!(!card.to_string().contains("form_submit"));
}
