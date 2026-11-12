use super::*;
use crate::retirement_archive::{ExclusionReceipt, Result, Verification};
use layer3_adapter::permanent_custody::{CustodyPolicy, ObjectReceipt};
use std::sync::atomic::{AtomicUsize, Ordering};

struct DelayedPolicy<'a> {
    inner: &'a Custody,
    expires: u64,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl PermanentCustody for DelayedPolicy<'_> {
    async fn policy(&self) -> Result<CustodyPolicy> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            while layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap()
                <= self.expires
            {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
        self.inner.policy().await
    }
    async fn retain(&self, operation: &str, data: Vec<u8>) -> Result<ObjectReceipt> {
        self.inner.retain(operation, data).await
    }
    async fn read(&self, receipt: &ObjectReceipt) -> Result<Vec<u8>> {
        self.inner.read(receipt).await
    }
}
async fn prepare(
    temp: &Temp,
    store: &ArchiveStore,
    request: &Request,
    bytes: &[u8],
    custody: &Custody,
) -> (
    RetirementArchiveReceipt,
    crate::retirement_archive::Manifest,
) {
    store.admit(request, bytes).await.unwrap();
    let manifest_receipt = export::publish(store, request, bytes, custody, None)
        .await
        .unwrap();
    let manifest = verify::load(custody, &manifest_receipt, request, bytes)
        .await
        .unwrap();
    let verification: Verification = verify::restore(
        custody,
        &manifest_receipt,
        request,
        bytes,
        &temp.0.join("restore"),
    )
    .await
    .unwrap();
    let restored = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&verification).unwrap(),
        )
        .await
        .unwrap();
    store
        .retain_restore(request, bytes, &restored)
        .await
        .unwrap();
    (
        RetirementArchiveReceipt {
            manifest_sha256: manifest_receipt.sha256,
            custody_sha256: verification.custody_sha256,
            verification_sha256: restored.sha256,
        },
        manifest,
    )
}
#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn exclusion_expiring_during_final_census_cannot_release_the_hold() {
    let (temp, store, mut request, _) = fixture().await;
    let mut exclusion: ExclusionReceipt =
        serde_json::from_slice(&std::fs::read(&request.exclusion_receipt).unwrap()).unwrap();
    let expiry = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap() + 10;
    exclusion.expires_at =
        layer2_core::rfc3339_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(expiry));
    let exclusion_bytes = serde_json::to_vec(&exclusion).unwrap();
    temp.write("exclusion.json", &exclusion_bytes);
    request.exclusion_sha256 = digest(&exclusion_bytes);
    let bytes = serde_json::to_vec(&request).unwrap();
    let custody = Custody::default();
    let (receipt, manifest) = prepare(&temp, &store, &request, &bytes, &custody).await;
    let slow = DelayedPolicy {
        inner: &custody,
        expires: expiry,
        calls: 0.into(),
    };
    assert_eq!(
        export::build(&store, &request, &bytes, &slow, None)
            .await
            .unwrap(),
        manifest
    );
    assert!(export::exclusion(&request).is_err());
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    assert!(store.complete(&request, &bytes, receipt).await.is_err());
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
    assert!(read_sandbox(&held).unwrap().retirement_archive_pending());
    store.database.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn changed_or_unreadable_final_authority_preserves_hold_and_committed_replay() {
    let (temp, store, request, bytes) = fixture().await;
    let custody = Custody::default();
    let (receipt, _) = prepare(&temp, &store, &request, &bytes, &custody).await;
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let authority = std::fs::read(&request.exclusion_receipt).unwrap();
    temp.write("exclusion.json", b"changed authority");
    assert!(store
        .complete(&request, &bytes, receipt.clone())
        .await
        .is_err());
    std::fs::remove_file(&request.exclusion_receipt).unwrap();
    assert!(store
        .complete(&request, &bytes, receipt.clone())
        .await
        .is_err());
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
    temp.write("exclusion.json", &authority);
    let (archive, observed) = commit_support::begin(&store).await;
    let result = archive.complete(&request, &bytes, receipt.clone()).await;
    commit_support::finish(&store, observed).await;
    result.unwrap();
    let committed = store.inspect(&request.sandbox_id).await.unwrap();
    assert!(!read_sandbox(&committed)
        .unwrap()
        .retirement_archive_pending());
    // Reconciliation acknowledges the existing release, even with no local
    // authority file. It must not reacquire a lease or write a second usage event.
    std::fs::remove_file(&request.exclusion_receipt).unwrap();
    store
        .complete(&request, &bytes, receipt.clone())
        .await
        .unwrap();
    let mut wrong = receipt;
    wrong.custody_sha256 = "0".repeat(64);
    assert!(store.complete(&request, &bytes, wrong).await.is_err());
    assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), committed);
    store.database.drop().await.unwrap();
}
