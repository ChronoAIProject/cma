use super::*;
use layer2_core::profiles::{
    content_digest, EnvironmentVariable, ProfileCapacityBounds, SandboxProfile, SandboxProfileStore,
};

const PROFILE: &str = "sbp_0123456789abcdef0123456789abcdef";
const AGENT_PATH: &str = "/api/v1/agents/agt_pin";
const SYNTHETIC_VALUE: &str = "profile-only fixture value";

#[tokio::test]
async fn agent_detail_agrees_with_the_public_revision_route_for_the_pinned_profile() {
    use ProfileVisibility::{Private, Public};
    use StatusCode as Status;

    for (case, visibility, profile_owner, internal, retired, pin, mismatch, status, readable) in [
        (
            "public",
            Public,
            OTHER,
            false,
            false,
            1,
            false,
            Status::OK,
            true,
        ),
        (
            "private owner",
            Private,
            OWNER,
            false,
            false,
            1,
            false,
            Status::OK,
            true,
        ),
        (
            "private non-owner",
            Private,
            OTHER,
            false,
            false,
            1,
            false,
            Status::NOT_FOUND,
            false,
        ),
        (
            "internal owner",
            Private,
            OWNER,
            true,
            false,
            1,
            false,
            Status::NOT_FOUND,
            false,
        ),
        (
            "retired owner",
            Public,
            OWNER,
            false,
            true,
            1,
            false,
            Status::OK,
            true,
        ),
        (
            "retired non-owner",
            Public,
            OTHER,
            false,
            true,
            1,
            false,
            Status::NOT_FOUND,
            false,
        ),
        (
            "missing revision",
            Public,
            OTHER,
            false,
            false,
            3,
            false,
            Status::NOT_FOUND,
            false,
        ),
        (
            "different digest",
            Public,
            OTHER,
            false,
            false,
            1,
            true,
            Status::OK,
            false,
        ),
    ] {
        let fixture = fixture().await;
        let profiles = Arc::new(InMemoryProfileStore::default());
        let mut parent = fixture
            .store
            .get_sandbox(&fixture.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let mut draft = parent.profile.content.clone();
        draft.environment_variables.push(EnvironmentVariable {
            name: "PUBLIC_TEST_SETTING".into(),
            value: SYNTHETIC_VALUE.into(),
        });
        let mut profile = SandboxProfile {
            internal,
            availability: Default::default(),
            id: SandboxProfileId(PROFILE.into()),
            owner: SubjectId::new(profile_owner),
            name: "Pinned profile".into(),
            description: "Profile used by the route fixture".into(),
            visibility,
            uses_count: 0,
            forked_from: None,
            draft,
            draft_version: 1,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            retired_at: None,
        };
        let digest = content_digest(&profile.draft).unwrap();
        profiles
            .create_sandbox_profile(profile.clone())
            .await
            .unwrap();
        let bounds = ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 4_000,
            min_memory_mib: 256,
            max_memory_mib: 8_192,
        };
        let scanner: [String; 0] = [];
        profiles
            .publish_sandbox_profile(&profile.id, 1, NOW.into(), bounds, &scanner)
            .await
            .unwrap();

        if case == "public" {
            profile.draft_version += 1;
            profile.draft.cpu_millis = 2000;
            profiles
                .save_sandbox_draft(1, profile.clone())
                .await
                .unwrap();
            let latest = profiles
                .publish_sandbox_profile(&profile.id, 2, NOW.into(), bounds, &scanner)
                .await
                .unwrap();
            assert_ne!(latest.digest, digest);
        }
        if retired {
            profiles
                .retire_sandbox_profile(&profile.id, &profile.owner, NOW.into())
                .await
                .unwrap();
        }

        parent.profile.id = profile.id;
        parent.profile.revision = pin;
        parent.profile.digest = if mismatch {
            "b".repeat(64)
        } else {
            digest.clone()
        };
        let expected_pin = json!({"id": PROFILE, "revision": pin, "digest": parent.profile.digest});
        let previous = parent.revision;
        parent.revision += 1;
        fixture.store.save_sandbox(previous, parent).await.unwrap();
        insert_agent(
            &fixture.store,
            seeded_agent("agt_pin", AgentStatus::Ready, 2, None),
            vec![],
        )
        .await;

        let (legacy_status, legacy) = json_response(&fixture.router, get(AGENT_PATH, true)).await;
        assert_eq!(legacy_status, Status::OK);
        assert_eq!(legacy["sandbox"]["profile"], expected_pin);

        let router = agent_router(
            fixture
                .state
                .with_sandbox_profile_readability(profiles.clone()),
        )
        .merge(crate::rest::profiles_tests::fixture_with_profile_store(
            profiles,
        ));
        let revision_path = format!("/api/v1/sandbox-profiles/{PROFILE}/revisions/{pin}");
        let (revision_status, revision) = json_response(&router, get(&revision_path, true)).await;
        assert_eq!(revision_status, status, "{case}");
        if revision_status == Status::OK {
            assert_eq!(revision["digest"], digest, "{case}");
        }
        let response = router.clone().oneshot(get(AGENT_PATH, true)).await.unwrap();
        assert_eq!(response.status(), Status::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let mut detail: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(!detail.to_string().contains(SYNTHETIC_VALUE), "{case}");
        assert_eq!(
            detail["sandbox"]["profile"]
                .as_object_mut()
                .unwrap()
                .remove("readable"),
            Some(json!(readable)),
            "{case}"
        );
        assert_eq!(detail, legacy, "{case}");

        if case == "public" {
            for path in [AGENT_PATH, &revision_path] {
                assert_eq!(
                    json_response(&router, get(path, false)).await.0,
                    Status::FORBIDDEN
                );
                assert_eq!(json_response(&router, get(path, true)).await.0, Status::OK);
            }
        }
    }
}

fn get(path: &str, use_sandboxes: bool) -> Request<Body> {
    let mut request = request(Method::GET, path, OWNER, None, None);
    if !use_sandboxes {
        request.extensions_mut().insert(Authorization::new([]));
    }
    request
}
