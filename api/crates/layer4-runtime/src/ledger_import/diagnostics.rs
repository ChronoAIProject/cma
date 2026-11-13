//! Unit-test assertion context only; never changes an error or logs input paths.
use std::cell::RefCell;
use std::panic::Location;

use crate::failure::Result;

#[derive(Debug)]
struct Rejection {
    guard: &'static str,
    site: &'static Location<'static>,
    os_error: Option<std::io::Error>,
}

thread_local! {
    // None means no synchronous assertion is collecting on this test thread.
    static ACTIVE: RefCell<Option<Option<Rejection>>> = const { RefCell::new(None) };
}

#[track_caller]
pub(super) fn record(guard: &'static str, os_error: Option<std::io::Error>) {
    let site = Location::caller();
    ACTIVE.with_borrow_mut(|active| {
        if let Some(first @ None) = active {
            *first = Some(Rejection {
                guard,
                site,
                os_error,
            });
        }
    });
}

// plan::create is synchronous: no context crosses an await or a thread boundary.
// Restore on unwinding too, and do not let a later cleanup replace the first guard.
fn capture<T>(operation: impl FnOnce() -> Result<T>) -> (Result<T>, Option<Rejection>) {
    struct Restore(Option<Option<Rejection>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.with_borrow_mut(|active| *active = self.0.take());
        }
    }
    let _restore = Restore(ACTIVE.replace(Some(None)));
    let result = operation();
    let rejection = ACTIVE.with_borrow_mut(|active| active.as_mut().and_then(Option::take));
    (result, rejection)
}

#[track_caller]
pub(super) fn assert_ok<T>(operation: impl FnOnce() -> Result<T>) -> T {
    let (result, rejection) = capture(operation);
    result.unwrap_or_else(|error| {
        let context = rejection.map(|r| {
            // OS-generated text is safe; never format an arbitrary io::Error payload.
            let os = r.os_error.map(|error| {
                (
                    error.kind(),
                    error.raw_os_error(),
                    error
                        .raw_os_error()
                        .map(|code| std::io::Error::from_raw_os_error(code).to_string()),
                )
            });
            (r.guard, r.site.to_string(), os)
        });
        panic!("ledger import plan assertion failed: {error:?}; guard/site/os_error={context:?}")
    })
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
