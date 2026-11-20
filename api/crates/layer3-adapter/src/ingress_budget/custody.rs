//! Keep counted shared metadata allocated only while its reservation is held.

use std::{fmt, ops::Deref, sync::Arc};

/// No weak or raw Arc handle escapes. Every strong owner participates in
/// Arc::into_inner, so exactly one racing final drop extracts the value.
pub(crate) struct SharedCustody<T> {
    inner: Option<Arc<T>>,
}

impl<T> SharedCustody<T> {
    /// The caller must admit the Arc layout before constructing this handle.
    pub(crate) fn new(value: T) -> Self {
        Self {
            inner: Some(Arc::new(value)),
        }
    }

    pub(crate) fn ptr_eq(left: &Self, right: &Self) -> bool {
        Arc::ptr_eq(left.inner.as_ref().unwrap(), right.inner.as_ref().unwrap())
    }
}

impl<T> Clone for SharedCustody<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Deref for SharedCustody<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.inner.as_ref().unwrap()
    }
}

impl<T: fmt::Debug> fmt::Debug for SharedCustody<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(formatter)
    }
}

impl<T> Drop for SharedCustody<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            // into_inner frees the Arc backing before returning its extracted
            // value. Only then may dropping that value return counted permits.
            drop(Arc::into_inner(inner));
        }
    }
}
