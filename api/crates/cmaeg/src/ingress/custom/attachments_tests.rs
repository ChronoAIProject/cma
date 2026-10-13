use super::*;
use serde_json::json;

fn event(data: Value) -> CloudEvent {
    serde_json::from_value(json!({
        "specversion":"1.0","id":"event-1","source":"https://orders.example",
        "type":"order.created","data":data
    }))
    .unwrap()
}

fn config(pointer: &str, origins: &[&str]) -> CustomAttachments {
    CustomAttachments {
        pointer: pointer.into(),
        origins: origins.iter().map(|origin| (*origin).to_owned()).collect(),
    }
}

#[test]
fn a_data_uri_needs_a_restricted_type_base64_and_a_payload() {
    let inline = data_uri("image/png;base64,iVBORw0K").unwrap();
    assert_eq!(inline.mime_type, "image/png");
    assert_eq!(inline.decoded_bytes, 6);
    assert_eq!(
        data_uri("IMAGE/PNG;base64,AA==").unwrap().mime_type,
        "image/png"
    );
    for uri in [
        "image/png,iVBORw0K",
        "image/png;BASE64,AA==",
        "image/png;base64;charset=x,AA==",
        "image/png;base64",
        "image;base64,AA==",
        "image/png ;base64,AA==",
        ";base64,AA==",
        "image/png;base64,AA=",
        "image/png;base64,AA===",
        "image/png;base64,A===",
        "image/png;base64,AB==",
        "image/png;base64,AA==AA==",
        "image/png;base64,AA AA==",
        "image/png;base64,AA_-",
    ] {
        assert!(data_uri(uri).is_none(), "{uri:?}");
    }
}

#[test]
fn parameters_may_precede_the_base64_marker() {
    let inline = data_uri("text/plain;charset=utf-8;name=a.txt;base64,aGk=").unwrap();
    assert_eq!(inline.mime_type, "text/plain");
    assert_eq!(inline.decoded_bytes, 2);
    for uri in [
        "text/plain;;base64,aGk=",
        "text/plain;charset;base64,aGk=",
        "text/plain;=utf-8;base64,aGk=",
        "text/plain;charset=;base64,aGk=",
        "text/plain;char set=x;base64,aGk=",
    ] {
        assert!(data_uri(uri).is_none(), "{uri:?}");
    }
}

#[test]
fn the_decoded_length_is_exact_from_length_and_padding() {
    for size in 0..=12 {
        let encoded = STANDARD.encode(vec![7_u8; size]);
        assert_eq!(decoded_length(&encoded), Some(size), "{encoded}");
    }
    assert_eq!(decoded_length("AAA"), None);
    assert_eq!(decoded_length("A==="), None);
    let largest = STANDARD.encode(vec![0_u8; MAX_INLINE_ATTACHMENT_BYTES]);
    assert_eq!(decoded_length(&largest), Some(196_608));
    assert!(data_uri(&format!("application/octet-stream;base64,{largest}")).is_some());
    let over = STANDARD.encode(vec![0_u8; MAX_INLINE_ATTACHMENT_BYTES + 1]);
    assert!(data_uri(&format!("application/octet-stream;base64,{over}")).is_none());
}

#[test]
fn kinds_default_from_the_mime_prefix() {
    assert_eq!(kind_of(Some("image/png")), AttachmentKind::Image);
    assert_eq!(kind_of(Some("audio/ogg")), AttachmentKind::Audio);
    assert_eq!(kind_of(Some("video/mp4")), AttachmentKind::Video);
    assert_eq!(kind_of(Some("application/pdf")), AttachmentKind::File);
    assert_eq!(kind_of(Some("imagex/png")), AttachmentKind::File);
    assert_eq!(kind_of(None), AttachmentKind::File);
}

#[test]
fn unknown_keys_are_reported_with_escaped_pointers() {
    let error = extract(
        &event(json!({"attachments":[{"url":"data:text/plain;base64,aGk=","a/b~c":1}]})),
        &config("/attachments", &[]),
    )
    .unwrap_err();
    assert_eq!(error.pointer, "/attachments/0/a~1b~0c");
}

#[test]
fn absent_or_null_targets_list_nothing() {
    let setting = config("/attachments", &[]);
    for data in [
        json!(null),
        json!({}),
        json!({"attachments":null}),
        json!([1]),
    ] {
        assert_eq!(extract(&event(data), &setting), Ok(Vec::new()));
    }
    let mut without_data = event(json!(null));
    without_data.data = None;
    assert_eq!(extract(&without_data, &setting), Ok(Vec::new()));
}

