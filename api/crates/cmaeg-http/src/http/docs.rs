//! One public integration document bundled with the gateway.
//! Operations runbooks stay private.
use axum::{
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::sync::{Arc, LazyLock};
mod diagrams;
mod render;
const GUIDE: &str = include_str!("../../../cmaeg/assets/docs/public/CMAEG_Integration.md");
const ARCHITECTURE: &str = include_str!("../../../cmaeg/assets/docs/public/CMAEG_Architecture.md");
const PROTOCOL: &str = include_str!("../../../cmaeg/assets/docs/public/CMAEG_Protocol.md");
const STYLE: &str = include_str!("docs.css");
const SECTIONS: [(&str, &str, &str); 3] = [
    ("integration", "Integration guide", GUIDE),
    ("architecture", "Architecture", ARCHITECTURE),
    ("protocol", "Protocol reference", PROTOCOL),
];
static PAGE: LazyLock<String> = LazyLock::new(page);
static DOWNLOAD: LazyLock<String> = LazyLock::new(download_markdown);

pub fn router() -> Router {
    router_with_base("", true)
}

/// Deployment URLs are configuration, not request Host or forwarding headers.
pub fn router_with_base(base: &str, advertise_canonical: bool) -> Router {
    let page = Arc::new(
        advertised_document(&PAGE, base, advertise_canonical)
            .replace("href=\"/docs", &format!("href=\"{base}/docs"))
            .replace("src=\"/docs", &format!("src=\"{base}/docs"))
            .replace(
                "href=\"/openapi.json",
                &format!("href=\"{base}/openapi.json"),
            ),
    );
    let origin = reqwest::Url::parse(base)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default();
    let csp = header::HeaderValue::from_str(&format!("default-src 'none'; style-src 'self' {origin}; img-src 'self' {origin}; base-uri 'none'; frame-ancestors 'none'"))
        .expect("validated public API origin");
    let download = Arc::new(advertised_document(&DOWNLOAD, base, advertise_canonical));
    let mut router = Router::new()
        .route(
            "/docs",
            get(move || {
                let page = page.clone();
                let csp = csp.clone();
                async move {
                    let mut response = document(page.as_str(), "text/html; charset=utf-8");
                    response
                        .headers_mut()
                        .insert(header::CONTENT_SECURITY_POLICY, csp);
                    response
                }
            }),
        )
        .route("/docs/", redirect(format!("{base}/docs")))
        .route(
            "/docs/architecture",
            redirect(format!("{base}/docs#architecture")),
        )
        .route("/docs/protocol", redirect(format!("{base}/docs#protocol")))
        .route(
            "/docs/download",
            get(move || {
                let download = download.clone();
                async move {
                    let mut response = document(&download, "text/markdown; charset=utf-8");
                    response.headers_mut().insert(
                        header::CONTENT_DISPOSITION,
                        header::HeaderValue::from_static(
                            "attachment; filename=\"agent-event-gateway.md\"",
                        ),
                    );
                    response
                }
            }),
        )
        .route(
            "/docs/integration.md",
            markdown(advertised_document(GUIDE, base, advertise_canonical)),
        )
        .route(
            "/docs/architecture.md",
            markdown(advertised_document(ARCHITECTURE, base, advertise_canonical)),
        )
        .route(
            "/docs/protocol.md",
            markdown(advertised_document(PROTOCOL, base, advertise_canonical)),
        )
        .route(
            "/docs/style.css",
            get(|| async { document(STYLE, "text/css; charset=utf-8") }),
        );
    for (_, name, svg) in diagrams::DIAGRAMS {
        router = router.route(
            &format!("/docs/diagrams/{name}.svg"),
            get(move || async move {
                let mut response = document(svg, "image/svg+xml");
                response.headers_mut().insert(
                    header::CONTENT_SECURITY_POLICY,
                    header::HeaderValue::from_static(
                        "default-src 'none'; style-src 'unsafe-inline'; sandbox",
                    ),
                );
                response
            }),
        );
    }
    router
}
fn advertised_document(source: &str, base: &str, advertise_canonical: bool) -> String {
    if base.is_empty() {
        return source.to_owned();
    }
    let source = if advertise_canonical {
        source.to_owned()
    } else {
        source.replace(
            "https://gateway.example.com/event-gateway/legacy-console",
            &format!("{base}/console"),
        )
    };
    source.replace("https://gateway.example.com/event-gateway", base)
}
fn markdown(body: String) -> axum::routing::MethodRouter {
    let body = Arc::new(body);
    get(move || {
        let body = body.clone();
        async move { document(&body, "text/markdown; charset=utf-8") }
    })
}
fn redirect(target: String) -> axum::routing::MethodRouter {
    get(move || {
        let target = target.clone();
        async move { axum::response::Redirect::permanent(&target) }
    })
}
fn document(body: &str, content_type: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=300"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body.to_owned(),
    )
        .into_response()
}
fn page() -> String {
    let mut navigation = String::new();
    let mut article = String::new();
    for (id, title, markdown) in SECTIONS {
        let (body, contents) = render::render(markdown, id);
        navigation.push_str(&format!(
            "<a class=\"section-title\" href=\"#{id}\">{title}</a>{contents}"
        ));
        article.push_str(&format!(
            "<section id=\"{id}\" aria-label=\"{title}\">{body}</section>"
        ));
    }
    format!(
        r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Documentation · Agent Event Gateway</title><meta name="description" content="Integration guide, architecture and complete protocol reference for Agent Event Gateway.">
<link rel="stylesheet" href="/docs/style.css"></head><body>
<a class="skip" href="#content">Skip to content</a>
<header><a class="brand" href="/docs">Agent Event Gateway</a><span>Developer documentation</span><a class="download" href="/docs/download" download="agent-event-gateway.md">Download document <span aria-hidden="true">↓</span></a></header>
<div class="layout"><nav aria-label="Sections"><p class="nav-label">ON THIS PAGE</p>{navigation}<a class="section-title" href="/openapi.json">OpenAPI JSON ↗</a></nav>
<main id="content"><h1>Agent Event Gateway</h1><p class="lead">Connect event sources to agents. Build channels, integrate providers and deliver replies using one gateway.</p>{article}</main></div></body></html>"##
    )
}
fn download_markdown() -> String {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    use std::collections::HashMap;
    let mut output = String::from("# Agent Event Gateway\n\nIntegration guide, architecture and protocol reference.\n\nOpenAPI: https://gateway.example.com/event-gateway/openapi.json\n\n");
    for (section, _, markdown) in SECTIONS {
        output.push_str(&format!("<a id=\"{section}\"></a>\n\n"));
        let events: Vec<_> = Parser::new_ext(markdown, Options::ENABLE_TABLES)
            .into_offset_iter()
            .collect();
        let mut headings = HashMap::new();
        let mut annotated = String::new();
        let mut cursor = 0;
        for (index, (event, range)) in events.iter().enumerate() {
            if !matches!(event, Event::Start(Tag::Heading { .. })) {
                continue;
            }
            let mut text = String::new();
            for (event, _) in &events[index + 1..] {
                match event {
                    Event::End(TagEnd::Heading(_)) => break,
                    Event::Text(value) | Event::Code(value) => text.push_str(value),
                    _ => {}
                }
            }
            let slug = text
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
            annotated.push_str(&markdown[cursor..range.start]);
            annotated.push_str(&format!("<a id=\"{section}-{id}\"></a>\n\n"));
            cursor = range.start;
        }
        annotated.push_str(&markdown[cursor..]);
        annotated = annotated.replace("](#", &format!("](#{section}-"));
        for (file, id) in [
            ("CMAEG_Integration.md", "integration"),
            ("CMAEG_Architecture.md", "architecture"),
            ("CMAEG_Protocol.md", "protocol"),
        ] {
            annotated = annotated
                .replace(&format!("]({file}#"), &format!("](#{id}-"))
                .replace(&format!("]({file})"), &format!("](#{id})"));
        }
        output.push_str(&annotated);
        output.push_str("\n\n---\n\n");
    }
    output
}
#[cfg(test)]
mod tests;
