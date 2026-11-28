use super::*;
#[test]
fn response_reads_and_concrete_submissions_use_only_the_documented_paths() {
    for path in [
        "/api/v2/responses/resp_one",
        "/api/v2/responses/resp_one/events",
        "/api/v2/responses/resp_one/output",
    ] {
        assert!(eligible(&Request::get(path).body(Body::empty()).unwrap()));
    }
    for path in [
        "/api/v2/responses/resp_one/events/extra",
        "/api/v2/responses/resp_one/cancel",
    ] {
        assert!(!eligible(&Request::get(path).body(Body::empty()).unwrap()));
    }
    let direct = Request::post("/api/v2/responses")
        .body(Body::empty())
        .unwrap();
    assert!(eligible(&direct));
    for body in [
        br#"{"conversation":"agt_one"}"#.as_slice(),
        br#"{"conversation":{"id":"agt_one"}}"#,
    ] {
        assert_eq!(agent(&direct, body).unwrap(), AgentId("agt_one".into()));
    }
    for body in [
        b"{}".as_slice(),
        b"malformed",
        br#"{"conversation":"invalid/id"}"#,
    ] {
        assert!(agent(&direct, body).is_err());
    }
    let alias = Request::post("/api/v2/agents/agt_one/responses")
        .body(Body::empty())
        .unwrap();
    assert!(eligible(&alias));
    assert_eq!(agent(&alias, b"{}").unwrap(), AgentId("agt_one".into()));
}
#[test]
fn observation_routes_bind_exact_agent_and_refuse_ambiguous_or_malformed_input() {
    for path in [
        "/api/v1/agui/events?thread_id=agt_one",
        "/api/v2/agents/agt_one/events",
    ] {
        let request = Request::get(path).body(Body::empty()).unwrap();
        assert!(eligible(&request));
        assert_eq!(agent(&request, &[]).unwrap(), AgentId("agt_one".into()));
    }
    for path in [
        "/api/v1/agui/events?thread_id=agt_one&thread_id=agt_two",
        "/api/v1/agui/events",
        "/api/v2/agents/invalid%2Fid/events",
    ] {
        let request = Request::get(path).body(Body::empty()).unwrap();
        assert!(agent(&request, &[]).is_err());
    }
    let post = Request::post("/api/v1/agui/run")
        .body(Body::empty())
        .unwrap();
    assert!(eligible(&post));
    assert_eq!(
        agent(&post, br#"{"threadId":"agt_one"}"#).unwrap(),
        AgentId("agt_one".into())
    );
    for body in [br#"{}"#.as_slice(), br#"{"threadId":9}"#, b"malformed"] {
        assert!(agent(&post, body).is_err());
    }
    assert!(!eligible(
        &Request::get("/api/v2/agents/agt_one/events/snapshot")
            .body(Body::empty())
            .unwrap()
    ));
    assert!(!eligible(
        &Request::post("/api/v2/agents/agt_one/events")
            .body(Body::empty())
            .unwrap()
    ));
}
