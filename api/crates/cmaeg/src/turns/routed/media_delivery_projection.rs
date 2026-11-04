//! Bounded terminal text projection and truthful transport outcome summaries.
use super::*;
pub(in crate::turns::routed) fn summary(state: &MediaResponseState) -> &'static str {
    if let Some(custom) = &state.custom {
        if matches!(custom.local, Some(LocalStreamOutcome::Lost))
            || custom.webhooks.iter().any(|leg| {
                matches!(
                    leg.status,
                    WebhookStatus::Unknown
                        | WebhookStatus::Intent
                        | WebhookStatus::Settled {
                            outcome: DeliveryOutcome::Unknown { .. }
                        }
                )
            })
        {
            return "unknown";
        }
        if custom.local == Some(LocalStreamOutcome::Pending)
            || custom
                .webhooks
                .iter()
                .any(|leg| matches!(leg.status, WebhookStatus::NotStarted))
        {
            return "pending";
        }
        if custom.webhooks.iter().any(|leg| {
            matches!(
                leg.status,
                WebhookStatus::Settled {
                    outcome: DeliveryOutcome::Rejected { .. }
                        | DeliveryOutcome::RateLimited { .. }
                        | DeliveryOutcome::Unsupported
                }
            )
        }) {
            return "failed";
        }
        if custom.local == Some(LocalStreamOutcome::Unavailable) && custom.webhooks.is_empty() {
            return "unavailable";
        }
        if custom.webhooks.is_empty() {
            "buffered"
        } else {
            "confirmed"
        }
    } else {
        match &state.native {
            NativeOutcome::NotStarted => "pending",
            NativeOutcome::Settled {
                outcome: DeliveryOutcome::Rejected { reason },
            } if reason == "automatic_reply_disabled_or_empty" => "suppressed",
            NativeOutcome::Settled {
                outcome: DeliveryOutcome::Confirmed { .. },
            } => "confirmed",
            NativeOutcome::Unknown
            | NativeOutcome::Intent
            | NativeOutcome::Settled {
                outcome: DeliveryOutcome::Unknown { .. },
            } => "unknown",
            _ => "failed",
        }
    }
}
pub(super) fn response_text(value: &Value) -> Result<String, ExecutionError> {
    let output = value["output"].as_array().ok_or(ExecutionError::Corrupt)?;
    let mut parts = Vec::new();
    let mut bytes = 0;
    for item in output {
        if item["type"] != "message" {
            continue;
        }
        let content = item["content"].as_array().ok_or(ExecutionError::Corrupt)?;
        for part in content {
            if part["type"] == "output_text" {
                let text = part["text"].as_str().ok_or(ExecutionError::Corrupt)?;
                bytes += text.len() + 1;
                if bytes > 64 * 1024 {
                    return Err(ExecutionError::Invalid);
                }
                parts.push(text);
            }
        }
    }
    Ok(parts.join("\n"))
}
