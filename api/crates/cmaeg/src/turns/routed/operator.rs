//! Read-only evidence for replica operator commands; no provider dependency.
use super::{journal, tool_reply_store::ReplyStore, *};
use crate::ingress::routing_store::{AdmittedPlan, StoredTarget};
use serde_json::Value;

pub(crate) struct Evidence {
    pub state: String,
    pub eligible: bool,
    pub live_until: i64,
    pub execution: Value,
    pub execution_version: u64,
    pub guard: Value,
    pub guard_id: String,
    pub guard_version: u64,
}
pub(crate) async fn inspect(
    j: &Journal,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<Evidence, ExecutionError> {
    let (guard, guard_version) = ReplyStore(j.clone(), None)
        .operator_record(plan, target)
        .await?;
    let guard_id = guard["id"]
        .as_str()
        .ok_or(ExecutionError::Corrupt)?
        .to_owned();
    let Some((mut state, version)) = j.read::<TargetState>(&target.id).await? else {
        if guard_version != 0 {
            return Err(ExecutionError::Corrupt);
        }
        return Ok(Evidence {
            state: "pending".into(),
            eligible: false,
            live_until: 0,
            execution: Value::Null,
            execution_version: 0,
            guard,
            guard_id,
            guard_version,
        });
    };
    state.version = version;
    journal::validate(&state, plan, target)?;
    let live_until = ["tool", "automatic"]
        .into_iter()
        .filter_map(|key| {
            let attempt = guard.get(key)?;
            attempt
                .get("receipt")
                .is_none_or(Value::is_null)
                .then(|| attempt.get("until").and_then(Value::as_i64).unwrap_or(0))
        })
        .max()
        .unwrap_or(0);
    let status = ReplyStore(j.clone(), None).status(plan, target).await?;
    let delivery_unknown = state.delivery.as_ref().is_some_and(|d| {
        matches!(
            d.status,
            DeliveryStatus::Unknown | DeliveryStatus::Partial { .. }
        )
    });
    let (label, eligible) = match &state.phase {
        Phase::Unknown => ("dispatch_unknown", true),
        Phase::Accepted { .. } if delivery_unknown || status == ToolReplyStatus::Unknown => (
            if matches!(
                state.delivery.as_ref().map(|d| &d.status),
                Some(DeliveryStatus::Partial { .. })
            ) {
                "accepted_delivery_partial"
            } else if delivery_unknown {
                "accepted_delivery_unknown"
            } else {
                "accepted_guard_unknown"
            },
            true,
        ),
        Phase::Accepted { .. } => ("accepted", false),
        Phase::Starting => ("starting", false),
        _ => ("not_uncertain", false),
    };
    Ok(Evidence {
        state: label.into(),
        eligible,
        live_until,
        execution: serde_json::to_value(&state).map_err(|_| ExecutionError::Corrupt)?,
        execution_version: version,
        guard_id: guard["id"].as_str().ok_or(ExecutionError::Corrupt)?.into(),
        guard,
        guard_version,
    })
}

pub(crate) async fn effect(
    j: &Journal,
    plans: &crate::ingress::routing_store::PlanStore,
    plan: &AdmittedPlan,
    target: &StoredTarget,
    work: &str,
) -> Result<(Value, u64, String, bool), ExecutionError> {
    if work != super::media_work::id(target) {
        let (value, version) =
            super::tool_effects::operator_effect(j.clone(), plan, target, work).await?;
        let phase = value["state"]["phase"]
            .as_str()
            .ok_or(ExecutionError::Corrupt)?;
        let (label, eligible) = match phase {
            "unknown" => ("effect_unknown", true),
            "sending" => ("effect_abandoned_sending", true),
            _ => ("effect_not_uncertain", false),
        };
        return Ok((value, version, label.into(), eligible));
    }
    let index = super::MediaWorkIndex::new(j.clone(), plans.clone());
    let head = index
        .load(plan, target)
        .await?
        .ok_or(ExecutionError::Corrupt)?;
    let store = super::MediaObligationStore(j.clone());
    let mut evidence = vec![serde_json::to_value(head).map_err(|_| ExecutionError::Corrupt)?];
    let mut uncertain = false;
    let mut sending = false;
    for n in 0..crate::turns::render_media::attachments(&plan.activity).len() {
        if let Some(obligation) = store.load(plan, target, n as u8).await? {
            for supplement in &obligation.supplements {
                if supplement.admission.is_some() {
                    if let Some(state) = store
                        .retained_media_response(&obligation, supplement.artifact.kind)
                        .await?
                    {
                        if let Some(custom) = &state.custom {
                            use crate::delivery::intents::DeliveryOutcome;
                            use crate::delivery::local_stream::LocalStreamOutcome;
                            sending |= custom.local == Some(LocalStreamOutcome::Pending)
                                || custom
                                    .webhooks
                                    .iter()
                                    .any(|leg| matches!(leg.status, super::WebhookStatus::Intent));
                            uncertain |= custom.local == Some(LocalStreamOutcome::Lost)
                                || custom.webhooks.iter().any(|leg| {
                                    matches!(
                                        leg.status,
                                        super::WebhookStatus::Unknown
                                            | super::WebhookStatus::Settled {
                                                outcome: DeliveryOutcome::Unknown { .. }
                                            }
                                    )
                                });
                        } else {
                            sending |= matches!(
                                state.native,
                                super::media_response_store::NativeOutcome::Intent
                            );
                            uncertain |= !matches!(
                                state.native,
                                super::media_response_store::NativeOutcome::Intent
                            ) && super::media_delivery::summary(&state) == "unknown";
                        }
                        evidence.push(
                            serde_json::to_value(state).map_err(|_| ExecutionError::Corrupt)?,
                        );
                    }
                }
            }
            evidence.push(serde_json::to_value(obligation).map_err(|_| ExecutionError::Corrupt)?);
        }
    }
    Ok((
        Value::Array(evidence),
        1,
        if uncertain {
            "media_effect_unknown"
        } else if sending {
            "media_effect_abandoned_sending"
        } else {
            "media_effect_not_uncertain"
        }
        .into(),
        uncertain || sending,
    ))
}

/// Supplementary media has the original event's shorter reply authority.
pub(crate) async fn media_custody(
    j: &Journal,
    plans: &crate::ingress::routing_store::PlanStore,
    plan: &AdmittedPlan,
    target: &StoredTarget,
) -> Result<Option<(String, bool, bool)>, ExecutionError> {
    if target.launch.event.expires_at <= (j.clock)() {
        return Ok(None);
    }
    let Some((state, _)) = j.read::<TargetState>(&target.id).await? else {
        return Ok(None);
    };
    if state.body.is_none() {
        return Ok(None);
    }
    let body = j.retained_body(&state).await?;
    if super::media_schedule::MediaSchedule::read(&body, plan, target)?.is_none() {
        return Ok(None);
    }
    let inactive = super::MediaWorkIndex::new(j.clone(), plans.clone())
        .load(plan, target)
        .await?
        .is_none();
    let store = super::MediaObligationStore(j.clone());
    let mut refused = true;
    for (n, attachment) in crate::turns::render_media::attachments(&plan.activity)
        .iter()
        .enumerate()
    {
        if attachment.source.is_some() {
            refused &= store
                .load(plan, target, n as u8)
                .await?
                .is_some_and(|record| record.refusal.is_some());
        }
    }
    Ok(Some((super::media_work::id(target), inactive, refused)))
}
