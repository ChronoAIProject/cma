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
    authority::Authorized,
    directory::ChannelView,
    frame::SignedIn,
    guard::CurrentSession,
    pages::{
        self, activity, channel,
        channel::Tab,
        consumers, overview,
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
    let query = query.map(|Query(query)| query).unwrap_or_default();
    let tab = Tab::parse(query.tab.as_deref());
    let (head, content) = match tab {
        Tab::Overview => (String::new(), overview_tab(&console, &view)),
        Tab::Sources => (String::new(), sources_tab(&console, &authorized, &view)),
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
    let signed_in = SignedIn::load(&console, &cell).await;
    signed_in.render(
        StatusCode::OK,
        &channel::title(&view),
        &head,
        &channel::body(&view, tab, &content),
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

fn overview_tab(console: &Console, view: &ChannelView) -> String {
    overview::render(&OverviewView {
        view,
        public_url: console.public_url(),
        sources: console.source_changes().availability(),
        multi_consumer: console.multi_consumer(),
        unknown_id: "<span class=\"empty\">Not observed since restart.</span>",
    })
}

fn sources_tab(console: &Console, authorized: &Authorized, view: &ChannelView) -> String {
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
                        source: Some(source.activity_label.clone()),
                        ..ActivityFilter::default()
                    },
                )
                .and_then(|entries| entries.first().map(|entry| entry.at))
        })
        .collect();
    sources::render(&SourcesView {
        view,
        last_events: &last_events,
        availability: console.source_changes().availability(),
    })
}
