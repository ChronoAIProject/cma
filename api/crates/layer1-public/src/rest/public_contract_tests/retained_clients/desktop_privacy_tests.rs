use std::panic::AssertUnwindSafe;

use axum::{body::Body, http::StatusCode, middleware, response::Response};
use futures_util::FutureExt;
use serde_json::json;

use super::super::{replay, scenario, Symbols};
use super::{corpora, desktop_privacy, directory, Corpus};

const CURRENT: &str = "4f03c35a80322673ec00df7a3b6b9399015f5a68";

fn frozen() -> Corpus {
    serde_json::from_slice(&std::fs::read(directory().join("current-desktop.json")).unwrap())
        .unwrap()
}

#[test]
fn retained_privacy_addition_preserves_every_other_release_expectation() {
    let mut corpus = frozen();
    let original = serde_json::to_value(&corpus).unwrap();
    assert_eq!(
        desktop_privacy::apply(CURRENT, "current-desktop.json", &mut corpus),
        6
    );
    let mut additions = 0;
    for (case, before) in corpus
        .cases
        .iter_mut()
        .zip(original["cases"].as_array().unwrap())
    {
        if case.response.status == 404 {
            assert_eq!(
                case.response.headers.remove("cache-control").as_deref(),
                Some("no-store, private")
            );
            additions += 1;
        }
        assert_eq!(serde_json::to_value(case).unwrap(), *before);
    }
    assert_eq!(additions, 6);
    for (release, file) in [
        ("unlisted-release", "current-desktop.json"),
        (CURRENT, "current-agents.json"),
        (CURRENT, "previous-desktop.json"),
    ] {
        let mut corpus = frozen();
        assert_eq!(desktop_privacy::apply(release, file, &mut corpus), 0);
        assert_eq!(serde_json::to_value(corpus).unwrap(), original);
    }
}

#[test]
fn retained_privacy_addition_refuses_mismatched_or_previously_changed_cases() {
    for mutation in [
        "operation",
        "method",
        "path",
        "request-header",
        "request-body",
        "anonymous",
        "success",
        "body",
        "existing-cache",
        "unlisted-case",
        "missing-case",
        "duplicate-case",
    ] {
        let mut corpus = frozen();
        let case = corpus
            .cases
            .iter_mut()
            .find(|case| case.id == "desktop-discovery-other_owner")
            .unwrap();
        match mutation {
            "operation" => case.operation = "GET /api/v1/agents/{id}".into(),
            "method" => case.request.method = "HEAD".into(),
            "path" => case.request.path = "/api/v1/agents/agt_contract".into(),
            "request-header" => {
                case.request
                    .headers
                    .insert("x-unexpected".into(), "extra".into());
            }
            "request-body" => case.request.body = Some(json!({"unexpected": true})),
            "anonymous" => case.scenario = "desktop_anonymous".into(),
            "success" => case.response.status = 200,
            "body" => case.response.body["code"] = json!("not_authenticated"),
            "existing-cache" => {
                case.response
                    .headers
                    .insert("cache-control".into(), "public".into());
            }
            "unlisted-case" => case.id = "desktop-discovery-new-case".into(),
            "missing-case" => {
                corpus
                    .cases
                    .retain(|case| case.id != "desktop-discovery-other_owner");
            }
            "duplicate-case" => {
                let duplicate =
                    serde_json::from_value(serde_json::to_value(case).unwrap()).unwrap();
                corpus.cases.push(duplicate);
            }
            _ => unreachable!(),
        }
        assert!(
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                desktop_privacy::apply(CURRENT, "current-desktop.json", &mut corpus)
            }))
            .is_err(),
            "{mutation} must not receive the privacy addition"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum Drift {
    MissingCache,
    WrongCache,
    ExtraHeader,
    RepeatedCache,
    RetryOnNotFound,
    Cookie,
    Status,
    Body,
    Route,
}

#[tokio::test]
async fn retained_privacy_replay_rejects_header_cookie_status_body_and_route_drift() {
    for release in 0..2 {
        for drift in [
            Drift::MissingCache,
            Drift::WrongCache,
            Drift::ExtraHeader,
            Drift::RepeatedCache,
            Drift::RetryOnNotFound,
            Drift::Cookie,
            Drift::Status,
            Drift::Body,
            Drift::Route,
        ] {
            // Use the same hash-verified and adjusted corpus as the retained replay.
            let mut corpus = corpora()
                .into_iter()
                .filter(|corpus| {
                    corpus
                        .cases
                        .iter()
                        .any(|case| case.id == "desktop-discovery-other_owner")
                })
                .nth(release)
                .unwrap();
            let mut case = corpus.cases.remove(
                corpus
                    .cases
                    .iter()
                    .position(|case| case.id == "desktop-discovery-other_owner")
                    .unwrap(),
            );
            let mut scenario = scenario(&case.scenario).await;
            if matches!(drift, Drift::Route) {
                case.request.path = "/api/v1/absent-retained-route".into();
            }
            // Perturb the real handler response, never normalize away its headers.
            scenario.router = scenario.router.layer(middleware::map_response(
                move |mut response: Response| async move {
                    match drift {
                        Drift::MissingCache => {
                            response.headers_mut().remove("cache-control");
                        }
                        Drift::WrongCache => {
                            response
                                .headers_mut()
                                .insert("cache-control", "public".parse().unwrap());
                        }
                        Drift::ExtraHeader => {
                            response
                                .headers_mut()
                                .insert("x-unexpected", "extra".parse().unwrap());
                        }
                        Drift::RepeatedCache => {
                            response
                                .headers_mut()
                                .append("cache-control", "no-store, private".parse().unwrap());
                        }
                        Drift::RetryOnNotFound => {
                            response
                                .headers_mut()
                                .insert("retry-after", "1".parse().unwrap());
                        }
                        Drift::Cookie => {
                            response
                                .headers_mut()
                                .append("set-cookie", "extra=fixture".parse().unwrap());
                        }
                        Drift::Status => *response.status_mut() = StatusCode::GONE,
                        Drift::Body => {
                            *response.body_mut() = Body::from(
                                json!({
                                    "code": "desktop_unavailable", "detail": "changed refusal"
                                })
                                .to_string(),
                            )
                        }
                        Drift::Route => {}
                    }
                    response
                },
            ));
            let mut symbols = Symbols::default();
            let result = AssertUnwindSafe(replay::run(&scenario, &mut symbols, &mut case, false))
                .catch_unwind()
                .await;
            assert!(
                result.is_err(),
                "release {release} {drift:?} escaped exact retained replay"
            );
        }
    }
}
