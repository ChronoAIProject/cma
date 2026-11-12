use super::*;

#[tokio::test]
async fn owner_transcript_pages_complete_transcript_and_usage_recheck_buffered_authority() {
    for projection in ["page", "complete", "usage"] {
        let (store, authority, access, selector) =
            setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let policy_store = Arc::new(PolicyStore {
            inner: store.clone(),
            replacement: None,
            calls: AtomicUsize::new(1),
            gate: Some((entered.clone(), resume.clone())),
        });
        let reads = service(store.clone()).with_resource_access(Arc::new(
            ResourceAccessService::new(policy_store, authority),
        ));
        let id = agent.id.clone();
        let task = tokio::spawn(async move {
            match projection {
                "page" => reads
                    .transcript_page(
                        &context("owner").caller,
                        &id,
                        &crate::sandbox::AgentTranscriptPageRequest {
                            turn: None,
                            after: None,
                            through: None,
                            limit: 2,
                        },
                    )
                    .await
                    .map(|_| ()),
                "complete" => reads
                    .transcript_bounded(&context("owner").caller, &id)
                    .await
                    .map(|_| ()),
                _ => reads
                    .usage_bounded(&context("owner").caller, &id)
                    .await
                    .map(|_| ()),
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        access
            .change_share(
                &context("owner"),
                Some("request-credential"),
                create(&selector, "policy-change"),
            )
            .await
            .unwrap();
        resume.notify_one();
        assert!(task.await.unwrap().is_err(), "{projection}");
    }
}
