//! Explicit operator command. No server, scheduler or startup activation.
mod command;
pub(crate) mod export;
mod model;
pub(crate) mod verify;
pub(crate) use command::{command, is_command};
pub(crate) use model::*;
pub(crate) type Result<T> = std::result::Result<T, &'static str>;
pub(crate) const REFUSED: &str =
    "retirement archive command refused or incomplete; reconcile the original operation";

#[cfg(test)]
pub(crate) mod tests;
