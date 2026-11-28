//! Exercise the real reconciler against Mongo and disposable home storage.
use super::*;
use crate::sandbox::SandboxReconciler;
use layer2_core::sandbox::{SandboxHome, SandboxPolicy, SandboxWorkState};
use layer2_core::SandboxId;
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeStore,
};
use mongodb::bson::{Bson, Document};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};
use tokio::sync::Mutex;

#[derive(Default)]
struct Homes {
    stored: Mutex<BTreeMap<String, HomeVolumeAttachment>>,
    removed: Mutex<Vec<HomeVolumeAttachment>>,
}

#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.stored.lock().await.values().cloned().collect())
    }

    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        request.validate()?;
        let receipt = HomeVolumeReceipt {
            native_authority: None,
            volume_id: request.volume_id.clone(),
            request_sha256: request.computed_sha256(),
            storage_reference: format!("fixture-{}", request.volume_id),
            storage_generation: "original-fixture-generation".into(),
        };
        let attachment = HomeVolumeAttachment {
            request: request.clone(),
            receipt: receipt.clone(),
        };
        let previous = self
            .stored
            .lock()
            .await
            .insert(request.volume_id.clone(), attachment.clone());
        assert!(previous.is_none_or(|previous| previous == attachment));
        Ok(receipt)
    }

    async fn verify(&self, attachment: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        attachment.validate_for(&attachment.request.sandbox_id)?;
        assert_eq!(
            self.stored.lock().await.get(&attachment.request.volume_id),
            Some(attachment)
        );
        Ok(())
    }

    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        let observed = self.stored.lock().await.remove(&request.volume_id).unwrap();
        assert_eq!(&observed.request, request);
        assert_eq!(receipt, Some(&observed.receipt));
        self.removed.lock().await.push(observed);
        Ok(())
    }
}

async fn home(homes: &Homes, id: &SandboxId) -> SandboxHome {
    let mut home = SandboxHome::reserve(
        id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 1,
            template_name: "fixture-seed".into(),
            template_generation: "fixture-seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(homes.prepare(&home.request).await.unwrap())
        .unwrap();
    home
}

async fn root(
    store: &MongoSandboxStore,
    homes: &Homes,
    id: &str,
    canonical: &str,
    marker: Option<&str>,
) -> SandboxHome {
    let mut sandbox = crate::store::sandbox_mongo_integration_tests::sandbox(id);
    sandbox.name = id.into();
    sandbox.status = SandboxStatus::Retiring;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::MarkRetired, sandbox.updated_at.clone());
    let retained = home(homes, &sandbox.id).await;
    let mut current = retained.clone();
    match canonical {
        "missing" | "missing_root" => sandbox.lifecycle.home = None,
        "changed" => {
            current.receipt.as_mut().unwrap().storage_generation = "replacement-generation".into();
            current.request.volume_id.push_str("-replacement");
            let receipt = current.receipt.as_mut().unwrap();
            receipt.volume_id = current.request.volume_id.clone();
            receipt.request_sha256 = current.request.computed_sha256();
            sandbox.lifecycle.home = Some(Box::new(current));
        }
        "released" => {
            current.released = true;
            sandbox.lifecycle.home = Some(Box::new(current));
        }
        "matching" => sandbox.lifecycle.home = Some(Box::new(current)),
        _ => panic!("unknown fixture canonical home"),
    }
    sandbox.validate().unwrap();
    if canonical != "missing_root" {
        let mut row = sandbox_document(&sandbox, 0).unwrap();
        if let Some(marker) = marker {
            // Even a malformed/null marker must hold generic cleanup.
            row.insert(marker, Bson::Null);
        }
        store.sandboxes.insert_one(row).await.unwrap();
    }
    retained
}

async fn reconcile(store: &MongoSandboxStore, homes: Arc<Homes>) {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    SandboxReconciler::new(
        Arc::new(store.clone()),
        Arc::new(adapter.with_home_volumes(homes)),
        Arc::new(SandboxPolicy::default()),
        "fixture-home-retention".into(),
    )
    .initialize(SystemTime::now())
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_reconciler_holds_marker_only_missing_changed_and_released_homes() {
    let (client, name, store) = database().await;
    let homes = Arc::new(Homes::default());
    for marker in [
        "channel_creation",
        "channel_provisioning",
        "channel_home_provenance",
        "channel_home_prepare_evidence",
        "channel_retirement",
        "channel_storage_retention",
    ] {
        for canonical in ["missing", "changed", "released"] {
            root(
                &store,
                &homes,
                &format!("sbx_{marker}_{canonical}"),
                canonical,
                Some(marker),
            )
            .await;
        }
    }
    let before = homes.inventory().await.unwrap();
    reconcile(&store, homes.clone()).await;
    assert!(
        homes.removed.lock().await.is_empty(),
        "retained home was physically removed"
    );
    assert_eq!(homes.inventory().await.unwrap(), before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_reconciler_holds_allocation_only_homes_without_canonical_ownership() {
    let (client, name, store) = database().await;
    let homes = Arc::new(Homes::default());
    let allocations = client
        .database(&name)
        .collection::<Document>("bridge_allocations");
    for evidence in ["admission", "placement", "unknown"] {
        for canonical in ["missing", "changed", "missing_root"] {
            let id = format!("sbx_{evidence}_{canonical}");
            root(&store, &homes, &id, canonical, None).await;
            let mut allocation = doc! {"_id":format!("allocation-{id}")};
            if evidence == "admission" {
                allocation.insert("sandbox_admission", doc! {"sandbox_id":&id});
            } else {
                allocation.insert(
                    "record",
                    doc! {"placement": {
                        "kind":if evidence == "placement" {"new_sandbox"} else {"unknown"},
                        "target":{"sandbox_id":&id},
                    }},
                );
            }
            allocations.insert_one(allocation).await.unwrap();
        }
    }
    let before = homes.inventory().await.unwrap();
    reconcile(&store, homes.clone()).await;
    assert!(
        homes.removed.lock().await.is_empty(),
        "allocation-retained home was removed"
    );
    assert_eq!(homes.inventory().await.unwrap(), before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_reconciler_keeps_ordinary_late_home_cleanup_and_matching_home_hold() {
    let (client, name, store) = database().await;
    let homes = Arc::new(Homes::default());
    for kind in ["reuse_parent", "existing_agent"] {
        let id = format!("sbx_ordinary_{kind}");
        root(&store, &homes, &id, "missing", None).await;
        client
            .database(&name)
            .collection::<Document>("bridge_allocations")
            .insert_one(
                doc! {"_id":format!("allocation-{kind}"),"creates_sandbox":false,
                "record":{"placement":{"kind":kind,"target":{"sandbox_id":id}}}},
            )
            .await
            .unwrap();
    }
    root(&store, &homes, "sbx_ordinary_orphan", "missing_root", None).await;
    let matching = root(&store, &homes, "sbx_ordinary_matching", "matching", None).await;
    reconcile(&store, homes.clone()).await;
    assert_eq!(homes.removed.lock().await.len(), 3);
    assert_eq!(
        homes.inventory().await.unwrap(),
        vec![matching.attachment().unwrap()]
    );
    client.database(&name).drop().await.unwrap();
}

#[path = "fair_sweep.rs"]
mod fair_sweep;

#[path = "quarantined_native_home.rs"]
mod quarantined_native_home;
