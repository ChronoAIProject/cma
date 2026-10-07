use super::*;
use crate::ingress::relay::payload::Content;
use serde_json::{json, Value};

fn parse(value: Value) -> RelayAttachments {
    serde_json::from_slice(value.to_string().as_bytes()).unwrap()
}
fn expected(counts: &[(RepairField, u32)]) -> RelayRepairs {
    let mut repairs = RelayRepairs::default();
    for (field, count) in counts {
        for _ in 0..*count {
            repairs.record(*field);
        }
    }
    repairs
}
/// One NyxID-shaped item with `edit` applied, parsed alone.
fn item(edit: impl FnOnce(&mut Value)) -> (Attachment, RelayRepairs) {
    let mut value = json!({
        "download_url": "https://nyx.example/api/v1/channel-relay/messages/m-1/attachments/0",
        "content_type": "file",
        "url": "https://open.larksuite.com/open-apis/im/v1/messages/om_1/resources/file_1?type=file",
        "platform_message_id": "om_1",
        "file_key": "file_1",
        "filename": "report.pdf",
        "mime_type": "application/pdf",
        "size_bytes": 12
    });
    edit(&mut value);
    let mut list = parse(json!([value]));
    assert_eq!(list.received, 1);
    let kept = list.kept.pop().expect("an object item is always kept");
    assert_eq!(kept.index, 0);
    (kept.attachment, list.repairs)
}

