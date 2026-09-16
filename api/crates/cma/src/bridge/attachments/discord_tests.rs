use super::*;
use crate::bridge::ingress::{CallbackFixture, NyxidCapabilities};
use layer2_core::bridge::{ingress::*, partition::PartitionRefusal, BridgePlatform};
use serde_json::json;

pub(crate) fn fixture() -> CallbackFixture {
    let mut f = CallbackFixture::new(BridgePlatform::Discord);
    let now = mongodb::bson::DateTime::now().timestamp_millis() / 1000;
    f.body["conversation"]["platform_id"] = json!("123456789012345678");
    f.body["sender"]["platform_id"] = json!("223456789012345678");
    f.body["raw_platform_data"] = json!({"op":0,"t":"MESSAGE_CREATE","d":{
        "id":"323456789012345678","type":0,"channel_id":"123456789012345678",
        "guild_id":"423456789012345678","content":"hello",
        "author":{"id":"223456789012345678","bot":false},
        "attachments":[{"id":"523456789012345678","filename":"note.txt","size":5,"content_type":"text/plain",
            "url":format!("https://cdn.discordapp.com/attachments/123456789012345678/523456789012345678/note.txt?ex={:08x}&is={:08x}&hm={}",now+3600,now-1,"1".repeat(64))}]}});
    f
}

#[tokio::test]
async fn authenticated_original_file_retains_receipt_caption_and_expiring_private_url() {
    let f = fixture();
    let write = f.write().unwrap();
    let inbox = write.record();
    assert_eq!(inbox.admission, InboxAdmission::Accepted);
    assert_eq!(inbox.envelope.text.as_deref(), Some("hello"));
    assert_eq!(
        inbox.attestation.body_sha256,
        format!("{:x}", Sha256::digest(f.bytes()))
    );
    let metadata = inbox.text_attachment.as_ref().unwrap();
    assert_eq!(metadata.custody_expires_at, inbox.accepted_at + 900);
    assert!(metadata.captured.is_none());
    let persisted = serde_json::to_string(inbox).unwrap();
    assert!(persisted.contains("discord_text_attachment"));
    assert!(!persisted.contains("https://cdn.discordapp.com"));
    assert!(write.attachment_url().unwrap().contains("/note.txt?ex="));
    let mut captured = inbox.clone();
    captured.text_attachment.as_mut().unwrap().captured = Some(CapturedAttachmentText {
        text: "hello".into(),
        sha256: format!("{:x}", Sha256::digest(b"hello")),
    });
    let metadata = captured.text_attachment.as_ref().unwrap();
    let prompt = metadata.agent_text(&captured).unwrap();
    let value: serde_json::Value = serde_json::from_str(&prompt).unwrap();
    assert_eq!(value["caption"], "hello");
    assert_eq!(value["attachment"]["untrusted_text"], "hello");
    assert!(metadata
        .require_custody(&captured, metadata.custody_expires_at)
        .is_err());
    // Expiry does not corrupt the canonical text already retained by an Agent.
    assert_eq!(metadata.agent_text(&captured).unwrap(), prompt);
}

#[tokio::test]
async fn unsupported_and_mixed_files_refuse_the_whole_message() {
    for case in 0..15 {
        let mut f = fixture();
        let file = &mut f.body["raw_platform_data"]["d"]["attachments"][0];
        match case {
            0 => file["url"] = json!("https://example.com/note.txt"),
            1 => {
                file["url"] = json!(file["url"]
                    .as_str()
                    .unwrap()
                    .replace("cdn.discordapp.com", "cdn.discordapp.com.evil.example"))
            }
            2 => {
                file["url"] = json!(file["url"]
                    .as_str()
                    .unwrap()
                    .replace("123456789012345678/", "999/"))
            }
            3 => {
                file["url"] = json!(file["url"]
                    .as_str()
                    .unwrap()
                    .replace("523456789012345678/", "999/"))
            }
            4 => {
                file["url"] = json!(file["url"]
                    .as_str()
                    .unwrap()
                    .replace("note.txt?", "%6eote.txt?"))
            }
            5 => file["url"] = json!(format!("{}&ex=ffffffff", file["url"].as_str().unwrap())),
            6 => file["url"] = json!(format!("{}#fragment", file["url"].as_str().unwrap())),
            7 => file["size"] = json!(16_385),
            8 => file["content_type"] = json!("application/pdf"),
            9 => file["filename"] = json!("../note.txt"),
            10 => file["ephemeral"] = json!(true),
            11 => file["url"] = json!(file["url"].as_str().unwrap().replace("?ex=", "?bad=")),
            12 => {
                let file = file.clone();
                f.body["raw_platform_data"]["d"]["attachments"]
                    .as_array_mut()
                    .unwrap()
                    .push(file);
            }
            13 => f.body["raw_platform_data"]["d"]["content"] = json!("different caption"),
            14 => {
                f.body["content"]["attachments"] =
                    json!([{"content_type":"file","url":"https://example.com"}])
            }
            _ => unreachable!(),
        }
        let write = f.write().unwrap();
        assert_eq!(
            write.record().admission,
            InboxAdmission::PartitionDenied {
                reason: PartitionRefusal::AttachmentDownloadRequired
            },
            "case {case}"
        );
        assert!(write.attachment_url().is_none());
    }
}

#[tokio::test]
async fn deployment_opt_in_is_required_and_old_refusal_is_not_reclassified_on_exact_replay() {
    let f = fixture();
    let disabled = crate::bridge::ingress::text_admission::TextAdmission {
        inner: &NyxidCapabilities,
        enabled: false,
    };
    let refused = f
        .signed()
        .unwrap()
        .classify_capabilities(&f.bytes(), &disabled)
        .unwrap()
        .bind(f.binding())
        .unwrap();
    assert_eq!(
        refused.record().admission,
        InboxAdmission::PartitionDenied {
            reason: PartitionRefusal::AttachmentDownloadRequired
        }
    );
    let enabled = f.write().unwrap();
    assert_eq!(
        enabled.match_replay(refused.record()).unwrap(),
        refused.record().receipt()
    );
    assert!(!crate::bridge::ingress::text_admission::enabled(None).unwrap());
    assert!(crate::bridge::ingress::text_admission::enabled(Some("yes")).is_err());
}

#[test]
fn text_validation_rejects_invalid_utf8_binary_controls_and_oversize() {
    for value in [b"\xff".as_slice(), b"a\x00b", b"\x1b[0m", b""] {
        assert!(!valid_attachment_text(value));
    }
    assert!(!valid_attachment_text(&vec![b'a'; 16_385]));
    assert!(valid_attachment_text("untrusted\ntext\t🙂".as_bytes()));
}
