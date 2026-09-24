//! Public, server-rendered integration documentation bundled with the gateway.
//! Only these reviewed documents are published; operations runbooks stay private.

use std::{collections::HashMap, sync::LazyLock};

use axum::{
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};

const GUIDE: &str = include_str!("../../../../../docs/CMAEG_Integration.md");
const ARCHITECTURE: &str = include_str!("../../../../../docs/CMAEG_Architecture.md");
const PROTOCOL: &str = include_str!("../../../../../docs/CMAEG_Protocol.md");
const STYLE: &str = include_str!("docs.css");

static PAGES: LazyLock<[String; 3]> = LazyLock::new(|| {
    [
        page("Integration guide", "/docs", "/docs/integration.md", GUIDE),
        page(
            "Architecture",
            "/docs/architecture",
            "/docs/architecture.md",
            ARCHITECTURE,
        ),
        page(
            "Protocol reference",
            "/docs/protocol",
            "/docs/protocol.md",
            PROTOCOL,
        ),
    ]
});

pub(crate) fn router() -> Router {
    Router::new()
        .route(
            "/docs",
            get(|| async { document(&PAGES[0], "text/html; charset=utf-8") }),
        )
        .route(
            "/docs/",
            get(|| async { axum::response::Redirect::permanent("/docs") }),
        )
        .route(
            "/docs/architecture",
            get(|| async { document(&PAGES[1], "text/html; charset=utf-8") }),
        )
        .route(
            "/docs/protocol",
            get(|| async { document(&PAGES[2], "text/html; charset=utf-8") }),
        )
        .route(
            "/docs/integration.md",
            get(|| async { document(GUIDE, "text/markdown; charset=utf-8") }),
        )
        .route(
            "/docs/architecture.md",
            get(|| async { document(ARCHITECTURE, "text/markdown; charset=utf-8") }),
        )
        .route(
            "/docs/protocol.md",
            get(|| async { document(PROTOCOL, "text/markdown; charset=utf-8") }),
        )
        .route(
            "/docs/style.css",
            get(|| async { document(STYLE, "text/css; charset=utf-8") }),
        )
}

fn document(body: &'static str, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=300"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

fn page(title: &str, selected: &str, source: &str, markdown: &str) -> String {
    let mut navigation = String::new();
    for (path, label) in [
        ("/docs", "Integration guide"),
        ("/docs/architecture", "Architecture"),
        ("/docs/protocol", "Protocol reference"),
    ] {
        let current = if path == selected {
            " aria-current=\"page\""
        } else {
            ""
        };
        navigation.push_str(&format!("<a href=\"{path}\"{current}>{label}</a>"));
    }
    let article = render(markdown);
    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} · Agent Event Gateway</title><meta name="description" content="Integrate event sources, agent tools and downstream providers with Agent Event Gateway.">
<link rel="stylesheet" href="/docs/style.css"></head><body>
<a class="skip" href="#content">Skip to content</a>
<header><a href="/docs">Agent Event Gateway</a><span>Developer documentation</span></header>
<div class="layout"><nav aria-label="Documentation">{navigation}<a href="/openapi.json">OpenAPI JSON</a><a href="{source}" download>Download Markdown</a></nav>
<main id="content">{article}</main></div></body></html>"##
    )
}

fn render(markdown: &str) -> String {
    let mut events: Vec<_> = Parser::new_ext(markdown, Options::ENABLE_TABLES).collect();
    let mut headings = HashMap::new();
    for index in 0..events.len() {
        if matches!(&events[index], Event::Start(Tag::Heading { .. })) {
            let mut text = String::new();
            for event in &events[index + 1..] {
                match event {
                    Event::End(TagEnd::Heading(_)) => break,
                    Event::Text(value) | Event::Code(value) => text.push_str(value),
                    _ => {}
                }
            }
            let slug: String = text
                .to_lowercase()
                .chars()
                .filter(|ch| ch.is_alphanumeric() || ch.is_whitespace() || *ch == '-')
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("-");
            let count = headings.entry(slug.clone()).or_insert(0);
            let id = if *count == 0 {
                slug
            } else {
                format!("{slug}-{count}")
            };
            *count += 1;
            if let Event::Start(Tag::Heading { id: target, .. }) = &mut events[index] {
                *target = Some(id.into());
            }
        }
    }
    let mut links = Vec::new();
    let events = events.into_iter().filter_map(|event| match event {
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let target = link_target(&dest_url);
            links.push(target.is_some());
            target.map(|dest_url| {
                Event::Start(Tag::Link {
                    link_type,
                    dest_url: CowStr::from(dest_url),
                    title,
                    id,
                })
            })
        }
        Event::End(TagEnd::Link) if links.pop() != Some(true) => None,
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
        Event::Start(Tag::Table(_)) => Some(Event::Html("<div class=\"table\"><table>".into())),
        Event::End(TagEnd::Table) => Some(Event::Html("</table></div>".into())),
        other => Some(other),
    });
    let mut output = String::new();
    html::push_html(&mut output, events);
    output
}

fn link_target(target: &str) -> Option<String> {
    if target.starts_with("https://") || target.starts_with("http://") || target.starts_with('#') {
        return Some(target.into());
    }
    let (file, fragment) = target.split_once('#').unwrap_or((target, ""));
    let path = match file {
        "CMAEG_Integration.md" => "/docs",
        "CMAEG_Architecture.md" => "/docs/architecture",
        "CMAEG_Protocol.md" => "/docs/protocol",
        _ => return None,
    };
    Some(if fragment.is_empty() {
        path.into()
    } else {
        format!("{path}#{fragment}")
    })
}

#[cfg(test)]
mod tests;
