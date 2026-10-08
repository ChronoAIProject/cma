//! Attach and detach a channel's sources from the console.
//!
//! The guard has checked Origin, the session and the CSRF token. Each
//! handler takes `Authorized`, so the owner was verified for this request
//! before anything is written; it acts only on a record the owner index
//! gives to that owner, and passes `authorized.creator()` to the same
//! record-keyed management functions as REST through [`SourceChanges`].
//! While source changes are unavailable every handler answers a typed page
//! and makes no call at all.
use axum::{
    body::Bytes,
    extract::{rejection::PathRejection, Path, State},
    http::StatusCode,
    response::Response,
    Extension,
};
use cmaeg_core::channel::SourceId;

use super::{
    attach_form::{self, Submitted},
    authority::Authorized,
    channel_routes::{owned_channel, sources_tab},
    directory::ChannelView,
    frame::SignedIn,
    guard::CurrentSession,
    mutations::{field, ActionLog},
    pages::{
        self, channel,
        channel::{href, Tab},
        detach::{self, DetachView, DETACH_PATH},
        reveal::{self, RevealView},
    },
    redirect,
    session::SessionCell,
    source_changes::{SourceAvailability, SourceChangeError},
    Console,
};

const CHANGED: &str = "This channel changed since the page was loaded. Review it and try again.";
const DEFAULT_REPLY: &str =
    "This source is the channel's default reply source. Change the channel's routing first.";

/// `POST /console/channels/{record_id}/sources`: attach a source. 303 to
/// the Sources tab, or, when the gateway generated the source's HMAC
/// secret, 200 with the one-time reveal. A refusal re-renders the form
/// without any submitted secret.
pub(crate) async fn attach(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    record: Result<Path<String>, PathRejection>,
    form: Bytes,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::signed_out(true);
    };
    let log = ActionLog::new("attach", &cell, &authorized);
    let Some(view) = owned_channel(&console, &authorized, record).await else {
        log.outcome(None, "not_found", None);
        return pages::not_found();
    };
    let changes = console.source_changes();
    let availability = changes.availability();
    if availability != SourceAvailability::Available {
        log.outcome(Some(&view.record), "unavailable", None);
        return unavailable(&console, &cell, &view, availability).await;
    }
    let parsed = match attach_form::parse(&form) {
        Ok(parsed) => parsed,
        Err(message) => {
            log.outcome(Some(&view.record), "invalid", None);
            let refusal = Refusal::new(StatusCode::BAD_REQUEST, message);
            return refused_attach(&console, &cell, &authorized, &view, &form, refusal).await;
        }
    };
    let generated = parsed.generated;
    let outcome = changes
        .attach(authorized.creator(), &view.record, parsed.request)
        .await;
    let attached = match outcome {
        Ok(attached) => attached,
        Err(error) => {
            log.outcome(Some(&view.record), label(&error), None);
            return match answer(error) {
                Answer::OnForm(refusal) => {
                    refused_attach(&console, &cell, &authorized, &view, &form, refusal).await
                }
                Answer::Page(response) => response,
                Answer::Unavailable(availability) => {
                    unavailable(&console, &cell, &view, availability).await
                }
            };
        }
    };
    log.outcome(Some(&view.record), "ok", Some(attached.source_id.as_str()));
    let Some(generated) = generated else {
        return redirect::see_other(&href(&view.record, Tab::Sources));
    };
    let signed_in = SignedIn::load(&console, &cell).await;
    signed_in.render(
        StatusCode::OK,
        "Source attached",
        "",
        &reveal::render(&RevealView {
            record: &view.record,
            source_id: &attached.source_id,
            key_id: &generated.key_id,
            secret: generated.secret.as_str(),
        }),
    )
}

/// `GET /console/channels/{record_id}/sources/{source_id}/detach`: what a
/// detach will do, and the form that confirms it.
pub(crate) async fn confirm_detach(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::see_other(redirect::HOME);
    };
    let Some((view, source_id)) = target(&console, &authorized, params).await else {
        return pages::not_found();
    };
    let availability = console.source_changes().availability();
    if availability != SourceAvailability::Available {
        return unavailable(&console, &cell, &view, availability).await;
    }
    render_detach(&console, &cell, &view, &source_id, StatusCode::OK, None).await
}

/// `POST /console/channels/{record_id}/sources/{source_id}/detach`: detach
/// the source at the version the owner confirmed, then 303 to the Sources
/// tab. The source stops admitting events before the response returns.
pub(crate) async fn detach(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    params: Result<Path<(String, String)>, PathRejection>,
    form: Bytes,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::signed_out(true);
    };
    let log = ActionLog::new("detach", &cell, &authorized);
    let Some((view, source_id)) = target(&console, &authorized, params).await else {
        log.outcome(None, "not_found", None);
        return pages::not_found();
    };
    let source = Some(source_id.as_str());
    let changes = console.source_changes();
    let availability = changes.availability();
    if availability != SourceAvailability::Available {
        log.outcome(Some(&view.record), "unavailable", source);
        return unavailable(&console, &cell, &view, availability).await;
    }
    let Some(expected_version) = field(&form, "expected_version").and_then(|v| v.parse().ok())
    else {
        log.outcome(Some(&view.record), "invalid", source);
        let message = "This form is out of date. Reload the page and try again.";
        return render_detach(
            &console,
            &cell,
            &view,
            &source_id,
            StatusCode::BAD_REQUEST,
            Some(message),
        )
        .await;
    };
    let outcome = changes
        .detach(
            authorized.creator(),
            &view.record,
            &source_id,
            expected_version,
        )
        .await;
    match outcome {
        Ok(_) => {
            log.outcome(Some(&view.record), "ok", source);
            redirect::see_other(&href(&view.record, Tab::Sources))
        }
        Err(error) => {
            log.outcome(Some(&view.record), label(&error), source);
            match answer(error) {
                Answer::OnForm(refusal) => {
                    // The confirmation again, at the version memory now holds.
                    let fresh = console
                        .directory()
                        .channel(&authorized, &view.record)
                        .await
                        .unwrap_or(view);
                    let message = Some(refusal.message.as_str());
                    render_detach(&console, &cell, &fresh, &source_id, refusal.status, message)
                        .await
                }
                Answer::Page(response) => response,
                Answer::Unavailable(availability) => {
                    unavailable(&console, &cell, &view, availability).await
                }
            }
        }
    }
}