#[test]
fn a_valid_nyxid_item_is_kept_verbatim_without_repairs() {
    let (attachment, repairs) = item(|_| {});
    assert_eq!(repairs, RelayRepairs::default());
    assert_eq!(attachment.content_type, "file");
    assert_eq!(
        attachment.download_url.as_deref(),
        Some("https://nyx.example/api/v1/channel-relay/messages/m-1/attachments/0")
    );
    assert!(attachment.url.starts_with("https://open.larksuite.com/"));
    assert_eq!(attachment.platform_message_id.as_deref(), Some("om_1"));
    assert_eq!(attachment.file_key.as_deref(), Some("file_1"));
    assert_eq!(attachment.image_key, None);
    assert_eq!(attachment.filename.as_deref(), Some("report.pdf"));
    assert_eq!(attachment.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(attachment.size_bytes, Some(12));
}

#[test]
fn null_absent_and_empty_lists_receive_nothing() {
    for list in [
        parse(Value::Null),
        parse(json!([])),
        serde_json::from_str::<Content>(r#"{"type":"text"}"#)
            .unwrap()
            .attachments,
    ] {
        assert_eq!(list.received, 0);
        assert!(list.kept.is_empty());
        assert_eq!(list.repairs, RelayRepairs::default());
    }
}

#[test]
fn a_non_list_container_is_one_malformed_position() {
    for container in [
        json!({}),
        json!({"content_type": "image", "url": ""}),
        json!(7),
        json!("attachments"),
        json!(true),
        json!(-1.5),
    ] {
        let list = parse(container);
        assert_eq!(list.received, 1);
        assert!(list.kept.is_empty());
        assert_eq!(
            list.repairs,
            expected(&[(RepairField::AttachmentContainer, 1)])
        );
    }
}

#[test]
fn a_non_object_item_is_dropped_and_its_sibling_keeps_its_original_index() {
    let list = parse(json!([7, {"content_type": "image", "url": ""}]));
    assert_eq!(list.received, 2);
    assert_eq!(list.kept.len(), 1);
    assert_eq!(list.kept[0].index, 1);
    assert_eq!(list.repairs, expected(&[(RepairField::AttachmentItem, 1)]));
    let list = parse(json!([null, "x", [1, [2]], false, {"content_type": "image", "url": ""}]));
    assert_eq!(list.received, 5);
    assert_eq!(list.kept[0].index, 4);
    assert_eq!(list.repairs, expected(&[(RepairField::AttachmentItem, 4)]));
}

#[test]
fn positions_beyond_thirty_two_are_counted_and_never_kept() {
    let items: Vec<_> = (0..33)
        .map(|_| json!({"content_type": "image", "url": ""}))
        .collect();
    let list = parse(Value::Array(items));
    assert_eq!(list.received, 33);
    assert_eq!(list.kept.len(), MAX_RELAY_ATTACHMENTS);
    assert!(list
        .kept
        .iter()
        .enumerate()
        .all(|(position, kept)| usize::from(kept.index) == position));
    assert_eq!(
        list.repairs,
        expected(&[(RepairField::AttachmentOverflow, 1)])
    );
}

#[test]
fn a_url_is_an_opaque_handle_kept_verbatim() {
    for url in [
        "AgACAgIAAxkBAAIBX2Zx-test_file_id",
        "https://open.larksuite.com/open-apis/im/v1/messages/om_1/resources/img_1?type=image",
        "ftp://files.example/a",
        "not a url",
        "",
    ] {
        let (attachment, repairs) = item(|value| value["url"] = url.into());
        assert_eq!(attachment.url, url);
        assert_eq!(repairs, RelayRepairs::default());
    }
    let longest = "x".repeat(4096);
    let (attachment, repairs) = item(|value| value["url"] = longest.clone().into());
    assert_eq!(attachment.url, longest);
    assert_eq!(repairs, RelayRepairs::default());
}

#[test]
fn an_unusable_url_becomes_empty_and_not_retrievable() {
    for url in [
        json!(42),
        json!("AgACAgIAAxkBAAIBX2Zx\u{7}"),
        json!("https://files.example/\u{0}"),
        Value::Null,
        json!(["a"]),
        json!("x".repeat(4097)),
    ] {
        let (attachment, repairs) = item(|value| value["url"] = url);
        assert_eq!(attachment.url, "");
        assert_eq!(repairs, expected(&[(RepairField::Url, 1)]));
    }
    let (attachment, repairs) = item(|value| {
        value.as_object_mut().unwrap().remove("url");
    });
    assert_eq!(attachment.url, "");
    assert_eq!(repairs, expected(&[(RepairField::Url, 1)]));
}

#[test]
fn only_non_negative_integers_within_i64_are_sizes() {
    for size in [
        json!("12"),
        json!(-1),
        json!(1.5),
        json!(true),
        json!(u64::MAX),
    ] {
        let (attachment, repairs) = item(|value| value["size_bytes"] = size);
        assert_eq!(attachment.size_bytes, None);
        assert_eq!(repairs, expected(&[(RepairField::SizeBytes, 1)]));
    }
    let (attachment, repairs) = item(|value| value["size_bytes"] = i64::MAX.into());
    assert_eq!(attachment.size_bytes, Some(i64::MAX as u64));
    assert_eq!(repairs, RelayRepairs::default());
}

#[test]
fn out_of_bound_filenames_are_dropped() {
    for filename in [
        json!("文".repeat(200)),
        json!("bell\u{7}.txt"),
        json!(""),
        json!(["report.pdf"]),
        json!({"name": "report.pdf"}),
    ] {
        let (attachment, repairs) = item(|value| value["filename"] = filename);
        assert_eq!(attachment.filename, None);
        assert_eq!(repairs, expected(&[(RepairField::Filename, 1)]));
    }
    let (attachment, repairs) = item(|value| value["filename"] = Value::Null);
    assert_eq!(attachment.filename, None);
    assert_eq!(repairs, RelayRepairs::default());
}

#[test]
fn every_optional_fact_has_its_own_bound_and_repair_label() {
    for (key, max, field) in [
        ("platform_message_id", 256, RepairField::PlatformMessageId),
        ("file_key", 1024, RepairField::FileKey),
        ("image_key", 1024, RepairField::ImageKey),
        ("mime_type", 256, RepairField::MimeType),
        ("download_url", 4096, RepairField::DownloadUrl),
    ] {
        let (_, repairs) = item(|value| value[key] = "x".repeat(max).into());
        assert_eq!(repairs, RelayRepairs::default(), "{key}");
        let (_, repairs) = item(|value| value[key] = "x".repeat(max + 1).into());
        assert_eq!(repairs, expected(&[(field, 1)]), "{key}");
    }
    let (attachment, _) = item(|value| value["mime_type"] = "m".repeat(300).into());
    assert_eq!(attachment.mime_type, None);
}

#[test]
fn a_missing_or_invalid_content_type_is_unknown() {
    for content_type in [None, Some(json!(3)), Some(json!("")), Some(json!("a\u{0}"))] {
        let (attachment, repairs) = item(|value| match content_type {
            None => {
                value.as_object_mut().unwrap().remove("content_type");
            }
            Some(content_type) => value["content_type"] = content_type,
        });
        assert_eq!(attachment.content_type, "unknown");
        assert_eq!(repairs, expected(&[(RepairField::ContentType, 1)]));
    }
}

#[test]
fn unknown_keys_are_ignored_and_the_first_duplicate_wins() {
    let (attachment, repairs) = item(|value| value["extra"] = "e".repeat(512 * 1024).into());
    assert_eq!(attachment.filename.as_deref(), Some("report.pdf"));
    assert_eq!(repairs, RelayRepairs::default());
    let body =
        r#"[{"content_type":"file","url":"","filename":"first.txt","filename":"second.txt"}]"#;
    let list: RelayAttachments = serde_json::from_str(body).unwrap();
    assert_eq!(
        list.kept[0].attachment.filename.as_deref(),
        Some("first.txt")
    );
    assert_eq!(list.repairs, RelayRepairs::default());
}

#[test]
fn an_in_memory_value_parses_with_the_same_rules() {
    let list: RelayAttachments = serde_json::from_value(json!([
        "dropped",
        {"content_type": "image", "url": 1, "size_bytes": "1"}
    ]))
    .unwrap();
    assert_eq!(list.received, 2);
    assert_eq!(list.kept[0].index, 1);
    assert_eq!(
        list.repairs,
        expected(&[
            (RepairField::AttachmentItem, 1),
            (RepairField::Url, 1),
            (RepairField::SizeBytes, 1),
        ])
    );
}

#[test]
fn malformed_json_inside_the_list_is_still_a_parse_error() {
    assert!(serde_json::from_str::<RelayAttachments>(r#"[{"url": }]"#).is_err());
}

#[test]
fn repairs_report_only_non_zero_counts_with_distinct_labels() {
    let mut repairs = RelayRepairs::default();
    assert_eq!(repairs.iter().count(), 0);
    repairs.record(RepairField::Filename);
    repairs.record(RepairField::Filename);
    repairs.record(RepairField::SenderDisplayName);
    assert_eq!(
        repairs.iter().collect::<Vec<_>>(),
        [
            (RepairField::Filename, 2),
            (RepairField::SenderDisplayName, 1)
        ]
    );
    assert_eq!(repairs.count(RepairField::Url), 0);
    let mut labels: Vec<_> = RepairField::ALL.iter().map(|field| field.label()).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), RepairField::ALL.len());
}
