use super::*;
use layer2_core::sandbox::store::SandboxStore;

#[tokio::test]
async fn visitor_policy_limits_two_services_and_replays_without_new_profile_use() {
    for limit in [1_u16, 2] {
        let fixture = Fixture::new().await;
        fixture
            .settings
            .0
            .lock()
            .unwrap()
            .settings
            .limits
            .visitor_max_sandboxes = u32::from(limit);
        let visitor = Caller::new(
            SubjectId::new("visitor"),
            Authorization::new([Capability::LaunchTriggers]),
        );
        let services = [fixture.service(), fixture.service()];
        let mut tasks = Vec::new();
        for index in 0..6 {
            let service = services[index % 2].clone();
            let visitor = visitor.clone();
            let mut request = fixture.request();
            request.origin = SandboxOrigin::Trigger {
                launch_id: format!("launch-{index}"),
            };
            tasks.push(tokio::spawn(async move {
                let result = service
                    .create_for_launch(&visitor, Capability::LaunchTriggers, request.clone())
                    .await;
                (request, result)
            }));
        }
        let mut accepted = Vec::new();
        let mut refused = 0;
        for task in tasks {
            let (request, result) = task.await.unwrap();
            match result {
                Ok(sandbox) => accepted.push((request, sandbox)),
                Err(SandboxServiceError::SandboxLimitReached {
                    limit: actual,
                    live,
                    ..
                }) => {
                    assert_eq!((actual, live), (limit, limit));
                    refused += 1;
                }
                other => panic!("unexpected visitor result: {other:?}"),
            }
        }
        assert_eq!(accepted.len(), usize::from(limit));
        assert_eq!(refused, 6 - limit);
        assert_eq!(fixture.uses().await, u64::from(limit));
        for (request, sandbox) in accepted {
            assert_eq!(
                services[1]
                    .create_for_launch(&visitor, Capability::LaunchTriggers, request)
                    .await
                    .unwrap(),
                sandbox
            );
        }
        assert_eq!(fixture.uses().await, u64::from(limit));
        assert_eq!(
            fixture
                .store
                .list_sandboxes(&visitor.subject, &PageRequest::default())
                .await
                .unwrap()
                .items
                .len(),
            usize::from(limit)
        );
        assert_eq!(
            services[0]
                .create(&visitor, fixture.request())
                .await
                .unwrap_err(),
            SandboxServiceError::Forbidden
        );
    }
}
