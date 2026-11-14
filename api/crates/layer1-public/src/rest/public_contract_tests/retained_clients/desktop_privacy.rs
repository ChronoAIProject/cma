//! Exact additive privacy policy introduced by #1963, without rewriting releases.
use super::super::BodyKind;
use super::Corpus;
use serde_json::json;

const REFUSALS: [(&str, &str, &str); 6] = [
    (
        "desktop-discovery-other_owner",
        "desktop_other_owner",
        "agt_contract",
    ),
    (
        "desktop-discovery-forbidden",
        "desktop_forbidden",
        "agt_contract",
    ),
    (
        "desktop-discovery-headless",
        "desktop_headless",
        "agt_contract",
    ),
    ("desktop-discovery-paused", "desktop_paused", "agt_contract"),
    (
        "desktop-discovery-isolated",
        "desktop_isolated",
        "agt_contract",
    ),
    ("desktop-discovery-missing", "desktop_owner", "agt_missing"),
];

pub(super) fn apply(release: &str, file: &str, corpus: &mut Corpus) -> usize {
    if !matches!(
        (release, file),
        (
            "93d4a199a3d36d47f5a89f9117d2bc735900bf4c",
            "previous-desktop.json"
        ) | (
            "4f03c35a80322673ec00df7a3b6b9399015f5a68",
            "current-desktop.json"
        )
    ) {
        return 0;
    }
    for (id, scenario, agent) in REFUSALS {
        let mut selected = corpus.cases.iter_mut().filter(|case| case.id == id);
        let case = selected.next().expect("declared retained refusal exists");
        assert!(selected.next().is_none(), "retained refusal is unique");
        assert_eq!(case.operation, "GET /api/v1/agents/{id}/desktop");
        assert_eq!(case.scenario, scenario);
        assert_eq!(case.request.method, "GET");
        assert_eq!(case.request.path, format!("/api/v1/agents/{agent}/desktop"));
        assert!(case.request.headers.is_empty());
        assert!(case.request.body.is_none());
        assert!(case.request.raw_body.is_none());
        assert!(case.request.raw_body_repeat.is_none());
        assert!(case.captures.is_empty());
        assert!(case.header_captures.is_empty());
        assert_eq!(case.response.status, 404);
        assert_eq!(case.response.body_kind, BodyKind::Json);
        assert!(!case.response.stream_prefix);
        assert_eq!(
            case.response.body,
            json!({
                "type": "https://spec.chrono-ai.fun/cma/1.0/problems/desktop_unavailable",
                "title": "desktop_unavailable", "status": 404,
                "detail": "desktop is unavailable for this Agent", "code": "desktop_unavailable"
            })
        );
        assert!(case.response.set_cookies.is_empty());
        assert_eq!(case.response.headers.len(), 1);
        assert_eq!(
            case.response.headers["content-type"],
            "application/problem+json"
        );
        // Never discard an actual response header or overwrite a frozen value.
        // Missing, changed, repeated and unrelated headers still fail replay.
        assert!(case
            .response
            .headers
            .insert("cache-control".into(), "no-store, private".into())
            .is_none());
    }
    REFUSALS.len()
}
