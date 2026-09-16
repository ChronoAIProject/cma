use super::fixture::Fixture;
use crate::sandbox::channel_provisioning::{
    home_provenance::ChannelHomeProvenance,
    state::{ChannelProvisioningPhase, ChannelProvisioningSettings, ChannelProvisioningState},
};
use layer2_core::{
    bridge::allocation::AllocationError,
    resource_access::{ResourceActor, ResourceOwner},
    SubjectId,
};
use layer3_adapter::{HomeVolumeScope, HomeVolumeStore};
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

#[path = "home_provenance_fixture.rs"]
mod support;
use support::*;

#[path = "home_prepare_evidence_tests.rs"]
mod prepare_debt;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_provenance_commits_before_prepare_and_recovers_lost_exact_response() {
    let (mut f, homes) = fixture(true).await;
    f.step().await.unwrap();
    let reserved = provenance(&row(&f).await).unwrap();
    let debt = prepare_evidence(&row(&f).await).unwrap();
    assert!(reserved.first_receipt.is_none());
    assert!(homes.requests.lock().await.is_empty());
    assert_eq!(reserved.allocation_id, f.record.id);
    assert_eq!(reserved.owner, f.record.owner);
    assert!(matches!(f.step().await, Err(AllocationError::Unavailable)));
    assert_eq!(provenance(&row(&f).await), Some(reserved.clone()));
    assert_eq!(prepare_evidence(&row(&f).await), Some(debt.clone()));
    assert_eq!(homes.inventory().await.unwrap().len(), 1);
    f.release_current_claim().await;
    reopen(&mut f, homes.clone());
    f.step().await.unwrap();
    let bound = provenance(&row(&f).await).unwrap();
    assert_eq!(
        bound,
        reserved
            .bind(homes.inventory().await.unwrap()[0].receipt.clone())
            .unwrap()
    );
    assert_eq!(
        *homes.requests.lock().await,
        vec![reserved.request.clone(); 2]
    );
    let child = f.children().await;
    assert_eq!(child.home_provenance, Some(bound));
    assert_eq!(child.home_prepare_evidence, Some(debt));
    assert_eq!(homes.inventory().await.unwrap().len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_provenance_refuses_scope_and_bound_receipt_replacement_before_effects() {
    let (mut f, homes) = fixture(false).await;
    f.step().await.unwrap();
    let original_scope = homes.allocation_scope().unwrap();
    *homes.scope.lock().unwrap() =
        HomeVolumeScope::from_authority("in-memory-fixture", &["replacement-scope"]).unwrap();
    assert!(matches!(
        f.step().await,
        Err(AllocationError::IdentityMismatch)
    ));
    assert!(homes.requests.lock().await.is_empty());
    *homes.scope.lock().unwrap() = original_scope;
    f.release_current_claim().await;
    f.step().await.unwrap();
    let original = row(&f).await;
    let original_provenance = provenance(&original).unwrap();
    let mut replacement = crate::store::sandboxes::read_sandbox(&original).unwrap();
    replacement
        .lifecycle
        .home
        .as_mut()
        .unwrap()
        .receipt
        .as_mut()
        .unwrap()
        .storage_generation = "replacement-generation".into();
    replacement.revision += 1;
    let fields = crate::store::sandboxes::sandbox_replacement_fields(&replacement).unwrap();
    let rows = f
        .client
        .database(&f.database)
        .collection::<Document>("sandboxes");
    rows.update_one(doc! {"_id":replacement.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    assert!(matches!(
        f.step().await,
        Err(AllocationError::IdentityMismatch)
    ));
    assert_eq!(provenance(&row(&f).await), Some(original_provenance));
    assert_eq!(homes.requests.lock().await.len(), 1);
    assert_eq!(
        homes.inventory().await.unwrap()[0]
            .receipt
            .storage_generation,
        "original-generation"
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_provenance_refuses_changed_original_allocation_owner_approver_and_request() {
    let (mut f, homes) = fixture(false).await;
    f.step().await.unwrap();
    let original = provenance(&row(&f).await).unwrap();
    for field in ["allocation", "owner", "approver", "created_at", "request"] {
        let mut changed = original.clone();
        match field {
            "allocation" => changed.allocation_id.push_str("-replacement"),
            "owner" => {
                changed.owner = ResourceOwner::Person {
                    subject: SubjectId("replacement-owner".into()),
                }
            }
            "approver" => {
                changed.creation_approver = ResourceActor::Human {
                    subject: SubjectId("replacement-approver".into()),
                }
            }
            "created_at" => changed.sandbox_created_at = "2001-01-01T00:00:00Z".into(),
            "request" => changed.request.volume_id.push_str("-replacement"),
            _ => unreachable!(),
        }
        put_provenance(&f, Some(&changed)).await;
        assert!(
            matches!(f.step().await, Err(AllocationError::IdentityMismatch)),
            "{field}"
        );
        assert!(homes.requests.lock().await.is_empty(), "{field}");
        put_provenance(&f, Some(&original)).await;
        f.release_current_claim().await;
    }
    f.step().await.unwrap();
    assert_eq!(homes.requests.lock().await.len(), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_provenance_keeps_legacy_admitted_provisioning_without_backfilling_ownership()
{
    let (mut f, homes) = fixture(false).await;
    f.step().await.unwrap();
    let admitted = crate::store::sandboxes::read_sandbox(&row(&f).await).unwrap();
    put_provenance(&f, None).await;
    f.client
        .database(&f.database)
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":admitted.id.as_str()},
            doc! {"$unset":{"channel_home_prepare_evidence":""}},
        )
        .await
        .unwrap();
    f.step().await.unwrap();
    let child = f.children().await;
    assert!(child.home_provenance.is_none());
    assert!(child.home_prepare_evidence.is_none());
    assert!(row(&f).await.get("channel_home_provenance").is_none());
    assert_eq!(
        child.sandbox.lifecycle.home.as_ref().unwrap().request,
        admitted.lifecycle.home.as_ref().unwrap().request
    );
    assert_eq!(homes.requests.lock().await.len(), 1);
    f.close().await;
}

// Exact pre-provenance state fields. deny_unknown_fields proves the new evidence
// is not inserted into the serialized state older provisioning workers decode.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProvisioningState {
    schema_version: u16,
    allocation_id: String,
    phase: ChannelProvisioningPhase,
    revision: u64,
    last_receipt_digest: Option<String>,
    settings: ChannelProvisioningSettings,
    mint_credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mint_intent: Option<crate::auth::keys::bridge::mint_intent::BridgeSandboxMintIntent>,
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_provenance_old_reader_preserves_sidecar_and_incomplete_evidence() {
    let (mut f, homes) = fixture(false).await;
    f.step().await.unwrap();
    let document = row(&f).await;
    let original = provenance(&document).unwrap();
    let debt = prepare_evidence(&document).unwrap();
    let state = document.get("channel_provisioning").unwrap().clone();
    let legacy: LegacyProvisioningState =
        mongodb::bson::deserialize_from_bson(state.clone()).unwrap();
    assert_eq!(
        crate::store::sandbox_mongo::record_bson(&legacy).unwrap(),
        state
    );
    let mut sandbox = crate::store::sandboxes::read_sandbox(&document).unwrap();
    let home = sandbox.lifecycle.home.as_mut().unwrap();
    let receipt = homes.prepare(&home.request).await.unwrap();
    home.bind(receipt).unwrap();
    // Model the old worker's existing Home save followed by Runtime transition:
    // canonical state advances via $set, with the unknown sidecar untouched.
    let current: ChannelProvisioningState = mongodb::bson::deserialize_from_bson(state).unwrap();
    let next = current
        .next(ChannelProvisioningPhase::Home, &sandbox.lifecycle.home)
        .unwrap()
        .next(ChannelProvisioningPhase::Runtime, &sandbox.lifecycle.home)
        .unwrap();
    sandbox.revision += 2;
    let mut fields = crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap();
    fields.insert(
        "channel_provisioning",
        crate::store::sandbox_mongo::record_bson(&next).unwrap(),
    );
    f.client
        .database(&f.database)
        .collection::<Document>("sandboxes")
        .update_one(doc! {"_id":sandbox.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    assert_eq!(provenance(&row(&f).await), Some(original.clone()));
    let child = f.children().await;
    assert_eq!(child.home_provenance, Some(original));
    assert!(child.home_provenance.unwrap().first_receipt.is_none());
    assert_eq!(child.home_prepare_evidence, Some(debt));
    assert_eq!(homes.requests.lock().await.len(), 1);
    f.close().await;
}

async fn put_provenance(f: &Fixture, value: Option<&ChannelHomeProvenance>) {
    let update = match value {
        Some(value) => {
            doc! {"$set":{"channel_home_provenance":crate::store::sandbox_mongo::record_bson(value).unwrap()}}
        }
        None => doc! {"$unset":{"channel_home_provenance":""}},
    };
    f.client
        .database(&f.database)
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":f.record.placement.target().sandbox_id.as_str()},
            update,
        )
        .await
        .unwrap();
}
