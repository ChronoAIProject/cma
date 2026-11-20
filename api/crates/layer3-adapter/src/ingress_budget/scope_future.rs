use super::*;
use futures_util::FutureExt;

impl<F: Future> OwnedWork<F> {
    /// Resolve an owned async transformation without detaching its input or
    /// result. This carries allocation custody; it does not install task locals.
    pub fn resolve(self) -> impl Future<Output = OwnedWork<F::Output>> {
        let Self { value, scope } = self;
        // futures-util 0.3.34 Map drops its future before the callback on
        // cancellation, and before invoking the callback on completion.
        value.map(move |value| OwnedWork { value, scope })
    }
}

#[cfg(test)]
#[path = "scope_future_tests.rs"]
mod tests;
