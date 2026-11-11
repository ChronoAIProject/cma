use super::*;
use layer2_core::sandbox::{
    colors::{SandboxColorService, SystemColorRandom},
    SandboxDetailPhase,
};
use layer2_core::{PublicIdentity, UserDirectory, UserDirectoryStore};
use std::time::Instant;

struct NoOrganization;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganization {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }

    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_snapshot_local_phase_measurement() {
    let (admin, name) = database("sandbox_snapshot_measure").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let root = store
        .resource_access_snapshot(&target)
        .await
        .unwrap()
        .lineage
        .sandbox;
    let colors_store = Arc::new(
        crate::store::sandbox_colors::MongoSandboxColorStore::open(&client.database(&name))
            .await
            .unwrap(),
    );
    let names_store = Arc::new(
        crate::store::directory::MongoUserDirectoryStore::open(&client, &name)
            .await
            .unwrap(),
    );
    names_store
        .observe(&PublicIdentity {
            subject: root.owner.clone(),
            display_name: Some("Fixture".into()),
            avatar_url: None,
        })
        .await
        .unwrap();
    let names = UserDirectory::new(names_store);
    let caller = Caller {
        subject: root.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::INFO)
            .finish(),
    );
    let mut expected = None;
    for enabled in [false, true] {
        let store = Arc::new(store.clone().with_sandbox_snapshot_reads(enabled));
        let colors = SandboxColorService::new(
            colors_store.clone(),
            store.clone(),
            Arc::new(SystemColorRandom::new()),
        );
        let service = HierarchyReadService::new(store.clone())
            .with_detail_timings(true)
            .with_resource_access(Arc::new(ResourceAccessService::new(
                store,
                Arc::new(NoOrganization),
            )));
        let run = || async {
            let result = service.sandbox(&caller, &root.id).await.unwrap();
            let timer = service.detail_phase(SandboxDetailPhase::Colors);
            assert_eq!(
                colors
                    .colors(&caller.subject, &[&result.sandbox])
                    .await
                    .len(),
                1
            );
            timer.complete();
            assert_eq!(
                service
                    .detail_phase(SandboxDetailPhase::Names)
                    .result(names.display_names(std::slice::from_ref(&caller.subject)))
                    .await
                    .unwrap()
                    .len(),
                1
            );
            result
        };
        run().await;
        trace.commands.lock().unwrap().clear();
        trace.checkouts.store(0, Ordering::SeqCst);
        let mut times = Vec::new();
        println!("MEASURE snapshot={enabled} samples=40");
        for _ in 0..40 {
            let start = Instant::now();
            let result = run().await;
            times.push(start.elapsed().as_micros());
            assert_eq!(expected.get_or_insert(result.clone()), &result);
        }
        times.sort_unstable();
        println!(
            "MEASURE snapshot={enabled} p50_us={} p95_us={} max_us={} checkouts={}",
            times[19],
            times[37],
            times[39],
            trace.checkouts.load(Ordering::SeqCst)
        );
    }
    admin.database(&name).drop().await.unwrap();
}
