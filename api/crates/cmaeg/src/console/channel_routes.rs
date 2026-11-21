//! The channel detail page. Every read goes through the owner-checked read
//! API with this request's `Authorized`; an unknown record and another
//! owner's record both get the generic 404 page.
use axum::{
    extract::{
        rejection::{PathRejection, QueryRejection},
        Path, Query, State,
    },
    http::StatusCode,
    response::Response,
    Extension,
};
use cmaeg_core::ids::RecordId;
use serde::Deserialize;

use super::{
    activity::ActivityFilter,
    attach_form::Submitted,
    authority::Authorized,
    directory::ChannelView,
    frame::SignedIn,
    guard::CurrentSession,
    pages::{
        self, activity, channel,
        channel::Tab,
        consumers, event, forms, overview,
        overview::OverviewView,
        producers, routing,
        sources::{self, SourcesView},
    },
    redirect::{self, HOME},
    Console,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ChannelQuery {
    tab: Option<String>,
    refresh: Option<String>,
    consumer: Option<String>,
    outcome: Option<String>,
    source: Option<String>,
}

/// `GET /console/channels/{record_id}?tab=…`.
pub(crate) async fn channel(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    record: Result<Path<String>, PathRejection>,
    query: Result<Query<ChannelQuery>, QueryRejection>,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::see_other(HOME);
    };
    let Some(view) = owned_channel(&console, &authorized, record).await else {
        return pages::not_found();
    };
    let signed_in = SignedIn::load(&console, &cell).await;
    let query = query.map(|Query(query)| query).unwrap_or_default();
    let tab = Tab::parse(query.tab.as_deref());
    let (head, content) = match tab {
        Tab::Overview => (
            String::new(),
            overview_tab(&console, &signed_in, &view, None),
        ),
        Tab::Sources => (
            String::new(),
            sources_tab(&console, &authorized, &signed_in, &view, None, None),
        ),
        Tab::Consumers => (String::new(), consumers::render(&view)),
        Tab::Routing => (
            String::new(),
            routing::render(&view, console.multi_consumer()),
        ),
        Tab::Activity => {
            let refresh = activity::refresh(query.refresh.as_deref());
            let filter = activity::filter(
                query.consumer.as_deref(),
                query.outcome.as_deref(),
                query.source.as_deref(),
            );
            let entries = console
                .directory()
                .activity(&authorized, &view.record, filter.clone())
                .unwrap_or_default();
            (
                activity::refresh_head(refresh),
                activity::render(&activity::ActivityView {
                    view: &view,
                    entries: &entries,
                    filter: &filter,
                    refresh,
                }),
            )
        }
        Tab::Producers => {
            let producers = console
                .directory()
                .producers(&authorized, &view.record)
                .unwrap_or_default();
            (String::new(), producers::render(&producers))
        }
    };
    signed_in.render(
        StatusCode::OK,
        &channel::title(&view),
        &head,
        &channel::body(&view, tab, &content),
    )
}

/// `GET /console/channels/{record_id}/events/{event_key}`: one event of
/// the ring, while it still holds the event.
pub(crate) async fn event(
    State(console): State<Console>,
    Extension(session): Extension<CurrentSession>,
    authorized: Authorized,
    params: Result<Path<(String, String)>, PathRejection>,
) -> Response {
    let Some(cell) = session.0 else {
        return redirect::see_other(HOME);
    };
    let Ok(Path((record, key))) = params else {
        return pages::not_found();
    };
    if !event::valid_key(&key) {
        return pages::not_found();
    }
    let Some(view) = owned_channel(&console, &authorized, Ok(Path(record))).await else {
        return pages::not_found();
    };
    let directory = console.directory();
    let Some(entry) = directory.event(&authorized, &view.record, &key) else {
        return pages::not_found();
    };
    let parent_held = entry.causation.as_ref().is_some_and(|causation| {
        directory
            .event(&authorized, &view.record, &causation.parent_key)
            .is_some()
    });
    let signed_in = SignedIn::load(&console, &cell).await;
    signed_in.render(
        StatusCode::OK,
        &format!("Event {key}"),
        "",
        &event::render(&event::EventView {
            view: &view,
            entry: &entry,
            parent_held,
        }),
    )
}

/// The channel the path names, when `authorized` owns it.
pub(crate) async fn owned_channel(
    console: &Console,
    authorized: &Authorized,
    record: Result<Path<String>, PathRejection>,
) -> Option<ChannelView> {
    let Path(raw) = record.ok()?;
    let record = RecordId::new(raw).ok()?;
    console.directory().channel(authorized, &record).await
}

/// The Overview tab; `link_error` re-renders a refused paste-link.
pub(crate) fn overview_tab(
    console: &Console,
    signed_in: &SignedIn,
    view: &ChannelView,
    link_error: Option<&str>,
) -> String {
    let csrf = signed_in.csrf(overview::LINK_PATH, view.record.as_str());
    overview::render(&OverviewView {
        view,
        public_url: console.public_api_url(),
        sources: console.source_changes().availability(),
        multi_consumer: console.multi_consumer(),
        unknown_id: &overview::paste_form(&view.record, &csrf, link_error),
    })
}

/// The Sources tab; `submitted` and `error` re-render a refused attach.
pub(crate) fn sources_tab(
    console: &Console,
    authorized: &Authorized,
    signed_in: &SignedIn,
    view: &ChannelView,
    submitted: Option<&Submitted>,
    error: Option<&str>,
) -> String {
    let last_events: Vec<_> = view
        .summary
        .detail
        .iter()
        .flat_map(|detail| detail.sources.iter())
        .map(|source| {
            console
                .directory()
                .activity(
                    authorized,
                    &view.record,
                    ActivityFilter {
                        source: Some(source.id.as_str().to_owned()),
                        ..ActivityFilter::default()
                    },
                )
                .and_then(|entries| entries.first().map(|entry| entry.at))
        })
        .collect();
    let forms = forms::render(
        &view.record,
        &signed_in.csrf(forms::ATTACH_PATH, view.record.as_str()),
        view.version,
        submitted,
        error,
    );
    sources::render(&SourcesView {
        view,
        last_events: &last_events,
        availability: console.source_changes().availability(),
        forms: &forms,
    })
}
