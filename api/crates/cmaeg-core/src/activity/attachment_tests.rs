//! Unit tests for the attachment domain shared by every source.
use super::*;

fn attachment(reference: &str) -> Attachment {
    Attachment {
        reference: reference.into(),
        kind: AttachmentKind::File,
        filename: None,
        mime_type: None,
        size_bytes: None,
        source: None,
        variant: None,
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    }
}

fn validate(attachments: &[Attachment]) -> Result<(), ValidationError> {
    validate_attachments(attachments, &SourceKind::Custom, "/kind/attachments")
}

#[test]
fn accepts_up_to_the_attachment_limit() {
    validate(&[]).unwrap();
    validate(&vec![attachment("r"); MAX_ATTACHMENTS]).unwrap();
}

#[test]
fn refuses_too_many_attachments_and_invalid_references() {
    let error = validate(&vec![attachment("r"); MAX_ATTACHMENTS + 1]).unwrap_err();
    assert_eq!(error.pointer, "/kind/attachments");
    assert_eq!(error.message, "too many attachments");
    let oversized = "x".repeat(MAX_REFERENCE_BYTES + 1);
    for reference in ["", oversized.as_str()] {
        let error = validate(&[attachment("r"), attachment(reference)]).unwrap_err();
        assert_eq!(error.pointer, "/kind/attachments/1/reference");
    }
}

#[test]
fn locator_debug_output_hides_identifiers_and_urls() {
    let file_id = "A".repeat(72);
    let rendered = format!(
        "{:?}",
        AttachmentSource::Telegram {
            file_id: file_id.clone(),
            file_unique_id: "unique".into(),
        }
    );
    assert_eq!(
        rendered,
        "Telegram { file_id: <redacted 72 bytes>, file_unique_id: <redacted 6 bytes> }"
    );
    let url = "https://files.example/private?token=hidden";
    let rendered = format!("{:?}", AttachmentSource::Url { url: url.into() });
    assert!(!rendered.contains("files.example") && !rendered.contains("hidden"));
    assert_eq!(
        format!(
            "{:?}",
            AttachmentSource::Inline {
                pointer: "/attachments/0/url".into()
            }
        ),
        "Inline { pointer: <redacted 18 bytes> }"
    );
    assert_eq!(
        format!("{:?}", AttachmentSource::NyxidRelay { index: 3 }),
        "NyxidRelay { index: 3 }"
    );
    let mut item = attachment("r");
    item.thumbnail = Some(AttachmentSource::Url { url: url.into() });
    assert!(!format!("{item:?}").contains("hidden"));
}

#[test]
fn shared_grammars_accept_their_domain_and_refuse_the_rest() {
    assert!(forbidden_text_char('\u{0007}'));
    assert!(forbidden_text_char('\u{0085}'));
    assert!(forbidden_text_char('\u{202E}'));
    assert!(forbidden_text_char('\u{2066}'));
    assert!(forbidden_text_char('\u{200F}'));
    assert!(!forbidden_text_char('é'));
    assert!(!forbidden_text_char('\u{200D}'));
    assert!(telegram_ids_valid("AgAD_-9", "AQAD"));
    assert!(!telegram_ids_valid("", "AQAD"));
    assert!(!telegram_ids_valid(
        "AgAD",
        &"a".repeat(MAX_FILE_UNIQUE_ID_BYTES + 1)
    ));
    assert!(telegram_ids_valid(&"a".repeat(MAX_FILE_ID_BYTES), "u"));
    assert!(!telegram_ids_valid("a/b", "u"));
    for valid in [
        "image/jpeg",
        "application/vnd.ms-excel",
        "audio/ogg",
        "x/a!#$&^_.+-",
    ] {
        assert!(mime_restricted_name(valid), "{valid}");
    }
    let long = format!("a/{}", "b".repeat(MAX_MIME_PART_BYTES + 1));
    for invalid in [
        "image",
        "Image/jpeg",
        "image/",
        "/jpeg",
        "image/jpeg; q=1",
        "image/-jpeg",
        "image/jp/eg",
        long.as_str(),
    ] {
        assert!(!mime_restricted_name(invalid), "{invalid}");
    }
}
