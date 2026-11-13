//! Diagnostic context is independent of execution identity and send authority.
mod bound;
pub(crate) mod config;
pub(crate) mod ingress;
mod operation;
mod parent;
mod record;
mod runtime;
pub(crate) mod scope;
mod writer;

pub(crate) use bound::BoundOperation;
pub(crate) use operation::Operation;
pub(crate) use parent::{RemoteParent, SpanId};
pub(crate) use record::{Context, Outcome, Stage};
pub(crate) use runtime::{enabled, log_writer, milestone, start, start_part};
pub(crate) use writer::Writer;

#[cfg(test)]
pub(crate) mod testing;
