use super::*;

fn locator() -> AttachmentSource {
    AttachmentSource::Whatsapp {
        message_id: "wamid.media".into(),
        media_id: "45678".into(),
        sha256: "ab".repeat(32),
    }
}

#[test]
fn whatsapp_locator_requires_its_native_source_and_canonical_signed_identity() {
    let source = SourceKind::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
    };
    assert!(validate_locator(&locator(), &source).is_ok());
    assert!(validate_locator(&locator(), &SourceKind::Custom).is_err());
    assert_eq!(format!("{:?}", locator()), "Whatsapp([REDACTED])");
    for field in 0..4 {
        let mut value = locator();
        let AttachmentSource::Whatsapp {
            message_id,
            media_id,
            sha256,
        } = &mut value
        else {
            panic!()
        };
        match field {
            0 => *message_id = "other-message".into(),
            1 => *media_id = "45678/path".into(),
            2 => *sha256 = "AB".repeat(32),
            _ => *sha256 = "ab".repeat(31),
        }
        assert!(validate_locator(&value, &source).is_err());
    }
    let encoded = serde_json::to_value(locator()).unwrap();
    assert_eq!(encoded["type"], "whatsapp");
    assert_eq!(
        serde_json::from_value::<AttachmentSource>(encoded).unwrap(),
        locator()
    );
}
