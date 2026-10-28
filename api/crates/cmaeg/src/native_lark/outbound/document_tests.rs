use super::*;
use crate::whatsapp::outbound::document;

#[tokio::test]
async fn native_lark_office_uploads_infer_complete_bytes_and_keep_the_original_encoding() {
    for (kind, bytes) in document::tests::fixtures() {
        let mut item = attachment(kind.mime());
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(&bytes),
        };
        item.mime_type = None;
        let upload = prepared(&item).await.unwrap();
        let wire = String::from_utf8_lossy(&upload.body);
        assert!(wire.contains(&format!("Content-Type: {}", kind.mime())));
        assert!(wire.contains(&format!("attachment-0.{}", kind.extensions()[0])));
        assert!(upload.body.windows(bytes.len()).any(|part| part == bytes));
    }
    for little in [true, false] {
        let mut bytes = if little {
            vec![255, 254]
        } else {
            vec![254, 255]
        };
        for unit in "A UTF-16 document 😀\n".encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        let mut item = attachment("text/plain");
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(&bytes),
        };
        let upload = prepared(&item).await.unwrap();
        assert!(upload.body.windows(bytes.len()).any(|part| part == bytes));
    }
}

#[tokio::test]
async fn native_lark_generic_files_keep_opaque_bytes_and_caller_filenames() {
    for (mime, name) in [
        (None, None),
        (Some("application/octet-stream"), Some("report.backup")),
    ] {
        let mut item = attachment("application/octet-stream");
        item.mime_type = mime.map(str::to_owned);
        item.filename = name.map(str::to_owned);
        let upload = prepared(&item).await.unwrap();
        let wire = String::from_utf8_lossy(&upload.body);
        assert!(wire.contains("Content-Type: application/octet-stream"));
        assert!(wire.contains("name=\"file_type\"\r\n\r\nstream"));
        assert!(wire.contains(name.unwrap_or("attachment-0.bin")));
        assert!(upload
            .body
            .windows(5)
            .any(|part| part == [0, 255, 254, 1, 128]));
    }
    // An explicit generic upload is opaque even when its contents are recognizable.
    let mut item = attachment("application/pdf");
    item.mime_type = Some("application/octet-stream".into());
    item.filename = Some("original.pdf".into());
    assert!(prepared(&item).await.is_ok());
    item.filename = Some("README".into());
    let upload = prepared(&item).await.unwrap();
    assert!(String::from_utf8_lossy(&upload.body).contains("README.bin"));
    item.filename = Some("x".repeat(255));
    assert!(matches!(prepared(&item).await, Err(Error::Invalid)));
}

#[tokio::test]
async fn native_lark_document_labels_cannot_substitute_for_valid_complete_documents() {
    for (kind, bytes) in document::tests::fixtures() {
        if kind == document::Document::Text {
            continue;
        }
        let mut item = attachment(kind.mime());
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(&bytes[..bytes.len() / 2]),
        };
        assert!(
            matches!(prepared(&item).await, Err(Error::Invalid)),
            "{kind:?}"
        );
    }
    for mime in ["application/zip", document::Document::WordXml.mime()] {
        let mut item = attachment(mime);
        item.source = NativeMediaSource::Base64 {
            data: STANDARD.encode(b"PK\x03\x04truncated"),
        };
        assert!(matches!(prepared(&item).await, Err(Error::Invalid)));
    }
    let mut item = attachment(document::Document::WordXml.mime());
    item.filename = Some("wrong.xlsx".into());
    assert!(matches!(prepared(&item).await, Err(Error::Invalid)));
}

#[tokio::test]
async fn native_lark_generic_files_still_have_the_same_size_and_caption_bounds() {
    let item = attachment("application/octet-stream");
    for bytes in [vec![], vec![0; MAX_OUTBOUND_BYTES as usize + 1]] {
        let file = PreparedFile {
            bytes,
            filename: "backup.bin".into(),
            mime: "application/octet-stream".into(),
            caption: None,
        };
        assert!(matches!(
            Client::prepare_media(&item, file).await,
            Err(Error::Invalid)
        ));
    }
    let mut captioned = item;
    captioned.caption = Some("caption".into());
    assert!(matches!(prepared(&captioned).await, Err(Error::Invalid)));
}
