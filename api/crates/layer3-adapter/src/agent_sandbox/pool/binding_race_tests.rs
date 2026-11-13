use super::*;

// A prior status success is never reusable proof for the reserved attempt.
// A changed Pod falls back to full bootstrap; other authority mismatches refuse.
#[tokio::test]
async fn binding_retry_repeats_native_authority_before_and_after_an_earlier_status() {
    for after_status in [false, true] {
        for change in ["claim", "pod", "pvc", "image", "generation"] {
            let fixture = inspection_fixture(false).await;
            if after_status {
                fixture
                    .adapter
                    .inspect_runtime(&fixture.adoption)
                    .await
                    .unwrap()
                    .unwrap();
            }
            let assignments = fixture
                .bound
                .fixture
                .state
                .lock()
                .unwrap()
                .assignments
                .len();
            {
                let mut state = fixture.bound.fixture.state.lock().unwrap();
                let (path, pointer, value) = match change {
                    "claim" => (
                        format!(
                            "{}/sandboxclaims/{}",
                            crate::agent_sandbox::pool::fixture::prefix(),
                            fixture.bound.claim.name
                        ),
                        "/metadata/uid",
                        json!("replacement"),
                    ),
                    "pod" => (POD.into(), "/metadata/uid", json!("replacement")),
                    "pvc" => (VOLUME.into(), "/metadata/uid", json!("replacement")),
                    "image" => (
                        MEMBER.into(),
                        "/spec/podTemplate/spec/containers/0/image",
                        json!("replacement"),
                    ),
                    "generation" => (
                        MEMBER.into(),
                        "/metadata/annotations/cma~1runtime-generation",
                        json!("42"),
                    ),
                    _ => unreachable!(),
                };
                *state
                    .rows
                    .get_mut(&path)
                    .unwrap()
                    .pointer_mut(pointer)
                    .unwrap() = value;
                state.calls.clear();
            }
            let result = crate::binding_diagnostics::scope(
                tokio::time::Instant::now() + std::time::Duration::from_secs(20),
                fixture.backend.inspect_native_runtime(
                    &fixture.bound.runtime,
                    &fixture.bound.request,
                    &fixture.home,
                ),
            )
            .await;
            if change == "pod" {
                assert!(
                    result.unwrap().is_none(),
                    "a previous process receipt cannot authorize this Pod"
                );
            } else {
                assert!(result.is_err(), "{change}, after_status={after_status}");
            }
            assert_eq!(
                fixture.transport.calls.load(Ordering::SeqCst),
                usize::from(after_status)
            );
            let state = fixture.bound.fixture.state.lock().unwrap();
            assert_eq!(state.assignments.len(), assignments);
            assert!(state
                .calls
                .iter()
                .all(|(method, _)| method.starts_with("GET ")));
        }
    }
}
