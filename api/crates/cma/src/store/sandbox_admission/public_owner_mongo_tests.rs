use std::num::NonZeroU16;
use std::sync::Arc;

use layer2_core::{
    profiles::InMemoryProfileStore,
    sandbox::{
        store::SandboxLimitClass, FixedSandboxCredentialPrerequisite, SandboxPolicy, SandboxService,
    },
    Authorization, Caller, Capability, PlatformSettings, PlatformSettingsStore,
};

use super::*;
use crate::auth::registration::RegistrationCapacity;
use crate::store::{
    registration_capacity::MongoRegistrationCapacityStore, MongoPlatformSettingsStore,
};

async fn register(client: &Client, name: &str, subject: &str) {
    let registration = MongoRegistrationCapacityStore::open(client, name, true)
        .await
        .unwrap();
    assert!(registration.admit(&SubjectId::new(subject)).await.unwrap());
}

fn create(owner: &str, index: usize, origin: SandboxOrigin) -> SandboxAdmission {
    let mut candidate = sandbox(&format!("sbx_public_{owner}_{index}"), "unused");
    candidate.owner = SubjectId::new(owner);
    candidate.origin = origin;
    SandboxAdmission::new(
        candidate,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        NOW.into(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn concurrent_public_owners_share_two_homes_and_exact_origin_replays() {
    let (client, name) = database("public_owner_limit").await;
    let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let primary = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_public_owner_sandbox_limit(NonZeroU16::new(2));
    let secondary = MongoSandboxStore::open(&peer, &name)
        .await
        .unwrap()
        .with_public_owner_sandbox_limit(NonZeroU16::new(2));
    register(&client, &name, "public").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(6));
    let results = futures_util::future::join_all((0..6).map(|index| {
        let store = if index % 2 == 0 { &primary } else { &secondary };
        let barrier = barrier.clone();
        async move {
            let request = create(
                "public",
                index,
                SandboxOrigin::Lifecycle {
                    operation_id: format!("operation-{index}"),
                },
            );
            barrier.wait().await;
            let result = store.admit_sandbox(request.clone()).await;
            (request, result)
        }
    }))
    .await;
    let mut accepted = Vec::new();
    for (request, result) in results {
        match result {
            Ok(SandboxAdmissionResult::Created(sandbox)) => accepted.push((request, sandbox)),
            Err(SandboxStoreError::LimitReached {
                limit: 2, live: 2, ..
            }) => {}
            other => panic!("unexpected public admission: {other:?}"),
        }
    }
    assert_eq!(accepted.len(), 2);
    assert_eq!(primary.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        primary
            .sandbox_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let capacity = secondary
        .owner_capacity(&SubjectId::new("public"))
        .await
        .unwrap();
    assert_eq!(capacity.public_owner_limit, Some(2));
    assert_eq!(capacity.counts, vec![(SandboxStatus::Provisioning, 2)]);
    let service = SandboxService::new(
        Arc::new(secondary.clone()),
        Arc::new(InMemoryProfileStore::default()),
        SandboxPolicy::default(),
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let caller = Caller::new(
        SubjectId::new("public"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let capacity = service.capacity(&caller).await.unwrap();
    assert_eq!(
        (capacity.limit, capacity.live, capacity.available),
        (2, 2, 0)
    );

    let restarted = MongoSandboxStore::open(&peer, &name)
        .await
        .unwrap()
        .with_public_owner_sandbox_limit(NonZeroU16::new(2));
    for (request, sandbox) in accepted {
        assert_eq!(
            restarted.admit_sandbox(request).await.unwrap(),
            SandboxAdmissionResult::Existing(sandbox)
        );
    }
    for (index, origin) in [
        SandboxOrigin::Manual,
        SandboxOrigin::Trigger {
            launch_id: "new-home".into(),
        },
    ]
    .into_iter()
    .enumerate()
    {
        assert!(matches!(
            secondary
                .admit_sandbox(create("public", index + 6, origin))
                .await,
            Err(SandboxStoreError::LimitReached {
                limit: 2,
                live: 2,
                ..
            })
        ));
    }
    assert_eq!(primary.sandboxes.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        primary
            .sandbox_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn public_owner_override_preserves_unrelated_quotas_and_respects_live_settings() {
    let (client, name) = database("public_owner_override").await;
    let off = MongoSandboxStore::open(&client, &name).await.unwrap();
    let on = off
        .clone()
        .with_public_owner_sandbox_limit(NonZeroU16::new(3));
    register(&client, &name, "public").await;
    register(&client, &name, "visitor").await;
    for index in 0..3 {
        on.admit_sandbox(create("public", index, SandboxOrigin::Manual))
            .await
            .unwrap();
    }
    let extra = create("public", 3, SandboxOrigin::Manual);
    assert!(matches!(
        on.admit_sandbox(extra.clone()).await,
        Err(SandboxStoreError::LimitReached {
            limit: 3,
            live: 3,
            ..
        })
    ));
    off.admit_sandbox(extra).await.unwrap();
    assert_eq!(
        off.owner_capacity(&SubjectId::new("public"))
            .await
            .unwrap()
            .public_owner_limit,
        None
    );
    assert_eq!(
        on.owner_capacity(&SubjectId::new("public"))
            .await
            .unwrap()
            .public_owner_limit,
        Some(3)
    );

    for index in 0..4 {
        on.admit_sandbox(create("direct", index, SandboxOrigin::Manual))
            .await
            .unwrap();
    }
    assert_eq!(
        on.owner_capacity(&SubjectId::new("direct"))
            .await
            .unwrap()
            .public_owner_limit,
        None
    );
    let visitor = |index| {
        let mut request = create(
            "visitor",
            index,
            SandboxOrigin::Trigger {
                launch_id: format!("visit-{index}"),
            },
        );
        request.limit_class = SandboxLimitClass::Visitor;
        request
    };
    on.admit_sandbox(visitor(0)).await.unwrap();
    assert!(matches!(
        on.admit_sandbox(visitor(1)).await,
        Err(SandboxStoreError::LimitReached {
            limit: 1,
            live: 1,
            ..
        })
    ));

    let operator = MongoPlatformSettingsStore::open(&client, &name)
        .await
        .unwrap();
    let mut settings = PlatformSettings::default();
    settings.limits.max_sandboxes_per_user = 1;
    operator.replace(0, &settings).await.unwrap();
    assert!(matches!(
        on.admit_sandbox(create("public", 4, SandboxOrigin::Manual))
            .await,
        Err(SandboxStoreError::LimitReached {
            limit: 1,
            live: 4,
            ..
        })
    ));
    client.database(&name).drop().await.unwrap();
}
