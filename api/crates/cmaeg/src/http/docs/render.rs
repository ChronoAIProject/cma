use pulldown_cmark::{
    html, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use std::collections::HashMap;

pub(super) fn render(markdown: &str, section: &str) -> (String, String) {
    let mut navigation = String::new();
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
            let id = format!("{section}-{id}");
            if let Event::Start(Tag::Heading {
                id: target, level, ..
            }) = &mut events[index]
            {
                if matches!(*level, HeadingLevel::H2)
                    || (matches!(*level, HeadingLevel::H3) && text.starts_with("Flow "))
                {
                    navigation.push_str(&format!("<a href=\"#{id}\">{}</a>", escape(&text)));
                }
                *target = Some(id.into());
                *level = demote(*level);
            }
        }
    }
    let mut rendered = Vec::new();
    let mut source = events.into_iter();
    let mut heading = String::new();
    let mut in_heading = false;
    while let Some(event) = source.next() {
        match &event {
            Event::Start(Tag::Heading { .. }) => {
                heading.clear();
                in_heading = true;
            }
            Event::End(TagEnd::Heading(_)) => in_heading = false,
            Event::Text(text) | Event::Code(text) if in_heading => heading.push_str(text),
            _ => {}
        }
        if matches!(&event, Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(kind))) if kind.as_ref() == "mermaid")
        {
            let mut code = String::new();
            for event in source.by_ref() {
                match event {
                    Event::End(TagEnd::CodeBlock) => break,
                    Event::Text(text) => code.push_str(&text),
                    _ => {}
                }
            }
            let hash = ring::digest::digest(&ring::digest::SHA256, code.trim().as_bytes())
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if let Some((_, name, svg)) = super::diagrams::DIAGRAMS
                .iter()
                .find(|(digest, _, _)| *digest == hash)
            {
                let dimensions = diagram_size(svg)
                    .map(|(width, height)| format!(" width=\"{width}\" height=\"{height}\""))
                    .unwrap_or_default();
                rendered.push(Event::Html(format!("<figure class=\"diagram\"><a href=\"/docs/diagrams/{name}.svg\" target=\"_blank\" rel=\"noopener\" aria-label=\"Open full-size flow diagram\"><img src=\"/docs/diagrams/{name}.svg\"{dimensions} alt=\"{} — service flow diagram\" loading=\"lazy\"></a><figcaption>Open diagram in a new tab to enlarge</figcaption></figure>", escape(&heading)).into()));
            } else {
                rendered.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
                    "mermaid".into(),
                ))));
                rendered.push(Event::Text(code.into()));
                rendered.push(Event::End(TagEnd::CodeBlock));
            }
        } else if !matches!(&event, Event::Html(_) | Event::InlineHtml(_)) {
            rendered.push(event);
        }
    }
    let mut links = Vec::new();
    let events = rendered.into_iter().filter_map(|event| match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let target = link_target(&dest_url, section);
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
        Event::End(TagEnd::Heading(level)) => Some(Event::End(TagEnd::Heading(demote(level)))),
        other => Some(other),
    });
    let mut output = String::new();
    html::push_html(&mut output, events);
    (output, navigation)
}

// Reserve each reviewed SVG's aspect ratio before lazy loading. Otherwise a
// deep link can land several sections early as preceding diagrams acquire size.
fn diagram_size(svg: &str) -> Option<(u32, u32)> {
    let view_box = svg.split_once("viewBox=\"")?.1.split('"').next()?;
    let values = view_box
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() != 4
        || values[2..]
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0 || *value > 100_000.0)
    {
        return None;
    }
    Some((values[2].ceil() as u32, values[3].ceil() as u32))
}

fn demote(level: HeadingLevel) -> HeadingLevel {
    match level {
        HeadingLevel::H1 => HeadingLevel::H2,
        HeadingLevel::H2 => HeadingLevel::H3,
        HeadingLevel::H3 => HeadingLevel::H4,
        HeadingLevel::H4 => HeadingLevel::H5,
        _ => HeadingLevel::H6,
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn link_target(target: &str, section: &str) -> Option<String> {
    if target.starts_with("https://") || target.starts_with("http://") {
        return Some(target.into());
    }
    let (file, fragment) = target.split_once('#').unwrap_or((target, ""));
    let section = match file {
        "" => section,
        "CMAEG_Integration.md" => "integration",
        "CMAEG_Architecture.md" => "architecture",
        "CMAEG_Protocol.md" => "protocol",
        _ => return None,
    };
    Some(if fragment.is_empty() {
        format!("#{section}")
    } else {
        format!("#{section}-{fragment}")
    })
}