#[test]
fn items_of_the_wrong_shape_or_json_type_are_refused_at_their_key() {
    let setting = config("/attachments", &["https://files.example"]);
    let url = "https://files.example/a.png";
    for (item, pointer) in [
        (json!("https://files.example/a.png"), "/attachments/0"),
        (json!({}), "/attachments/0/url"),
        (json!({"url":7}), "/attachments/0/url"),
        (json!({"url":url,"kind":"picture"}), "/attachments/0/kind"),
        (
            json!({"url":url,"mime_type":null}),
            "/attachments/0/mime_type",
        ),
        (
            json!({"url":url,"filename":["a"]}),
            "/attachments/0/filename",
        ),
        (
            json!({"url":url,"size_bytes":-1}),
            "/attachments/0/size_bytes",
        ),
        (
            json!({"url":url,"size_bytes":1.5}),
            "/attachments/0/size_bytes",
        ),
        (
            json!({"url":url,"width":4294967296_u64,"height":1}),
            "/attachments/0/width",
        ),
        (
            json!({"url":url,"width":1,"height":"1"}),
            "/attachments/0/height",
        ),
        (
            json!({"url":url,"duration_seconds":true}),
            "/attachments/0/duration_seconds",
        ),
        (
            json!({"url":"http://files.example/a.png"}),
            "/attachments/0/url",
        ),
        (
            json!({"url":"https://files.example/a.png#x"}),
            "/attachments/0/url",
        ),
        (
            json!({"url":"https://u@files.example/a.png"}),
            "/attachments/0/url",
        ),
        (
            json!({"url":" https://files.example/a.png"}),
            "/attachments/0/url",
        ),
        (
            json!({"url":"https://other.example/a.png"}),
            "/attachments/0/url",
        ),
        (
            json!({"url":"ftp://files.example/a.png"}),
            "/attachments/0/url",
        ),
    ] {
        let data = json!({"attachments":[item]});
        assert_eq!(
            extract(&event(data), &setting).unwrap_err().pointer,
            pointer
        );
    }
}

#[test]
fn facts_are_copied_verbatim_and_inline_items_point_at_their_url() {
    let built = extract(
        &event(json!({"attachments":[
            {"url":"https://files.example:443/report.pdf","filename":"report.pdf",
             "size_bytes":12345,"kind":"other"},
            {"url":"data:image/png;base64,iVBORw0K","width":2,"height":3,"duration_seconds":0}
        ]})),
        &config("/attachments", &["https://files.example"]),
    )
    .unwrap();
    assert_eq!(built.len(), 2);
    assert_eq!(built[0].reference, "custom:0");
    assert_eq!(built[0].kind, AttachmentKind::Other);
    assert_eq!(built[0].filename.as_deref(), Some("report.pdf"));
    assert_eq!(built[0].mime_type, None);
    assert_eq!(built[0].size_bytes, Some(12345));
    assert_eq!(
        built[0].source,
        Some(AttachmentSource::Url {
            url: "https://files.example:443/report.pdf".into()
        })
    );
    assert_eq!(built[1].reference, "custom:1");
    assert_eq!(built[1].kind, AttachmentKind::Image);
    assert_eq!(built[1].mime_type.as_deref(), Some("image/png"));
    assert_eq!(built[1].size_bytes, Some(6));
    assert_eq!((built[1].width, built[1].height), (Some(2), Some(3)));
    assert_eq!(built[1].duration_seconds, Some(0));
    assert_eq!(
        built[1].source,
        Some(AttachmentSource::Inline {
            pointer: "/attachments/1/url".into()
        })
    );
    assert!(built.iter().all(|item| item.variant.is_none()));
}

#[test]
fn elision_replaces_only_a_present_target_when_items_were_admitted() {
    let original = event(json!({"order":7,"media":{"items":["data:..."]}}));
    assert!(matches!(
        elided(&original, "/media/items", 0),
        Cow::Borrowed(_)
    ));
    let replaced = elided(&original, "/media/items", 2);
    assert_eq!(
        replaced.data,
        Some(json!({"order":7,"media":{"items":"[2 attachments, listed below]"}}))
    );
    assert_eq!(elided(&original, "/absent", 1).data, original.data);
}
