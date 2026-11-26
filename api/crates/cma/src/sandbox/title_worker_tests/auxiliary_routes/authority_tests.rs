use super::*;

#[tokio::test]
async fn mismatched_material_never_reaches_either_auxiliary_service() {
    for change in 0..7 {
        let f = Fixture::new(Capture::default(), None).await;
        f.turn(1, "Private task");
        {
            let mut material = f.credentials.0.lock().unwrap();
            let material = material.as_mut().unwrap();
            match change {
                0 => material.owner = SubjectId::new("other"),
                1 => material.sandbox_id = SandboxId("other".into()),
                2 => material.authority.generation += 1,
                3 => material.authority.provider_key_id = "other".into(),
                4 => material.authority.provider_state_version += 1,
                5 => material.authority.grant_fingerprint = "c".repeat(64),
                _ => material.value = SecretValue::new(" ".into()),
            }
        }
        let peer = Peer::start(Reply::Success).await;
        assert_eq!(
            title_worker(&f, peer.endpoint())
                .run_one(&f.id)
                .await
                .unwrap(),
            AgentTitleOutcome::Skipped
        );
        assert!(matches!(
            suggestion_worker(&f, peer.endpoint())
                .generate(&suggestion_work(&f).await)
                .await,
            Err(SuggestionFailure::Unavailable)
        ));
        assert!(
            peer.calls.lock().unwrap().is_empty(),
            "material case {change}"
        );
        assert!(f.service.title(&f.id).await.unwrap().is_none());
        assert_no_legacy_proxy(&f);
    }
}

#[tokio::test]
async fn expired_managed_and_unknown_expiry_supplied_custody_remain_refused() {
    for supplied in [false, true] {
        let mut sandbox = sandbox();
        let authority = sandbox.credential.current.as_mut().unwrap();
        if supplied {
            let key_sha256 = "d".repeat(64);
            sandbox.credential.source = SandboxCredentialSource::Provided {
                key_sha256: key_sha256.clone(),
            };
            authority.provider_key_id = format!("provided-{key_sha256}");
            authority.provider_state_version = 1;
            authority.expires_at.clear();
        } else {
            authority.expires_at = NOW.into();
        }
        assert!(sandbox.credential.validate());
        let f = Fixture::with_records(sandbox, agent(), Capture::default(), None).await;
        f.turn(1, "Private task");
        let peer = Peer::start(Reply::Success).await;
        assert_eq!(
            title_worker(&f, peer.endpoint())
                .run_one(&f.id)
                .await
                .unwrap(),
            AgentTitleOutcome::Skipped
        );
        assert!(matches!(
            suggestion_worker(&f, peer.endpoint())
                .generate(&suggestion_work(&f).await)
                .await,
            Err(SuggestionFailure::Unavailable)
        ));
        assert_eq!(f.credentials.1.load(Ordering::SeqCst), 0);
        assert!(peer.calls.lock().unwrap().is_empty());
        assert_no_legacy_proxy(&f);
    }
}

struct GatedEndpoint {
    inner: Arc<PlatformTitleEndpoint>,
    entered: tokio::sync::Notify,
    release: Semaphore,
}

#[async_trait::async_trait]
impl TitleRuntimeEndpoint for GatedEndpoint {
    async fn endpoint(&self, sandbox: &Sandbox) -> Option<TitleProxy> {
        let selected = self.inner.endpoint(sandbox).await;
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        selected
    }
}

#[tokio::test]
async fn persisted_authority_drift_during_route_resolution_prevents_both_sends() {
    for suggestion in [false, true] {
        let f = Fixture::new(Capture::default(), None).await;
        f.turn(1, "Private task");
        let peer = Peer::start(Reply::Success).await;
        let endpoint = Arc::new(GatedEndpoint {
            inner: peer.endpoint(),
            entered: tokio::sync::Notify::new(),
            release: Semaphore::new(0),
        });
        let title = title_worker(&f, endpoint.clone());
        let suggestions = suggestion_worker(&f, endpoint.clone());
        let work = suggestion_work(&f).await;
        let id = f.id.clone();
        let task = tokio::spawn(async move {
            if suggestion {
                assert!(matches!(
                    suggestions.generate(&work).await,
                    Err(SuggestionFailure::Unavailable)
                ));
            } else {
                assert_eq!(
                    title.run_one(&id).await,
                    Err(AgentTitleServiceError::GenerationUnavailable)
                );
            }
        });
        tokio::time::timeout(Duration::from_secs(5), endpoint.entered.notified())
            .await
            .unwrap();
        let mut updated = f
            .store
            .get_sandbox(&SandboxId("sbx_title".into()))
            .await
            .unwrap()
            .unwrap();
        let revision = updated.revision;
        updated
            .credential
            .current
            .as_mut()
            .unwrap()
            .provider_state_version += 1;
        updated.revision += 1;
        f.store.save_sandbox(revision, updated).await.unwrap();
        endpoint.release.add_permits(1);
        task.await.unwrap();
        assert!(peer.calls.lock().unwrap().is_empty());
        assert_no_legacy_proxy(&f);
    }
}
