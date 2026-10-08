//! Custom sinks preserve neutral messages as JSON without platform degradation.
use cmaeg_core::{
    message::{
        BlockRendering, BlockRenderingOutcome, OutboundMessage, RenderingOutcome, RenderingReport,
    },
    seal::EventRefPayload,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
pub fn report(message: &OutboundMessage) -> RenderingReport {
    RenderingReport {
        outcome: RenderingOutcome::Rendered,
        blocks: message
            .blocks
            .iter()
            .enumerate()
            .map(|(index, _)| BlockRendering {
                index: index as u16,
                outcome: BlockRenderingOutcome::Exact,
            })
            .collect(),
    }
}
pub fn result(public_origin: &str, event: &EventRefPayload, kind: &str, data: Value) -> Value {
    let channel = format!("{:x}", Sha256::digest(event.channel_id.as_str().as_bytes()));
    json!({"specversion":"1.0","type":kind,"source":format!("{public_origin}/channels/{}", &channel[..16]),
        "id":uuid::Uuid::new_v4().to_string(), "subject":event.event_id,
        "time":time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339).unwrap(),
        "datacontenttype":"application/json", "data":data })
}
pub fn data(
    event: &EventRefPayload,
    text: Option<&str>,
    message: Option<&OutboundMessage>,
    error: Option<cmaeg_core::problem::Problem>,
) -> Value {
    json!({"conversation_id":event.conversation_id,"event_id":event.event_id,"text":text,"message":message,"error":error})
}
/// Tag envelope `data` as delivered to the channel's default reply
/// destination. The envelope's `subject` stays the event id, so a receiver
/// still correlates it with the event it answers.
pub fn to_default(data: &mut Value) {
    data["destination"] = "default".into();
}
