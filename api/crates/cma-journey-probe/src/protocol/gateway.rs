use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use ring::hmac;
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::{
    config::{secret, Gateway},
    model::{now, Intent, Reason},
};

use super::{bind, check_answer, http, sse::Stream, submitted, timestamp, Outcome, Protocol};

impl Protocol {
    fn gateway_target(&self, intent: &Intent) -> Result<&Gateway, Reason> {
        let target = &self
            .config
            .targets(intent.job.mode)
            .ok_or(Reason::Contract)?
            .gateway;
        if target.channel != intent.target
            || intent.provenance.configuration != self.config.revision
        {
            return Err(Reason::Contract);
        }
        Ok(target)
    }

    pub(super) async fn gateway_send(
        &self,
        intent: &Intent,
        outcome: &mut Outcome,
    ) -> Result<(), Reason> {
        let target = self.gateway_target(intent)?;
        let key = secret(&target.secret_file)?;
        let timestamp = now().to_string();
        let hmac = hmac::Key::new(hmac::HMAC_SHA256, key.as_bytes());
        let signed = format!("{}.{timestamp}.{}", intent.key, intent.body);
        let signature = STANDARD.encode(hmac::sign(&hmac, signed.as_bytes()).as_ref());
        let mut signature = reqwest::header::HeaderValue::from_str(&format!("v1,{signature}"))
            .map_err(|_| Reason::Contract)?;
        signature.set_sensitive(true);
        let mut url = target.ingress.clone();
        url.query_pairs_mut().append_pair("mode", "stream");
        let request = self
            .client
            .post(url)
            .header(ACCEPT, "text/event-stream")
            .header(CONTENT_TYPE, "application/cloudevents+json")
            .header("webhook-id", &intent.key)
            .header("webhook-timestamp", timestamp)
            .header("webhook-signature", signature)
            .body(intent.body.clone());
        let response = Self::send(request).await.map_err(|_| Reason::UnknownSend)?;
        submitted(&response, outcome)?;
        let mut stream = Stream::open(response)?;
        let mut answer = String::new();
        let mut terminal = None;
        while let Some(frame) = stream.next().await? {
            if terminal.is_some() {
                return Err(Reason::Contract);
            }
            http::same(&frame.value["event_id"], &intent.key)?;
            bind(
                &mut outcome.evidence.conversation,
                http::id(&frame.value["conversation_id"])?,
            )?;
            match frame.event.as_deref() {
                Some("cmaeg.text.delta") => {
                    frame.value["text"].as_str().ok_or(Reason::Contract)?;
                }
                Some("cmaeg.message") => {
                    answer.push_str(frame.value["text"].as_str().ok_or(Reason::Contract)?);
                    if answer.len() > 32 * 1024 {
                        return Err(Reason::Contract);
                    }
                }
                Some("cmaeg.completed") => {
                    if let Some(text) = frame.value["text"].as_str().filter(|text| !text.is_empty())
                    {
                        if answer != text {
                            return Err(Reason::Contract);
                        }
                    }
                    terminal = Some(true);
                }
                Some("cmaeg.failed") => terminal = Some(false),
                _ => return Err(Reason::Contract),
            }
        }
        match terminal {
            None => Err(Reason::DeliveryLost),
            Some(false) => Err(Reason::TerminalFailed),
            Some(true) => {
                outcome.evidence.result_digest = Some(check_answer(&answer, intent)?);
                outcome.evidence.delivery_at = Some(now());
                Ok(())
            }
        }
    }

