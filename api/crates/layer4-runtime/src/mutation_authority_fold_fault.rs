//! Thread-local directory-sync fault injection for synchronous folding tests.
use std::cell::RefCell;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub path: PathBuf,
    pub watched_exists: bool,
    pub succeeded: bool,
}

struct State {
    failure: Option<(PathBuf, usize)>,
    watched: PathBuf,
    events: Vec<Event>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub(crate) struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        STATE.with(|state| *state.borrow_mut() = None);
    }
}

pub(crate) fn arm(failure: Option<(PathBuf, usize)>, watched: PathBuf) -> Guard {
    STATE.with(|state| {
        assert!(state.borrow().is_none(), "sync fault guards must not nest");
        *state.borrow_mut() = Some(State {
            failure,
            watched,
            events: Vec::new(),
        });
    });
    Guard
}

pub(crate) fn events() -> Vec<Event> {
    STATE.with(|state| state.borrow().as_ref().unwrap().events.clone())
}

pub(crate) fn before_sync(path: &Path) -> std::io::Result<()> {
    STATE.with(|cell| {
        let mut cell = cell.borrow_mut();
        let Some(state) = cell.as_mut() else {
            return Ok(());
        };
        state.events.push(Event {
            path: path.to_path_buf(),
            watched_exists: state.watched.exists(),
            succeeded: false,
        });
        if let Some((target, remaining)) = &mut state.failure {
            if path == target {
                *remaining -= 1;
                if *remaining == 0 {
                    state.failure = None;
                    return Err(std::io::Error::other(
                        "injected authority directory sync failure",
                    ));
                }
            }
        }
        Ok(())
    })
}

pub(crate) fn after_sync() {
    STATE.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            state.events.last_mut().unwrap().succeeded = true;
        }
    });
}
