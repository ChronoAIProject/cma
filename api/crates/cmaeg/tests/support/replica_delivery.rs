//! Shared by the independent-process harness and delivery Mongo contracts.
//! A terminal provider turn alone never proves an outbound message arrived.
#![allow(dead_code)] // Each integration binary includes only the helpers it needs.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    delivery::intents::DeliveryOutcome,
    ingress::routing_store::{AdmittedPlan, EventIdentity, PlanStore},
    records::{mongo::MongoRecords, RecordClient, RecordError},
    turns::routed::{DeliveryStatus, Journal, Phase, TargetState},
};
use cmaeg_core::seal::{open_interaction, MasterKeyRing};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};

/// Edit the existing signed relay fixture, preserving its source authentication.
/// All messages from its one conversation go to the same durable consumer.
pub fn routed_channel(value: &mut Value) {
    let credential = "runtime-test-provider-bearer";
    value["routing"] = json!({
        "version": 1, "default_consumer": {"name": "lead"},
        "consumers": [], "default_targets": ["default"]
    });
    value["partition"]["key"] = "conversation_and_sender".into();
    value["secrets"]["provider_credential"] = credential.into();
    value["secrets"]["agent_key_hash"] =
        format!("sha256:{:x}", Sha256::digest(credential.as_bytes())).into();
    value["secrets"]["channel_key"] = "test-route-credential".into();
    value["reply"]["ack_after_ms"] = 0.into();
    value["reply"]["auto_final"] = true.into();
}

/// Use the same routing selection at N=1 and N>1. The harness supplies the
/// replica switch separately, so its baseline cannot silently use legacy ingress.
pub fn routed_env(record_id: &str) -> Vec<(String, String)> {
    [
        ("CMAEG_ROUTED_RECOVERY_ENABLED", "true"),
        ("CMAEG_ROUTED_ADMISSION_ENABLED", "true"),
        ("CMAEG_ROUTED_CHANNEL_RECORDS", record_id),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

pub fn runner_keys() -> Arc<MasterKeyRing> {
    Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundEvidence {
    Missing,
    Intent,
    Unknown,
    Partial,
    Rejected,
    RateLimited,
    Unsupported,
    NoEffect,
    Confirmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub completed: bool,
    pub refused: bool,
    pub outbound: OutboundEvidence,
    pub terminal_checkpoint: bool,
    pub cursor_committed: bool,
}
impl Observation {
    /// For the single terminal text-effect workload only. Workloads with earlier
    /// effects also need the sink's full ordered receipt list; one checkpoint
    /// cannot establish the history it has already superseded.
    pub fn confirmed_terminal_text(&self) -> bool {
        self.completed
            && self.outbound == OutboundEvidence::Confirmed
            && self.terminal_checkpoint
            && self.cursor_committed
    }
}

pub fn observe(state: &TargetState) -> Observation {
    let checkpoint = state.delivery.as_ref();
    let outbound = match checkpoint.map(|checkpoint| &checkpoint.status) {
        None => OutboundEvidence::Missing,
        Some(DeliveryStatus::Intent) => OutboundEvidence::Intent,
        Some(DeliveryStatus::Unknown) => OutboundEvidence::Unknown,
        Some(DeliveryStatus::Partial { .. }) => OutboundEvidence::Partial,
        Some(DeliveryStatus::Settled { receipt: None }) => OutboundEvidence::NoEffect,
        Some(DeliveryStatus::Settled {
            receipt: Some(receipt),
        }) => match receipt.delivery {
            DeliveryOutcome::Confirmed { .. } => OutboundEvidence::Confirmed,
            DeliveryOutcome::Rejected { .. } => OutboundEvidence::Rejected,
            DeliveryOutcome::RateLimited { .. } => OutboundEvidence::RateLimited,
            DeliveryOutcome::Unknown { .. } => OutboundEvidence::Unknown,
            DeliveryOutcome::Unsupported => OutboundEvidence::Unsupported,
        },
    };
    let cursor_committed = match (&state.phase, checkpoint) {
        (
            Phase::Accepted {
                response_id,
                durable_cursor,
            }
            | Phase::Completed {
                response_id,
                durable_cursor,
            },
            Some(checkpoint),
        ) => response_id == &checkpoint.response_id && *durable_cursor == Some(checkpoint.cursor),
        _ => false,
    };
    Observation {
        completed: matches!(state.phase, Phase::Completed { .. }),
        refused: matches!(state.phase, Phase::Refused { .. }),
        outbound,
        terminal_checkpoint: checkpoint.is_some_and(|checkpoint| checkpoint.terminal),
        cursor_committed,
    }
}

pub struct DeliveryOracle {
    pub plans: PlanStore,
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
impl DeliveryOracle {
    pub fn journal(&self) -> Journal {
        Journal::new(self.records.clone(), self.keys.clone())
    }

    /// Opens an independent client using the fixture's limited runtime identity.
    /// Never render the URI in assertion output or use the admin client to serve.
    pub async fn open(uri: &str, database: &str, keys: Arc<MasterKeyRing>) -> Self {
        let client = mongodb::Client::with_uri_str(uri)
            .await
            .expect("fixture client");
        let mongo = MongoRecords::open(client.database(database))
            .await
            .expect("fixture grants");
        let records = RecordClient::new("http://127.0.0.1:1", "test-service-bearer")
            .unwrap()
            .with_database(mongo);
        Self {
            plans: PlanStore::new(records.clone(), keys.clone()),
            records: records.interactions(),
            keys,
        }
    }

    pub async fn plan(&self, identity: &EventIdentity) -> Option<AdmittedPlan> {
        self.plans
            .load(identity)
            .await
            .expect("durable admission read")
    }

    pub async fn targets(&self, plan: &AdmittedPlan) -> BTreeMap<String, Option<Observation>> {
        let mut observed = BTreeMap::new();
        for target in &plan.targets {
            let row = match self
                .records
                .get(self.records.record_url(&target.id).unwrap().as_str())
                .await
            {
                Ok(row) => row,
                Err(RecordError::NotFound) => {
                    assert!(observed.insert(target.id.clone(), None).is_none());
                    continue;
                }
                Err(_) => panic!("durable target read failed"),
            };
            let bytes = open_interaction(&self.keys, &target.id, &row.blob)
                .expect("authenticated target record")
                .payload;
            let state: TargetState = serde_json::from_slice(&bytes).expect("target schema");
            assert!(state.id == target.id && state.root == plan.admission_id);
            assert!(state.owner == plan.identity.owner && state.request_hash == plan.request_hash);
            assert_eq!(state.retained_until, plan.retained_until);
            assert_eq!(state.event_expires_at, target.launch.event.expires_at);
            assert!(row.version > 0);
            assert!(observed
                .insert(target.id.clone(), Some(observe(&state)))
                .is_none());
        }
        assert_eq!(observed.len(), plan.targets.len());
        observed
    }
}
