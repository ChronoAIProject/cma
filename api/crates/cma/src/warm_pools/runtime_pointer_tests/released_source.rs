use layer3_adapter::{ReleasedRuntime, ReleasedRuntimeSource, StaleReleasedRuntime};
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Call {
    Snapshot(u64),
    RequireCurrent(u64, bool),
}

#[derive(Debug)]
pub(crate) struct ControlledSource(Mutex<State>);

#[derive(Debug)]
struct State {
    cached: ReleasedRuntime,
    current: ReleasedRuntime,
    advance_after_validation: Option<ReleasedRuntime>,
    advance_after_snapshot: Option<ReleasedRuntime>,
    calls: Vec<Call>,
}

impl ControlledSource {
    pub(crate) fn new(initial: ReleasedRuntime) -> Self {
        Self(Mutex::new(State {
            cached: initial.clone(),
            current: initial,
            advance_after_validation: None,
            advance_after_snapshot: None,
            calls: vec![],
        }))
    }

    pub(crate) fn refresh_to(&self, next: ReleasedRuntime) {
        let mut state = self.0.lock().unwrap();
        assert!(next.revision > state.current.revision);
        state.cached = next.clone();
        state.current = next;
    }

    pub(crate) fn miss_refresh(&self, next: ReleasedRuntime) {
        let mut state = self.0.lock().unwrap();
        assert!(next.revision > state.current.revision);
        state.current = next;
    }

    pub(crate) fn advance_after_validation(&self, next: ReleasedRuntime) {
        let mut state = self.0.lock().unwrap();
        assert!(next.revision > state.current.revision);
        assert!(state.advance_after_validation.replace(next).is_none());
    }

    pub(crate) fn advance_after_snapshot(&self, next: ReleasedRuntime) {
        let mut state = self.0.lock().unwrap();
        assert!(next.revision > state.current.revision);
        assert!(state.advance_after_snapshot.replace(next).is_none());
    }

    pub(crate) fn take_calls(&self) -> Vec<Call> {
        std::mem::take(&mut self.0.lock().unwrap().calls)
    }
}

impl ReleasedRuntimeSource for ControlledSource {
    fn snapshot(&self) -> ReleasedRuntime {
        let mut state = self.0.lock().unwrap();
        let released = state.cached.clone();
        state.calls.push(Call::Snapshot(released.revision));
        if let Some(next) = state.advance_after_snapshot.take() {
            state.cached = next.clone();
            state.current = next;
        }
        released
    }

    fn require_current(&self, released: &ReleasedRuntime) -> Result<(), StaleReleasedRuntime> {
        let mut state = self.0.lock().unwrap();
        let current = *released == state.current;
        state
            .calls
            .push(Call::RequireCurrent(released.revision, current));
        if !current {
            state.cached = state.current.clone();
            return Err(StaleReleasedRuntime);
        }
        if let Some(next) = state.advance_after_validation.take() {
            state.cached = next.clone();
            state.current = next;
        }
        Ok(())
    }
}

#[test]
fn validation_detects_a_refresh_miss_and_preserves_a_prior_snapshot() {
    let release = |revision| ReleasedRuntime {
        lane_revisions: None,
        revision,
        agent: format!("registry.example/agent@sha256:{revision:064x}"),
        machine: None,
    };
    let source = ControlledSource::new(release(1));
    let original = source.snapshot();
    source.miss_refresh(release(2));
    assert_eq!(source.snapshot(), original);
    assert_eq!(source.require_current(&original), Err(StaleReleasedRuntime));
    let second = source.snapshot();
    source.advance_after_validation(release(3));
    assert_eq!(source.require_current(&second), Ok(()));
    assert_eq!(second, release(2));
    assert_eq!(source.snapshot(), release(3));
}
