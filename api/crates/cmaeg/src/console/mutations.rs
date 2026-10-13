//! The console's form posts on a channel. The guard has checked Origin, the
//! session and the CSRF token before these run; each takes `Authorized`, so
//! the owner is verified again for this request, and acts only on a record
//! the owner index gives to that owner. No handler reads the session's
//! selection or builds a creator itself.
use axum::{
    body::Bytes,
    extract::{rejection::PathRejection, Path, State},
    http::StatusCode,
    response::Response,
    Extension,
};
use cmaeg_core::ids::{ChannelId, RecordId};

use super::{
    authority::{Authorized, OwnerScope},
    channel_ids::LinkError,
    channel_routes::{overview_tab, owned_channel},
    frame::SignedIn,
    guard::CurrentSession,
    pages::{
        self, channel,
        channel::{href, Tab},
    },
    redirect,
    session::SessionCell,
    subject_hash_prefix, Console,
};

/// `POST /console/channels/{record_id}/link`: remember a pasted channel id
/// for display once it opens to this record. 303 to the Overview; a refused
/// id re-renders it with a fixed message, never the pasted value.
pub(crate) async fn link(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    record: Result<Path<String>, PathRejection>,
    form: Bytes,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::signed_out(true);
    };
    let log = ActionLog::new("link", &cell, &authorized);
    let Some(view) = owned_channel(&console, &authorized, record).await else {
        log.outcome(None, "not_found", None);
        return pages::not_found();
    };
    let outcome = match field(&form, "channel_id").and_then(|id| ChannelId::new(id.trim()).ok()) {
        Some(id) => {
            console
                .directory()
                .link(&authorized, &view.record, &id)
                .await
        }
        None => Err(LinkError::Malformed),
    };
    let (label, message) = match outcome {
        Ok(()) => ("ok", None),
        Err(LinkError::NotFound) => ("not_found", None),
        Err(LinkError::Mismatch) => (
            "mismatch",
            Some("This channel id belongs to another channel."),
        ),
        Err(LinkError::Malformed) => (
            "malformed",
            Some("This is not a channel id of this gateway."),
        ),
    };
    log.outcome(Some(&view.record), label, None);
    match (label, message) {
        ("ok", _) => redirect::see_other(&href(&view.record, Tab::Overview)),
        (_, Some(message)) => {
            let signed_in = SignedIn::load(&console, &cell).await;
            let content = overview_tab(&console, &signed_in, &view, Some(message));
            signed_in.render(
                StatusCode::BAD_REQUEST,
                &channel::title(&view),
                "",
                &channel::body(&view, Tab::Overview, &content),
            )
        }
        _ => pages::not_found(),
    }
}

/// The one value of form field `name`; `None` when it is absent or given
/// more than once.
pub(crate) fn field(form: &[u8], name: &str) -> Option<String> {
    let mut values = url::form_urlencoded::parse(form)
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned());
    match (values.next(), values.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}

/// `console_action` lines for one request: what was done to which record,
/// with the outcome, the subject's 8-hex hash prefix and the owner kind.
/// Never a channel id, a form value or a secret.
pub(crate) struct ActionLog<'a> {
    action: &'static str,
    cell: &'a SessionCell,
    authorized: &'a Authorized,
}

impl<'a> ActionLog<'a> {
    pub(crate) fn new(
        action: &'static str,
        cell: &'a SessionCell,
        authorized: &'a Authorized,
    ) -> Self {
        Self {
            action,
            cell,
            authorized,
        }
    }

    /// Log `outcome` on `record` (unknown when `None`), naming the source a
    /// source change acted on.
    pub(crate) fn outcome(
        &self,
        record: Option<&RecordId>,
        outcome: &'static str,
        source_id: Option<&str>,
    ) {
        let record_hash = record.map_or_else(
            || "-".to_owned(),
            |record| crate::telemetry::channel_hash(record.as_str()),
        );
        let owner_kind = match self.authorized.scope() {
            OwnerScope::Personal => "personal",
            OwnerScope::Organization => "organization",
        };
        let subject_hash = subject_hash_prefix(&self.cell.meta().subject);
        tracing::info!(
            action = self.action,
            record_hash = %record_hash,
            outcome,
            subject_hash_prefix = %subject_hash,
            owner_kind,
            source_id = source_id.unwrap_or("-"),
            "console_action"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::field;

    #[test]
    fn a_field_must_appear_exactly_once() {
        assert_eq!(field(b"a=1&b=2", "a").as_deref(), Some("1"));
        assert_eq!(field(b"a=1&a=2", "a"), None);
        assert_eq!(field(b"b=2", "a"), None);
        assert_eq!(field(b"a=%3Cx%3E+y", "a").as_deref(), Some("<x> y"));
    }
}