/// The owner's channel and one of its sources, from the path. A source id
/// outside the slot grammar names no source of any channel.
async fn target(
    console: &Console,
    authorized: &Authorized,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Option<(ChannelView, SourceId)> {
    let Path((record, source_id)) = params.ok()?;
    let source_id = SourceId::parse(&source_id).ok()?;
    let view = owned_channel(console, authorized, Ok(Path(record))).await?;
    let known = view
        .summary
        .detail
        .as_ref()?
        .sources
        .iter()
        .any(|source| source.id == source_id);
    known.then_some((view, source_id))
}

/// A refusal shown on the form it came from; the message is fixed console
/// text or management's own pointer and message.
struct Refusal {
    status: StatusCode,
    message: String,
}

impl Refusal {
    fn new(status: StatusCode, message: &str) -> Self {
        Self {
            status,
            message: message.to_owned(),
        }
    }
}

/// How a refused change is answered.
enum Answer {
    /// On the form the owner posted.
    OnForm(Refusal),
    /// With a page of its own.
    Page(Response),
    /// With the typed page for source changes this gateway cannot make.
    Unavailable(SourceAvailability),
}

fn answer(error: SourceChangeError) -> Answer {
    match error {
        SourceChangeError::Conflict => Answer::OnForm(Refusal::new(StatusCode::CONFLICT, CHANGED)),
        SourceChangeError::DefaultReplyInUse => {
            Answer::OnForm(Refusal::new(StatusCode::CONFLICT, DEFAULT_REPLY))
        }
        SourceChangeError::Invalid { pointer, message } => Answer::OnForm(Refusal {
            status: StatusCode::BAD_REQUEST,
            message: format!("Refused at {pointer}: {message}"),
        }),
        SourceChangeError::NotFound => Answer::Page(pages::not_found()),
        SourceChangeError::Unavailable => Answer::Page(pages::unavailable()),
        SourceChangeError::Disabled => Answer::Unavailable(SourceAvailability::Disabled),
        SourceChangeError::NotYetAvailable => {
            Answer::Unavailable(SourceAvailability::NotYetAvailable)
        }
    }
}

fn label(error: &SourceChangeError) -> &'static str {
    match error {
        SourceChangeError::NotYetAvailable | SourceChangeError::Disabled => "unavailable",
        SourceChangeError::Invalid { .. } => "invalid",
        SourceChangeError::Conflict => "conflict",
        SourceChangeError::DefaultReplyInUse => "default_reply_in_use",
        SourceChangeError::NotFound => "not_found",
        SourceChangeError::Unavailable => "error",
    }
}

/// The Sources tab again with the refused form: what the owner typed, but
/// never a secret.
async fn refused_attach(
    console: &Console,
    cell: &std::sync::Arc<SessionCell>,
    authorized: &Authorized,
    view: &ChannelView,
    form: &[u8],
    refusal: Refusal,
) -> Response {
    let fresh = console
        .directory()
        .channel(authorized, &view.record)
        .await
        .unwrap_or_else(|| view.clone());
    let signed_in = SignedIn::load(console, cell).await;
    let submitted = Submitted::of(form);
    let content = sources_tab(
        console,
        authorized,
        &signed_in,
        &fresh,
        Some(&submitted),
        Some(&refusal.message),
    );
    signed_in.render(
        refusal.status,
        &channel::title(&fresh),
        "",
        &channel::body(&fresh, Tab::Sources, &content),
    )
}

async fn render_detach(
    console: &Console,
    cell: &std::sync::Arc<SessionCell>,
    view: &ChannelView,
    source_id: &SourceId,
    status: StatusCode,
    error: Option<&str>,
) -> Response {
    let Some(source) = view
        .summary
        .detail
        .iter()
        .flat_map(|detail| detail.sources.iter())
        .find(|source| source.id == *source_id)
    else {
        return pages::not_found();
    };
    let signed_in = SignedIn::load(console, cell).await;
    let content = detach::render(&DetachView {
        view,
        source,
        csrf: &signed_in.csrf(DETACH_PATH, view.record.as_str()),
        error,
    });
    signed_in.render(
        status,
        &channel::title(view),
        "",
        &channel::body(view, Tab::Sources, &content),
    )
}

/// The typed page for a source change this gateway cannot make now.
async fn unavailable(
    console: &Console,
    cell: &std::sync::Arc<SessionCell>,
    view: &ChannelView,
    availability: SourceAvailability,
) -> Response {
    let (status, text) = detach::unavailable(availability);
    let signed_in = SignedIn::load(console, cell).await;
    signed_in.render(
        status,
        "Source change not made",
        "",
        &detach::unavailable_body(&view.record, text),
    )
}
