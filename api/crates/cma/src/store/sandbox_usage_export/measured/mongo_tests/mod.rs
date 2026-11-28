use super::*;
use crate::store::{sandbox_metric_bindings, usage_reports};
use futures_util::TryStreamExt;
use layer2_core::{
    sandbox_metrics::{Binding, BindingKind, ProofStatus},
    sandbox_usage::{Allocation, UsageEvent, UsageEventKind},
};
use layer3_adapter::sandbox_metrics::{PhysicalSeries, Sample, SourceMetric};
use mongodb::{Client, Database};
use std::collections::BTreeMap;

mod authority;
mod bridge;
mod completeness;
mod delivery;
mod inventory;
mod publication;
mod refresh;
mod refresh_source;
mod retention;
mod sample_loss;
mod support_loss;

struct Fixture {
    store: CaptureStore,
    db: Database,
    stream: Stream,
    minute: u64,
}
impl Fixture {
    async fn new() -> Self {
        let client = Client::with_uri_str(
            std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required"),
        )
        .await
        .unwrap();
        let db = client.database(&format!("cma_measured_{}", uuid::Uuid::new_v4().simple()));
        for _ in 0..super::super::schema::MAX_STEPS {
            if !super::super::schema::apply_next(&db).await.unwrap() {
                break;
            }
        }
        for name in [sandbox_metric_bindings::COLLECTION, usage_reports::CONTROL] {
            db.create_collection(name).await.unwrap();
        }
        db.collection::<Document>(usage_reports::CONTROL)
            .insert_one(doc! {"_id":"observer","document_schema_version":1_i64,"revision":1_i64,"control_write_fence":0_i64})
            .await
            .unwrap();
        let config = crate::config::UsageCaptureConfig {
            completeness_enabled: false,
            namespace: "fixture".into(),
            owner_issuer: "https://identity.example".into(),
        };
        let store = CaptureStore::open(&client, db.name(), config)
            .await
            .unwrap();
        let minute = usage_reports::now() / 60 * 60 - 300;
        let stream = Stream {
            id: uuid::Uuid::new_v4().to_string(),
            source_epoch: uuid::Uuid::new_v4().to_string(),
            namespace: "fixture".into(),
            owner: "owner-unfunded-fixture".into(),
            owner_issuer: "https://identity.example".into(),
            sandbox_id: "sandbox-fixture".into(),
            created_at: minute,
            tracking_started_at: minute,
            capture_start: minute,
            first_revision: 1,
            last_revision: 2,
            last_ended_at: minute + 120,
            complete_lifecycle: true,
            sequence: 0,
            previous_digest: None,
            projected_revision: None,
            resource: ResourceSnapshot {
                runtime_id: Some("runtime-fixture".into()),
                runtime_generation: Some(1),
                home_id: Some("logical-home-fixture".into()),
                home_generation: Some("pvc-uid-fixture".into()),
            },
        };
        store
            .collection(STREAMS)
            .insert_one(stream.document().unwrap())
            .await
            .unwrap();
        let allocation = Allocation {
            running: true,
            cpu_millis: 1000,
            memory_mib: 1,
            disk_gib: 1,
        };
        let source = UsageEvent {
            sandbox_id: layer2_core::SandboxId(stream.sandbox_id.clone()),
            owner: layer2_core::SubjectId::new(&stream.owner),
            revision: 2,
            source_seq: 2,
            kind: UsageEventKind::Checkpoint,
            started_at: minute,
            ended_at: minute + 120,
            quantity: allocation.usage(120).unwrap(),
            next_allocation: allocation.clone(),
            allocation,
            status: "active".into(),
        };
        let capture = Captured {
            stream_id: stream.id.clone(),
            source,
            previous_revision: Some(1),
            source_gap: false,
            tracking_started_at: minute,
            complete_lifecycle: true,
            resource: stream.resource.clone(),
        };
        store.collection(CAPTURES).insert_one(doc! {"_id":capture.source.transaction_id(),"document_schema_version":1_i64,"stream_id":&stream.id,
            "sandbox_id":&stream.sandbox_id,"owner":&stream.owner,"revision":2_i64,"source_digest":hash(&serde_json::to_vec(&capture).unwrap()),"record":encode(&capture).unwrap(),"complete":false,"next_second":minute as i64}).await.unwrap();
        let key = keys::prefix("authority", "fixture");
        store
            .collection(INVENTORY)
            .insert_one(
                document(
                    &key,
                    &Authority {
                        namespace: "fixture".into(),
                        owner_issuer: stream.owner_issuer.clone(),
                        capture_start: minute,
                        after: None,
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store.discover_measured().await.unwrap();
        Self {
            store,
            db,
            stream,
            minute,
        }
    }
    async fn lease(&self) -> MeasuredLease {
        self.store
            .claim_measured(&keys::stream("fixture", &self.stream.id))
            .await
            .unwrap()
            .unwrap()
    }
    async fn input(&self, lease: &MeasuredLease) -> MeasuredInput {
        self.store.prepare_measured(lease).await.unwrap().unwrap()
    }
    async fn rows(&self, collection: &str) -> Vec<Document> {
        self.store
            .collection(collection)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap()
    }
    async fn bind(&self) -> Binding {
        let binding = Binding {
            id: "binding-fixture".into(),
            schema_version: 1,
            revision: 1,
            deployment: "fixture".into(),
            cluster: "fixture".into(),
            namespace: "fixture".into(),
            native_sandbox_uid: "native-uid-fixture".into(),
            claim_uid: "claim-fixture".into(),
            sandbox_id: self.stream.sandbox_id.clone(),
            owner_subject: self.stream.owner.clone(),
            pod_name: Some("pod-fixture".into()),
            pod_uid: Some("pod-uid-fixture".into()),
            runtime_container_id: Some("container-fixture".into()),
            restart_count: Some(0),
            runtime_generation: Some(1),
            pvc_name: "pvc-name-fixture".into(),
            pvc_uid: "pvc-uid-fixture".into(),
            kind: BindingKind::Compute,
            cpu_limit_cores: Some(1.0),
            memory_limit_bytes: Some(1024.0),
            valid_from: self.minute,
            valid_until: Some(self.minute + 120),
            observed_at: self.minute + 120,
            proof_version: 1,
            proof_status: ProofStatus::Verified,
        };
        self.store
            .collection(sandbox_metric_bindings::COLLECTION)
            .insert_one(binding_document(&binding))
            .await
            .unwrap();
        let key = format!("{}{:016x}", keys::bridge(&binding.id), self.minute);
        self.store
            .collection(INVENTORY)
            .insert_one(
                document(
                    &key,
                    &BindingProof {
                        binding: binding.clone(),
                        resource: self.stream.resource.clone(),
                        verified_from: self.minute,
                        verified_until: self.minute + 120,
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap();
        binding
    }
    fn samples(&self, delta: f64) -> Vec<PhysicalSeries> {
        vec![PhysicalSeries {
            metric: SourceMetric::CpuCounter,
            name: "pod-fixture".into(),
            labels: BTreeMap::from([
                ("cluster".into(), "fixture".into()),
                ("namespace".into(), "fixture".into()),
            ]),
            samples: vec![
                Sample {
                    at: self.minute as f64,
                    value: Some(10.0),
                },
                Sample {
                    at: (self.minute + 60) as f64,
                    value: Some(10.0 + delta),
                },
            ],
        }]
    }
    async fn events(&self) -> Vec<serde_json::Value> {
        self.store
            .collection(OUTBOX)
            .find(doc! {})
            .sort(doc! {"sequence":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let bytes = r.get_binary_generic("payload").unwrap();
                let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
                super::super::fixture_tests::assert_wire_event(&value);
                assert_eq!(serde_json_canonicalizer::to_vec(&value).unwrap(), *bytes);
                assert_eq!(hash(bytes), r.get_str("digest").unwrap());
                println!("measured-fixture={}",serde_json::json!({"canonical":String::from_utf8(bytes.clone()).unwrap(),"digest":hash(bytes)}));
                value
            })
            .collect()
    }
}
fn binding_document(b: &Binding) -> Document {
    doc! {"_id":&b.id,"document_schema_version":1_i64,"revision":b.revision as i64,"sandbox_id":&b.sandbox_id,"owner":&b.owner_subject,
    "physical_key":b.physical_key(),"valid_from":b.valid_from as i64,"valid_until":b.valid_until.map(|v|v as i64),"record":encode(b).unwrap()}
}
