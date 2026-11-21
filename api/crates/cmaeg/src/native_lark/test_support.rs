use super::*;
use base64::Engine as _;
use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use cmaeg_core::channel::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
pub(crate) fn source() -> Source {
    Source::NativeLark(NativeLarkSource {
        separate_app: true,
        conversation_history: false,
        identity: identity(),
        admission: AdmissionPolicy::All {},
    })
}
pub(crate) fn identity() -> NativeLarkIdentity {
    NativeLarkIdentity {
        region: LarkRegion::Feishu,
        app_id: "cli_native_app".into(),
        tenant_key: "native_tenant".into(),
        bot_open_id: "ou_native_bot".into(),
    }
}
pub(crate) fn credentials() -> NativeLarkCredentials {
    NativeLarkCredentials {
        app_secret: Secret::new("fixture-native-app-secret-12345"),
        encrypt_key: Secret::new("fixture-native-encrypt-key-12345"),
        verification_token: Secret::new("fixture-native-verification-12345"),
    }
}
pub(crate) fn envelope(kind: &str, id: &str, event: Value) -> Value {
    json!({"schema":"2.0","header":{"event_type":kind,"event_id":id,"app_id":identity().app_id,"tenant_key":identity().tenant_key,"token":credentials().verification_token.as_str()},"event":event})
}
pub(crate) fn text(id: &str, text: &str, now: i64) -> Value {
    envelope(
        "im.message.receive_v1",
        "receive_event",
        json!({"sender":{"sender_type":"user","tenant_key":identity().tenant_key,"sender_id":{"open_id":"ou_human"}},"message":{"message_id":id,"chat_id":"oc_native_chat","chat_type":"p2p","message_type":"text","content":json!({"text":text}).to_string(),"create_time":(now*1000).to_string()}}),
    )
}
pub(crate) fn reaction(id: &str, target: &str, added: bool, now: i64) -> Value {
    envelope(
        if added {
            "im.message.reaction.created_v1"
        } else {
            "im.message.reaction.deleted_v1"
        },
        id,
        json!({"message_id":target,"operator_type":"user","user_id":{"open_id":"ou_human"},"reaction_type":{"emoji_type":"SMILE"},"action_time":(now*1000).to_string()}),
    )
}
pub(crate) fn recall(id: &str, target: &str, now: i64) -> Value {
    envelope(
        "im.message.recalled_v1",
        id,
        json!({"message_id":target,"chat_id":"oc_native_chat","recall_type":"group_manager","recall_time":(now*1000).to_string()}),
    )
}
pub(crate) fn signed(value: &Value, now: i64) -> (Vec<u8>, String) {
    let credentials = credentials();
    let key = Sha256::digest(credentials.encrypt_key.as_str().as_bytes());
    let iv = [7; 16];
    let plaintext = serde_json::to_vec(value).unwrap();
    let mut buffer = vec![0; plaintext.len() + 16];
    buffer[..plaintext.len()].copy_from_slice(&plaintext);
    let encrypted = cbc::Encryptor::<aes::Aes256>::new(&key, (&iv).into())
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
        .unwrap();
    let mut bytes = iv.to_vec();
    bytes.extend_from_slice(encrypted);
    let body = serde_json::to_vec(
        &json!({"encrypt":base64::engine::general_purpose::STANDARD.encode(bytes)}),
    )
    .unwrap();
    let mut hash = Sha256::new();
    hash.update(now.to_string());
    hash.update("fixture_nonce");
    hash.update(credentials.encrypt_key.as_str());
    hash.update(&body);
    (body, format!("{:x}", hash.finalize()))
}
