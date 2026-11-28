//! A confirmed source refusal grants at most one bounded relay notice attempt.
use crate::{
    channels::resolver::ResolvedChannel, delivery::relay_client::Credential, ingress::IngressState,
};
use cmaeg_core::{
    channel::Platform,
    ids::{EventId, RecordId},
    seal::{seal_interaction, ReplyAddress},
};
use sha2::{Digest, Sha256};

pub(super) async fn send(
    state: &IngressState,
    channel: &ResolvedChannel,
    event: &EventId,
    occurred_at: &str,
    address: &ReplyAddress,
    text: &'static str,
) {
    let (Some(relay), Ok(_permit)) = (&state.routing_notice_relay, state.inflight.try_acquire())
    else {
        return;
    };
    let ReplyAddress::NyxidRelay {
        platform: Platform::Telegram,
        reply_token,
        reply_token_expires_at,
        ..
    } = address
    else {
        return;
    };
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let Some(timestamp) = crate::ingress::relay::payload::timestamp(occurred_at) else {
        return;
    };
    let expires_at = timestamp.saturating_add(7 * 86_400);
    if *reply_token_expires_at <= now || expires_at <= now {
        return;
    }
    // Claim the event, independently of the reply credential. A redelivery
    // with another valid token cannot grant a second notice attempt.
    let digest = Sha256::digest(format!(
        "cmaeg.telegram-refusal-notice.v1\0{}\0{}",
        channel.definition.record_id, event,
    ));
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let id: RecordId = uuid::Uuid::from_bytes(bytes).to_string().parse().unwrap();
    let Ok(blob) = seal_interaction(
        &state.master_keys,
        id.as_str(),
        br#"{"kind":"telegram_refusal_notice_v1"}"#,
    ) else {
        return;
    };
    let records = state.resolver.record_client().interactions();
    let claim = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        records.put_expiring(
            &id,
            &channel.definition.owner.subject,
            &blob,
            None,
            Some(expires_at),
        ),
    )
    .await;
    if !matches!(claim, Ok(Ok(_)))
        || *reply_token_expires_at <= time::OffsetDateTime::now_utc().unix_timestamp()
    {
        return;
    }
    let outcome = relay
        .send(address, Credential::ReplyToken(reply_token), text)
        .await;
    tracing::debug!(
        outcome = outcome.kind(),
        "Telegram refusal notice completed"
    );
}
