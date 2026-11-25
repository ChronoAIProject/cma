//! Add the Agent-list cache policy while retaining exact historical responses.
use super::super::BodyKind;
use super::Corpus;

pub(super) fn apply(release: &str, file: &str, corpus: &mut Corpus) -> usize {
    if !matches!(
        (release, file),
        (
            "93d4a199a3d36d47f5a89f9117d2bc735900bf4c",
            "previous-agents.json"
        ) | (
            "4f03c35a80322673ec00df7a3b6b9399015f5a68",
            "current-agents.json"
        )
    ) {
        return 0;
    }
    for (id, scenario, query, status) in [
        ("agent-list", "agents_owner", "", 200),
        ("agent-invalid-query", "agents_owner", "?limit=0", 400),
        ("agent-forbidden", "agents_forbidden", "", 403),
        ("agent-unauthenticated", "agents_anonymous", "", 401),
    ] {
        let mut selected = corpus.cases.iter_mut().filter(|case| case.id == id);
        let case = selected.next().expect("retained Agent list exists");
        assert!(selected.next().is_none());
        assert_eq!(case.operation, "GET /api/v1/agents");
        assert_eq!(case.scenario, scenario);
        assert_eq!(case.request.method, "GET");
        assert_eq!(case.request.path, format!("/api/v1/agents{query}"));
        assert_eq!(case.response.status, status);
        assert_eq!(case.response.body_kind, BodyKind::Json);
        assert_eq!(case.response.headers.len(), 1);
        assert_eq!(
            case.response.headers["content-type"],
            if status == 200 {
                "application/json"
            } else {
                "application/problem+json"
            }
        );
        assert!(case
            .response
            .headers
            .insert("cache-control".into(), "private, no-store".into())
            .is_none());
    }
    4
}

#[tokio::test]
async fn retained_agent_list_privacy_preserves_frozen_fields_and_rejects_header_drift() {
    for (release, file) in [
        (
            "93d4a199a3d36d47f5a89f9117d2bc735900bf4c",
            "previous-agents.json",
        ),
        (
            "4f03c35a80322673ec00df7a3b6b9399015f5a68",
            "current-agents.json",
        ),
    ] {
        let bytes = std::fs::read(super::directory().join(file)).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut corpus: Corpus = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(apply("unknown-release", file, &mut corpus), 0);
        assert_eq!(serde_json::to_value(&corpus).unwrap(), original);
        assert_eq!(apply(release, file, &mut corpus), 4);
        for case in &mut corpus.cases {
            if case.operation == "GET /api/v1/agents" {
                assert_eq!(
                    case.response.headers.remove("cache-control").as_deref(),
                    Some("private, no-store")
                );
            }
        }
        assert_eq!(serde_json::to_value(&corpus).unwrap(), original);
        assert_eq!(apply(release, file, &mut corpus), 4);
        let case = corpus
            .cases
            .iter_mut()
            .find(|case| case.id == "agent-list")
            .unwrap();
        for cache in [None, Some("public")] {
            let mut fixture = super::super::scenario(&case.scenario).await;
            fixture.router = fixture.router.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                    let mut response = next.run(request).await;
                    match cache {
                        None => {
                            response.headers_mut().remove("cache-control");
                        }
                        Some(value) => {
                            response
                                .headers_mut()
                                .insert("cache-control", value.parse().unwrap());
                        }
                    }
                    response
                },
            ));
            use futures_util::FutureExt;
            let mut symbols = super::super::Symbols::default();
            let result = std::panic::AssertUnwindSafe(super::super::replay::run(
                &fixture,
                &mut symbols,
                case,
                false,
            ))
            .catch_unwind()
            .await;
            assert!(
                result.is_err(),
                "changed cache policy must fail exact replay"
            );
        }
    }
}
