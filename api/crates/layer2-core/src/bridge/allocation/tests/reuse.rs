use super::*;
use crate::{bridge::*, profiles::*, resource_access::*, sandbox::*};
use std::time::Duration;
mod fixtures;
use fixtures::*;

#[tokio::test]
async fn shared_modes_require_explicit_trust_and_keep_both_parent_modes_separate() {
    for shared_workspace in [false, true] {
        let (record, authority, resources, _) = reused(shared_workspace).await;
        assert!(!record.placement.creates_sandbox());
        assert!(record.config.placement.creates_partition_agent());
        authority.require_record(&record).unwrap();
        let json = serde_json::to_value(&record).unwrap();
        let roundtrip: AllocationRecord = serde_json::from_value(json).unwrap();
        roundtrip.validate().unwrap();
        assert!(roundtrip == record);
        assert_eq!(
            record.placement.target().sandbox_id,
            resources.lineage.sandbox.id
        );
        if shared_workspace {
            assert_eq!(
                record.placement.target().workspace_id,
                resources.lineage.workspace.as_ref().unwrap().id
            );
        }
        for change in [
            "missing_trust",
            "false_isolation",
            "zero_capacity",
            "too_large_capacity",
            "zero_pin",
        ] {
            let mut config = serde_json::to_value(&record.config).unwrap();
            match change {
                "missing_trust" => {
                    config["placement"]
                        .as_object_mut()
                        .unwrap()
                        .remove("shared_trust");
                }
                "false_isolation" => {
                    config["placement"]["shared_trust"] = "private_per_user_isolation".into()
                }
                "zero_capacity" => {
                    config["placement"]["capacity"]["keep_other_workspaces"] = 0.into()
                }
                "too_large_capacity" => {
                    config["placement"]["capacity"]["keep_other_agents_per_workspace"] = 16.into()
                }
                _ => config["placement"]["agent_profile"]["revision"] = 0.into(),
            }
            assert!(
                serde_json::from_value::<BridgeConfig>(config)
                    .map_or(true, |config| config.validate().is_err()),
                "{change}"
            );
        }
        assert!(matches!(
            ChannelSandboxReservation::prepare(authority, record),
            Err(AllocationError::IdentityMismatch)
        ));
    }
}

#[tokio::test]
async fn reuse_parent_profile_scope_ancestry_and_capacity_cannot_drift() {
    for shared_workspace in [false, true] {
        let (record, authority, resources, _) = reused(shared_workspace).await;
        let AllocationAuthority::Reuse(proof) = authority else {
            unreachable!()
        };
        for change in [
            "root_id",
            "root_creation",
            "root_profile",
            "limit",
            "workspace_creation",
            "service_scope",
        ] {
            let mut changed = resources.clone();
            match change {
                "root_id" => changed.lineage.sandbox.id = crate::SandboxId("foreign".into()),
                "root_creation" => {
                    changed.lineage.sandbox.created_at = "2020-01-01T00:00:00Z".into()
                }
                "root_profile" => changed.lineage.sandbox.profile.revision += 1,
                "limit" => changed.lineage.sandbox.limits.max_agents_per_workspace = 1,
                "workspace_creation" if shared_workspace => {
                    changed.lineage.workspace.as_mut().unwrap().created_at =
                        "2020-01-01T00:00:00Z".into()
                }
                "workspace_creation" => continue,
                _ => changed
                    .lineage
                    .sandbox
                    .credential
                    .reaches
                    .push(crate::ServiceId::new("unapproved-service")),
            }
            assert!(
                proof
                    .parent()
                    .require(&record.config, proof.profiles(), &changed)
                    .is_err(),
                "{change}"
            );
        }
        let mut changed = record.clone();
        if let AllocationPlacement::ReuseParent { target, .. } = &mut changed.placement {
            target.agent_id = crate::AgentId("foreign-agent".into());
        }
        assert!(changed.validate().is_err());
    }
}

#[tokio::test]
async fn only_newly_created_partition_agent_receives_channel_origin_under_manual_or_trigger_root() {
    for shared_workspace in [false, true] {
        let (record, authority, resources, mut agent) = reused(shared_workspace).await;
        let mut sandbox = resources.lineage.sandbox;
        for origin in [
            SandboxOrigin::Manual,
            SandboxOrigin::Trigger {
                launch_id: "launch-existing".into(),
            },
        ] {
            sandbox.origin = origin.clone();
            let provenance = AgentCreationProvenance::for_channel_admission(
                &authority, &record, &agent, &sandbox,
            )
            .unwrap();
            assert!(matches!(
                provenance.creation_origin,
                AgentCreationOrigin::ChannelBridge { .. }
            ));
            assert_eq!(sandbox.origin, origin);
            agent.revision = 2;
            assert!(AgentCreationProvenance::for_channel_admission(
                &authority, &record, &agent, &sandbox
            )
            .is_err());
            agent.revision = 1;
            let manual = AgentCreationProvenance::for_admission(
                &agent,
                &sandbox,
                record.owner.clone(),
                Some(&record.placement.profiles().unwrap().approved_by),
            )
            .unwrap();
            assert!(matches!(
                manual.creation_origin,
                AgentCreationOrigin::Manual
            ));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn resource_management_and_runtime_observation_expiry_cannot_be_extended_by_claim() {
    let (record, authority, _, _) = reused(true).await;
    let claimed =
        AllocationMutation::claim(record, authority.clone(), "reuse-worker", 1001, 60_000).unwrap();
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(claimed.require_current().is_err());
    assert!(authority.require_current().is_err());
}

#[tokio::test]
async fn generation_reset_changes_only_owned_ids_and_keeps_selected_parent() {
    for shared_workspace in [false, true] {
        let (record, _, _, _) = reused(shared_workspace).await;
        let mut identity = record.identity.clone();
        identity.generation += 1;
        let mut next = super::super::model::reserved_target(&identity.id().unwrap()).unwrap();
        next.sandbox_id = record.placement.target().sandbox_id.clone();
        if shared_workspace {
            next.workspace_id = record.placement.target().workspace_id.clone();
        }
        assert_eq!(next.sandbox_id, record.placement.target().sandbox_id);
        assert_ne!(next.agent_id, record.placement.target().agent_id);
        assert_ne!(next.worktree_id, record.placement.target().worktree_id);
        assert_eq!(
            next.workspace_id == record.placement.target().workspace_id,
            shared_workspace
        );
    }
}
