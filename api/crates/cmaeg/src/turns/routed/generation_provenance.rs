//! Derivative events inherit their originating execution's generation. No read
//! here selects a current provider hierarchy or acquires the original lease.
use super::{journal, ExecutionError, Journal, TargetState};
use crate::ingress::routing_store::{
    AdmissionError, AdmittedPlan, EventIdentity, PlanStore, StoredTarget,
};
use cmaeg_core::{
    activity::ActivityKind,
    ids::{EventId, MessageRef},
    seal::MasterKeyRing,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum GenerationProvenance {
    Fresh,
    Inherited {
        original_admission_id: String,
        original_target_id: String,
        /// None is a validated legacy execution, never missing provenance.
        generation: Option<u64>,
    },
}

impl Journal {
    /// Call after authenticating current source/consumer/control custody. Reads
    /// exactly one retained admission and one target checkpoint. A derivative of
    /// another derivative uses its immutable inherited checkpoint, not recursion.
    pub(super) async fn inherited_binding_generation(
        &self,
        plans: &PlanStore,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<GenerationProvenance, ExecutionError> {
        if !plan.targets.contains(target) {
            return Err(ExecutionError::Corrupt);
        }
        let Some(identity) = origin_identity(plan, target, &self.keys)? else {
            return Ok(GenerationProvenance::Fresh);
        };
        let original = plans
            .load(&identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::AuthorityChanged)?;
        let original_target = inherited_target(plan, target, &original)?;
        let generation = self
            .retained_target_generation(&original, original_target)
            .await?;
        Ok(GenerationProvenance::Inherited {
            original_admission_id: original.admission_id.clone(),
            original_target_id: original_target.id.clone(),
            generation,
        })
    }

    async fn retained_target_generation(
        &self,
        original: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<Option<u64>, ExecutionError> {
        let (state, version) = self
            .read::<TargetState>(&target.id)
            .await?
            .ok_or(ExecutionError::Unavailable)?;
        if version == 0 {
            return Err(ExecutionError::Corrupt);
        }
        if original.retained_until <= (self.clock)() {
            return Err(ExecutionError::Expired);
        }
        journal::validate(&state, original, target)?;
        match &state.binding_preparation {
            Some(preparation) => preparation.resolved_generation(),
            // An enabled original that has not yet pinned its generation is
            // unresolved. It must never become a legacy-generation request.
            None if original.prepares_binding(target) => Err(ExecutionError::Unavailable),
            None => Ok(None),
        }
    }
}

fn origin_identity(
    plan: &AdmittedPlan,
    target: &StoredTarget,
    keys: &MasterKeyRing,
) -> Result<Option<EventIdentity>, ExecutionError> {
    let event_id = if plan.is_message_event() {
        if !crate::ingress::message_event::shape(&plan.activity) {
            return Err(ExecutionError::Corrupt);
        }
        let ActivityKind::Event { ce, .. } = &plan.activity.kind else {
            return Err(ExecutionError::Corrupt);
        };
        EventId::new(
            ce.data
                .as_ref()
                .and_then(|data| data["original_event_id"].as_str())
                .ok_or(ExecutionError::Corrupt)?,
        )
        .map_err(|_| ExecutionError::Corrupt)?
    } else if plan.is_native_interaction() {
        if !crate::ingress::native_interaction::shape(&plan.activity) {
            return Err(ExecutionError::Corrupt);
        }
        let ActivityKind::Interaction(interaction) = &plan.activity.kind else {
            return Err(ExecutionError::Corrupt);
        };
        // These values were retained by the authenticated native callback path.
        // Opening the sealed reference independently proves the original event;
        // an arbitrary callback JSON field cannot select another execution.
        let reference: MessageRef = serde_json::from_value(
            interaction
                .values
                .get("message_ref")
                .cloned()
                .ok_or(ExecutionError::Corrupt)?,
        )
        .map_err(|_| ExecutionError::Corrupt)?;
        let reference = reference
            .open(keys)
            .map_err(|_| ExecutionError::Corrupt)?
            .payload;
        let card_id = interaction
            .values
            .get("interaction_record_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            .ok_or(ExecutionError::Corrupt)?;
        let platform_id = interaction
            .values
            .get("platform_message_id")
            .and_then(serde_json::Value::as_str)
            .ok_or(ExecutionError::Corrupt)?;
        if reference.validate().is_err()
            || reference.channel_id != plan.identity.channel_id
            || reference.interaction_record_id.as_deref() != Some(card_id)
            || reference.platform_message_id != platform_id
            || reference.consumer.as_ref()
                != (!target.launch.consumer.is_default()).then_some(&target.launch.consumer)
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        reference.event_id
    } else {
        return Ok(None);
    };
    if event_id == plan.identity.event_id {
        return Err(ExecutionError::Corrupt);
    }
    Ok(Some(EventIdentity {
        event_id,
        ..plan.identity.clone()
    }))
}

fn inherited_target<'a>(
    plan: &AdmittedPlan,
    target: &StoredTarget,
    original: &'a AdmittedPlan,
) -> Result<&'a StoredTarget, ExecutionError> {
    if original.identity.owner != plan.identity.owner
        || original.identity.channel_id != plan.identity.channel_id
        || original.identity.record_id != plan.identity.record_id
        || original.identity.event_id == plan.identity.event_id
        || original.admitted_at > plan.admitted_at
        || original.activity.source != plan.activity.source
        || original.activity.conversation != plan.activity.conversation
        || original.source != plan.source
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    let mut candidates = original
        .targets
        .iter()
        .filter(|original| original.launch.consumer == target.launch.consumer);
    let selected = candidates.next().ok_or(ExecutionError::AuthorityChanged)?;
    if candidates.next().is_some()
        || selected.launch.provider != target.launch.provider
        || selected.launch.event.conversation_id != target.launch.event.conversation_id
        || selected.launch.event.partition_key != target.launch.event.partition_key
        || selected.launch.event.lineage.is_some()
    {
        return Err(ExecutionError::AuthorityChanged);
    }
    Ok(selected)
}

fn admission_error(error: AdmissionError) -> ExecutionError {
    match error {
        AdmissionError::Expired => ExecutionError::Expired,
        AdmissionError::Unavailable
        | AdmissionError::Uncertain
        | AdmissionError::ExpiredOrUnavailable => ExecutionError::Unavailable,
        _ => ExecutionError::Corrupt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turns::routed::{test_support::*, BindingPreparation};
    use serde_json::json;

    #[tokio::test]
    async fn sealed_native_callback_preserves_original_event_consumer_and_card() {
        let setup = Setup::new(supported()).await;
        // This unit test exercises the provenance decoder after native ingress
        // admission; the production PlanStore independently validates the plan.
        let mut value = serde_json::to_value(&setup.plan).unwrap();
        value["kind"] = "native_discord_interaction_payload_v1".into();
        let mut plan: AdmittedPlan = serde_json::from_value(value).unwrap();
        let original = plan.identity.clone();
        let callback_id = EventId::new("native-click-1").unwrap();
        plan.identity.event_id = callback_id.clone();
        plan.activity.event_id = callback_id;
        plan.activity.source = cmaeg_core::activity::SourceKind::Discord {
            bot_id: "123456789012345678".into(),
        };
        plan.targets.truncate(1);
        let routing = plan.activity.routing.as_mut().unwrap();
        routing.resolution = cmaeg_core::activity::RouteResolution::Interaction;
        routing.targets.truncate(1);
        routing.source_id = plan.activity.source.source_id();
        routing.addressing_bytes = 0;
        routing.rule_id = None;
        routing.causation = None;
        let card_id = "912529eb-6227-4635-b7ec-f69389188476";
        let mut reference: cmaeg_core::seal::MessageRefPayload = serde_json::from_value(json!({
            "channel_id":original.channel_id,"event_id":original.event_id,
            "platform_message_id":"123456789012345679",
            "route_id":"discord:123456789012345678:42",
            "interaction_record_id":card_id,
            "consumer":plan.targets[0].launch.consumer
        }))
        .unwrap();
        let sealed = MessageRef::seal(&reference, &fixture::keys()).unwrap();
        plan.activity.kind = serde_json::from_value(json!({"type":"interaction",
            "block_id":"approval","action_id":"yes","values":{
                "message_ref":sealed,"interaction_record_id":card_id,
                "platform_message_id":reference.platform_message_id
            }
        }))
        .unwrap();
        assert_eq!(
            origin_identity(&plan, &plan.targets[0], &fixture::keys()).unwrap(),
            Some(original)
        );

        let mut forged = plan.clone();
        let ActivityKind::Interaction(interaction) = &mut forged.activity.kind else {
            panic!("callback fixture")
        };
        interaction.values["message_ref"] = "cmaeg1.mr.forged".into();
        assert!(origin_identity(&forged, &forged.targets[0], &fixture::keys()).is_err());

        let mut wrong_card = plan.clone();
        let ActivityKind::Interaction(interaction) = &mut wrong_card.activity.kind else {
            panic!("callback fixture")
        };
        interaction.values["interaction_record_id"] = "38aebdd6-dfe8-47d0-803e-8447f4c5c620".into();
        assert!(origin_identity(&wrong_card, &wrong_card.targets[0], &fixture::keys()).is_err());

        reference.consumer = Some(cmaeg_core::ids::ConsumerId::parse(fixture::NYX).unwrap());
        let wrong_consumer = MessageRef::seal(&reference, &fixture::keys()).unwrap();
        let ActivityKind::Interaction(interaction) = &mut plan.activity.kind else {
            panic!("callback fixture")
        };
        interaction.values["message_ref"] = serde_json::to_value(wrong_consumer).unwrap();
        assert!(origin_identity(&plan, &plan.targets[0], &fixture::keys()).is_err());
    }

    #[tokio::test]
    async fn legacy_provenance_requires_a_present_original_execution() {
        let setup = Setup::new(supported()).await;
        assert_eq!(
            setup
                .journal()
                .retained_target_generation(&setup.plan, setup.target())
                .await,
            Err(ExecutionError::Unavailable)
        );
        let _lease = setup
            .journal()
            .claim(&setup.plan, setup.target(), "original")
            .await
            .unwrap();
        assert_eq!(
            setup
                .journal()
                .retained_target_generation(&setup.plan, setup.target())
                .await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn origin_generation_is_read_without_reclaiming_its_live_lease() {
        let mut setup = Setup::new(supported()).await;
        setup.plan.binding_preparation_version = Some(1);
        let journal = setup.journal();
        let mut lease = journal
            .claim(&setup.plan, setup.target(), "original")
            .await
            .unwrap();
        assert_eq!(
            journal
                .retained_target_generation(&setup.plan, setup.target())
                .await,
            Err(ExecutionError::Unavailable)
        );
        journal
            .pin_binding_preparation(
                &mut lease,
                &setup.plan,
                setup.target(),
                BindingPreparation::Pinned {
                    version: 1,
                    binding_id: setup.target().launch.provider.binding_id.to_string(),
                    generation: 7,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            journal
                .retained_target_generation(&setup.plan, setup.target())
                .await,
            Ok(Some(7))
        );
        journal.guard(&lease).await.unwrap();
    }

    #[tokio::test]
    async fn origin_selection_rejects_rebinding_and_cross_conversation_targets() {
        let setup = Setup::new(supported()).await;
        let mut derivative = setup.plan.clone();
        derivative.identity.event_id = EventId::new("new-event").unwrap();
        assert_eq!(
            inherited_target(&derivative, &derivative.targets[0], &setup.plan)
                .unwrap()
                .id,
            setup.target().id
        );
        derivative.targets[0].launch.provider.binding_id =
            cmaeg_core::ids::BindingId::new("another-binding").unwrap();
        assert!(inherited_target(&derivative, &derivative.targets[0], &setup.plan).is_err());
        derivative.targets[0] = setup.target().clone();
        derivative.targets[0].launch.event.partition_key.push('x');
        assert!(inherited_target(&derivative, &derivative.targets[0], &setup.plan).is_err());
    }
}