    pub(super) async fn gateway_verify(
        &self,
        intent: &Intent,
        outcome: &mut Outcome,
    ) -> Result<(), Reason> {
        let target = self.gateway_target(intent)?;
        let path = format!("/v1/channels/{}/activity/history", target.channel);
        let mut cursor: Option<String> = None;
        for _ in 0..4 {
            let mut url = self.gateway_url(&path);
            if let Some(cursor) = &cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            } else {
                url.query_pairs_mut()
                    .append_pair("from", &time_at(intent.slot - 60)?)
                    .append_pair("until", &time_at(now())?)
                    .append_pair("limit", "20");
            }
            let page = http::json(self.get_owner(url, true, "application/json").await?).await?;
            for item in page["items"].as_array().ok_or(Reason::Contract)? {
                let activity = &item["activity"];
                if activity["event_id"] == intent.key {
                    if item["detail_incomplete"] == true
                        || item["detail_truncated"] == true
                        || item["detail_cursor"].is_string()
                    {
                        return Err(Reason::Contract);
                    }
                    return check_activity(activity, target, outcome);
                }
            }
            match page["has_more"].as_bool() {
                Some(false) => return Err(Reason::ReceiptMissing),
                Some(true) => {
                    let next = page["next_cursor"]
                        .as_str()
                        .filter(|v| !v.is_empty() && v.len() <= 4096)
                        .ok_or(Reason::Contract)?;
                    if cursor.as_deref() == Some(next) {
                        return Err(Reason::Contract);
                    }
                    cursor = Some(next.to_owned());
                }
                None => return Err(Reason::Contract),
            }
        }
        Err(Reason::ReceiptMissing)
    }

    pub(super) async fn gateway_reference(&self) -> Result<(), Reason> {
        let r = &self.config.reference;
        let url = self.gateway_url(&format!("/v1/channels/{}/activity/stream", r.channel));
        let response = self.get_owner(url, true, "text/event-stream").await?;
        let mut stream = Stream::open(response)?;
        let frame = stream.next().await?.ok_or(Reason::DeliveryLost)?;
        let page = frame.value;
        let limit = page["limit"]
            .as_u64()
            .filter(|v| (1..=50).contains(v))
            .ok_or(Reason::Contract)?;
        let items = page["items"].as_array().ok_or(Reason::Contract)?;
        if !matches!(frame.event.as_deref(), Some("activity" | "reset"))
            || frame.id.is_none()
            || frame.id.as_deref() != page["cursor"].as_str()
            || items.len() as u64 > limit
            || page["has_more"].as_bool().is_none()
            || page["retention_days"].as_u64().is_none_or(|v| v == 0)
        {
            return Err(Reason::Contract);
        }
        let item = items
            .iter()
            .find(|item| item["event_id"] == r.event)
            .ok_or(Reason::ReceiptMissing)?;
        if !timestamp(&item["received_at"]) {
            return Err(Reason::Contract);
        }
        let consumer = consumer(item, &r.consumer)?;
        http::same(&consumer["state"], "completed")
    }
}

fn time_at(value: i64) -> Result<String, Reason> {
    OffsetDateTime::from_unix_timestamp(value)
        .map_err(|_| Reason::Contract)?
        .format(&Rfc3339)
        .map_err(|_| Reason::Contract)
}

fn consumer<'a>(activity: &'a Value, id: &str) -> Result<&'a Value, Reason> {
    let mut consumers = activity["consumers"]
        .as_array()
        .ok_or(Reason::Contract)?
        .iter()
        .filter(|record| record["consumer_id"] == id);
    let record = consumers.next().ok_or(Reason::TerminalMissing)?;
    if consumers.next().is_some() {
        return Err(Reason::Contract);
    }
    Ok(record)
}

fn check_activity(activity: &Value, target: &Gateway, outcome: &mut Outcome) -> Result<(), Reason> {
    http::same(&activity["source"]["type"], "custom")?;
    http::same(&activity["actor"]["id"], target.source.as_str())?;
    http::same(&activity["event_type"], &target.event_type)?;
    if !timestamp(&activity["received_at"]) {
        return Err(Reason::Contract);
    }
    outcome.evidence.receipt_at.get_or_insert_with(now);
    let consumer = consumer(activity, &target.consumer)?;
    match consumer["state"].as_str() {
        Some("completed") => {}
        Some("failed" | "refused") => {
            outcome.settled = true;
            outcome.evidence.terminal_at.get_or_insert_with(now);
            return Err(Reason::TerminalFailed);
        }
        _ => return Err(Reason::TerminalMissing),
    }
    bind(
        &mut outcome.evidence.conversation,
        http::id(&consumer["conversation_id"])?,
    )?;
    bind(
        &mut outcome.evidence.response,
        http::id(&consumer["response_id"])?,
    )?;
    if !timestamp(&consumer["accepted_at"]) || !timestamp(&consumer["completed_at"]) {
        return Err(Reason::Contract);
    }
    outcome.settled = true;
    outcome.evidence.terminal_at.get_or_insert_with(now);
    let deliveries = consumer["deliveries"].as_array().ok_or(Reason::Contract)?;
    if outcome.evidence.delivery_at.is_some()
        && deliveries
            .iter()
            .any(|d| d["step"] == "answer" && d["destination"] == "origin")
        && !deliveries.iter().any(|d| {
            d["step"] == "answer" && d["destination"] == "origin" && d["outcome"] == "buffered"
        })
    {
        return Err(Reason::DeliveryLost);
    }
    Ok(())
}
