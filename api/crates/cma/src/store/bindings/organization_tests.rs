use super::admission_tests::sandbox;
use super::mongo_tests::{binding, fixture, NOW};
use super::*;
use layer2_core::resource_access::{ResourceAccessStore, ResourceOwner, ResourceTarget};
use layer2_core::sandbox::binding::{BindingAdmissionFence, BindingWork};
use layer2_core::sandbox::store::*;

async fn admit<S: BindingStore + SandboxAdmissionStore + ResourceAccessStore>(
    store: &S,
) -> ProviderBinding {
    let mut value = binding();
    value.profile.organization_owned = true;
    value
        .profile
        .environment
        .as_mut()
        .unwrap()
        .always_keep_alive = true;
    store.save_binding(None, value.clone()).await.unwrap();
    value.work = BindingWork::ProvisionSandbox;
    value.advance_revision(NOW.into()).unwrap();
    store.save_binding(Some(1), value.clone()).await.unwrap();
    let parent = sandbox(&value);
    let mut request = SandboxAdmission::new(
        parent.clone(),
        SandboxAdmissionName::Exact(parent.name.clone()),
        3,
        NOW.into(),
    )
    .unwrap();
    request.binding = Some(BindingAdmissionFence::environment(&value));
    store.admit_sandbox(request).await.unwrap();
    verify(store, &value).await;
    value
}
async fn verify<S: ResourceAccessStore>(store: &S, value: &ProviderBinding) {
    let snapshot = store
        .resource_access_snapshot(&ResourceTarget::Sandbox {
            sandbox_id: value.sandbox_id.clone(),
        })
        .await
        .unwrap();
    let policy = snapshot.policy.unwrap();
    assert_eq!(
        policy.owner,
        ResourceOwner::Organization {
            subject: value.owner.clone()
        }
    );
    assert_eq!(
        policy.organization_evidence.unwrap().organization,
        value.owner
    );
}
#[tokio::test]
async fn memory_binding_admission_persists_explicit_organization_ownership() {
    admit(&layer2_core::sandbox::InMemorySandboxStore::default()).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_organization_binding_policy_and_keep_alive_survive_reopen() {
    let (client, database, store) = fixture().await;
    let value = admit(&store).await;
    let reopened = MongoSandboxStore::open(&client, &database).await.unwrap();
    verify(&reopened, &value).await;
    let parent = reopened
        .get_sandbox(&value.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    super::super::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .bindings
            .keep_alive(&parent, &mut session)
            .await
            .unwrap(),
        Some(true)
    );
    let mut wrong = parent.clone();
    wrong.owner = layer2_core::SubjectId::new("another-owner");
    assert_eq!(
        reopened.bindings.keep_alive(&wrong, &mut session).await,
        Err(SandboxStoreError::Conflict)
    );
    session.abort_transaction().await.unwrap();
    client.database(&database).drop().await.unwrap();
}
