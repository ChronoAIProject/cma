use super::*;
use crate::sandbox::*;

pub(super) fn admitted_agent() -> (AllocationRecord, AllocationAuthority, Sandbox, Agent) {
    let (record, authority) = reserved();
    let request = ChannelSandboxReservation::prepare(authority.clone(), record.clone()).unwrap();
    let sandbox = request.sandbox().clone();
    let AllocationPlacement::NewSandbox { target, profiles } = &record.placement else {
        unreachable!()
    };
    let agent = Agent {
        id: target.agent_id.clone(),
        worktree_id: target.worktree_id.clone(),
        workspace_id: target.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("channel-allocation:{}", record.id),
        creation_fingerprint: channel_fingerprint(&record).unwrap(),
        profile: profiles.agent.clone(),
        capabilities: crate::RuntimeCapabilities::default(),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            conversation_id: None,
            runtime_generation: None,
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: Vec::new(),
        },
        work: AgentWorkState::new(AgentWorkStep::None, sandbox.created_at.clone()),
        failure: None,
        title: None,
        created_at: sandbox.created_at.clone(),
        updated_at: sandbox.created_at.clone(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    (record, authority, sandbox, agent)
}

#[tokio::test]
async fn exact_allocation_proof_captures_channel_origin_with_original_approver_and_source() {
    let (record, authority, sandbox, agent) = admitted_agent();
    let provenance =
        AgentCreationProvenance::for_channel_admission(&authority, &record, &agent, &sandbox)
            .unwrap();
    assert!(
        matches!(&provenance.creation_origin, AgentCreationOrigin::ChannelBridge {
        bridge_id, partition_id, generation, source_reference,
    } if bridge_id == record.identity.bridge_id.as_str() && partition_id == record.identity.key.hash()
        && *generation == record.identity.generation && source_reference == record.first_source.inbox_id())
    );
    let AllocationPlacement::NewSandbox { profiles, .. } = &record.placement else {
        unreachable!()
    };
    assert_eq!(provenance.created_by.as_ref(), Some(&profiles.approved_by));
    provenance.validate_against(&agent, &sandbox).unwrap();
    let old: Agent = serde_json::from_slice(&serde_json::to_vec(&agent).unwrap()).unwrap();
    assert_eq!(old.origin, AgentOrigin::Manual);
    assert_eq!(old.work.step, AgentWorkStep::None);
    let manual = AgentCreationProvenance::for_admission(
        &agent,
        &sandbox,
        record.owner.clone(),
        Some(&profiles.approved_by),
    )
    .unwrap();
    assert_eq!(manual.creation_origin, AgentCreationOrigin::Manual);
}

#[tokio::test(start_paused = true)]
async fn channel_creation_refuses_retrofit_changed_selection_and_expired_authority() {
    let (record, authority, sandbox, agent) = admitted_agent();
    let mut changed = agent.clone();
    changed.revision = 2;
    assert!(AgentCreationProvenance::for_channel_admission(
        &authority, &record, &changed, &sandbox
    )
    .is_err());
    let mut changed = agent.clone();
    changed.creation_key = format!("manual:{}", changed.id);
    assert!(AgentCreationProvenance::for_channel_admission(
        &authority, &record, &changed, &sandbox
    )
    .is_err());
    let mut changed = agent.clone();
    changed.profile.content.system_prompt = "unapproved".into();
    assert!(AgentCreationProvenance::for_channel_admission(
        &authority, &record, &changed, &sandbox
    )
    .is_err());
    let mut changed = agent.clone();
    changed.workspace_id = crate::WorkspaceId("wks_foreign".into());
    assert!(AgentCreationProvenance::for_channel_admission(
        &authority, &record, &changed, &sandbox
    )
    .is_err());
    tokio::time::advance(std::time::Duration::from_secs(16)).await;
    assert!(
        AgentCreationProvenance::for_channel_admission(&authority, &record, &agent, &sandbox)
            .is_err()
    );
}

#[tokio::test]
async fn channel_inbound_generation_may_change_profiles_but_never_partition_or_suspended_creation()
{
    use crate::bridge::{
        invoke::ChannelInvokeBasis, AcceptedWorkPolicy, BridgeSecretRef, BridgeState, BridgeWorkPin,
    };
    let (record, authority, sandbox, agent) = admitted_agent();
    let creation =
        AgentCreationProvenance::for_channel_admission(&authority, &record, &agent, &sandbox)
            .unwrap();
    let basis = ChannelInvokeBasis {
        allocation: record.clone(),
        creation,
    };
    let mut bridge = authority.bridge().clone();
    bridge.revision += 1;
    bridge.config_revision = 2;
    let mut generation = bridge.generations[0].clone();
    generation.config_revision = 2;
    generation.previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    generation.config.display_name = "New inbound settings".into();
    generation.config.connector.key_id = "new-inbound-key".into();
    generation.config.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    if let crate::bridge::BridgePlacementPolicy::NewSandbox { agent_profile, .. } =
        &mut generation.config.placement
    {
        agent_profile.revision = 2;
    }
    let config = generation.config.clone();
    bridge.generations.push(generation);
    let pin = BridgeWorkPin {
        bridge_id: bridge.id.clone(),
        config_revision: 2,
    };
    basis.require_bridge(&bridge, &pin, &config).unwrap();
    assert_eq!(basis.allocation.pin.config_revision, 1);
    assert_eq!(basis.allocation.config, record.config);
    bridge.generations[1].previous_work = Some(AcceptedWorkPolicy::SuspendOriginal);
    assert!(matches!(
        basis.require_bridge(&bridge, &pin, &config),
        Err(crate::bridge::invoke::BridgeInvokeError::Suspended)
    ));
    bridge.generations[1].previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    for field in ["bot", "issuer", "platform", "partition"] {
        let mut changed = config.clone();
        match field {
            "bot" => changed.connector.bot_id = "foreign-bot".into(),
            "issuer" => changed.connector.issuer = "https://other.example".into(),
            "platform" => changed.connector.platform = crate::bridge::BridgePlatform::Discord,
            _ => changed.partition = crate::bridge::BridgePartitionPolicy::Conversation,
        }
        bridge.generations[1].config = changed.clone();
        assert!(
            basis.require_bridge(&bridge, &pin, &changed).is_err(),
            "{field}"
        );
    }
    bridge.generations[1].config = config.clone();
    bridge.state = BridgeState::Disabled {
        accepted_work: AcceptedWorkPolicy::SuspendOriginal,
    };
    assert!(basis.require_bridge(&bridge, &pin, &config).is_err());
}
