use super::{
    tests::{envelope, source},
    webhook::{self, Content},
};
use serde_json::json;
fn raw(list: bool) -> serde_json::Value {
    let kind = if list { "list_reply" } else { "button_reply" };
    json!({"from":"447700900123","id":"wamid.callback","timestamp":"1800000000","type":"interactive","context":{"id":"wamid.sent","from":"15550001234"},"interactive":{"type":kind,kind:{"id":format!("{}:12a4567890124123a123123456789012:1:0",if list { "cw1l" } else { "cw1b" }),"title":"Untrusted title"}}})
}
#[test]
fn native_whatsapp_controls_extract_only_bounded_locator_and_context_facts() {
    for list in [false, true] {
        let original = raw(list);
        let messages = webhook::extract(&source(), &envelope(vec![original.clone()])).unwrap();
        assert!(
            matches!(&messages[0].content,Content::Control { context,.. } if context=="wamid.sent")
        );
        let mut changed = original;
        let kind = if list { "list_reply" } else { "button_reply" };
        changed["interactive"][kind]["title"] = json!("Different display text");
        changed["context"]["from"] = json!("15550009999");
        assert_eq!(
            messages,
            webhook::extract(&source(), &envelope(vec![changed])).unwrap()
        );
    }
}
#[test]
fn native_whatsapp_controls_refuse_foreign_mixed_unbounded_and_wrong_shape_payloads() {
    for case in 0..10 {
        let mut value = raw(false);
        match case {
            0 => value["interactive"]["button_reply"]["id"] = json!("foreign-control"),
            1 => value["interactive"]["type"] = json!("list_reply"),
            2 => value["interactive"]["list_reply"] = json!({"id":"another"}),
            3 => {
                value["interactive"]["button_reply"]["id"] =
                    json!(format!("cw1b:{}", "x".repeat(65)))
            }
            4 => value["context"].as_object_mut().unwrap().clear(),
            5 => value["context"]["id"] = json!(false),
            6 => value["from"] = json!("not-numeric"),
            7 => value["group_id"] = json!("group"),
            8 => value["interactive"]["button_reply"]["id"] = json!("cw1b:\n"),
            _ => value["interactive"]["button_reply"]["id"] = serde_json::Value::Null,
        }
        assert!(
            webhook::extract(&source(), &envelope(vec![value])).is_err(),
            "case {case}"
        );
    }
}
