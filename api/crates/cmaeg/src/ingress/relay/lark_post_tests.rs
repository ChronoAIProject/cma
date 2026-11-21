//! Post flattening unit tests: each element and layout rule, and a seeded
//! invariant that every flattened post fits the activity's text and attachment
//! domain.
use super::*;
use crate::ingress::relay::seeded_rng::Rng;
use cmaeg_core::{
    activity::{validate_attachments, SourceKind},
    channel::Platform,
};
use serde_json::json;

const BOT: &str = "ou_bot";

fn mentions() -> Vec<Value> {
    vec![
        json!({"key": "@_user_1", "id": {"open_id": BOT, "union_id": "on_bot"}, "name": "Support Bot"}),
        json!({"key": "@_user_2", "id": {"open_id": "ou_alice"}, "name": "Alice"}),
    ]
}

/// The flattened text of one post whose single paragraph is `elements`.
fn paragraph(elements: Value) -> String {
    flatten(&json!({ "content": [elements] }), &mentions(), Some(BOT))
        .map(|post| post.text)
        .unwrap_or_default()
}

fn at(user_id: &str) -> Value {
    json!({"tag": "at", "user_id": user_id, "user_name": ""})
}

#[test]
fn a_title_and_inline_elements_join_into_paragraph_lines() {
    let post = json!({"title": "Weekly \t update", "content": [
        [{"tag": "text", "text": "Hello "}, {"tag": "a", "text": "docs", "href": "https://docs.example/a?b=1"},
         {"tag": "text", "text": "  and\u{a0}"}, {"tag": "emotion", "emoji_type": "SMILE"}],
        [],
        [{"tag": "text", "text": "   "}],
        [{"tag": "text", "text": "second"}],
    ]});
    let flattened = flatten(&post, &[], None).unwrap();
    assert_eq!(
        flattened.text,
        "Weekly update\nHello docs (https://docs.example/a?b=1) and :SMILE:\nsecond"
    );
    assert!(flattened.placeholders.is_empty());
}

#[test]
fn links_keep_only_absolute_web_targets() {
    let link = |href: &str| paragraph(json!([{"tag": "a", "text": "site", "href": href}]));
    assert_eq!(link("http://a.example"), "site (http://a.example)");
    let long = format!("https://a.example/{}", "p".repeat(2_048));
    for href in [
        "javascript:alert(1)",
        "/relative/path",
        "ftp://a.example/file",
        "https://a.example/\nnext",
        "https://a.example/\u{202E}gpj",
        "not a url",
        "",
        long.as_str(),
    ] {
        assert_eq!(link(href), "site", "{href}");
    }
    assert_eq!(paragraph(json!([{"tag": "a", "href": 7}])), "");
}

#[test]
fn mentions_render_names_without_the_pinned_bot() {
    let elements = json!([at("@_user_1"), {"tag": "text", "text": " ping "}, at("@_user_2")]);
    assert_eq!(paragraph(elements.clone()), "ping @Alice");
    // Unpinned, the gateway cannot tell which mention is the bot.
    let unpinned = flatten(&json!({ "content": [elements] }), &mentions(), None).unwrap();
    assert_eq!(unpinned.text, "@Support Bot ping @Alice");
}

#[test]
fn all_and_unresolvable_mentions() {
    for user_id in ["@_all", "all"] {
        assert_eq!(paragraph(json!([at(user_id)])), "@all");
    }
    let named = json!([{"tag": "at", "user_id": "@_user_9", "user_name": "Bob"}]);
    assert_eq!(paragraph(named), "@Bob");
    for user_name in [json!(""), json!("\u{202E}\u{7}"), json!(7)] {
        let unnamed = json!([{"tag": "at", "user_id": "@_user_9", "user_name": user_name}, {"tag": "text", "text": "hi"}]);
        assert_eq!(paragraph(unnamed), "hi");
    }
    assert_eq!(
        paragraph(json!([{"tag": "at"}, {"tag": "text", "text": "x"}])),
        "x"
    );
}

#[test]
fn titles_and_names_lose_controls_and_bidirectional_characters() {
    let post =
        json!({"title": "Re\u{202E}port\u{7} \u{2066}Q3\u{85}", "content": [[at("@_user_3")]]});
    let long = "N".repeat(70);
    for (name, expected) in [
        ("Al\u{200F}ice\n  Smith\u{1b}", "@Alice Smith".to_owned()),
        (long.as_str(), format!("@{}", "N".repeat(64))),
    ] {
        let mention = json!([{"key": "@_user_3", "id": {"open_id": "ou_3"}, "name": name}]);
        let text = flatten(&post, mention.as_array().unwrap(), Some(BOT))
            .unwrap()
            .text;
        assert_eq!(text, format!("Report Q3\n{expected}"));
    }
}

#[test]
fn code_blocks_and_rules_stand_on_their_own_lines() {
    let code = "func main() {\n\treturn 0\u{7}\n}";
    let text = paragraph(json!([
        {"tag": "text", "text": "before"},
        {"tag": "code_block", "language": "GO", "text": code},
        {"tag": "text", "text": " after "},
        {"tag": "hr"},
    ]));
    assert_eq!(
        text,
        "before\n```go\nfunc main() {\n\treturn 0\n}\n```\nafter\n---"
    );
    for (language, fence) in [
        ("C++", "```c++"),
        ("objective-c#_1", "```objective-c#_1"),
        ("bad lang", "```"),
        ("x```", "```"),
        ("文", "```"),
    ] {
        let block = paragraph(json!([{"tag": "code_block", "language": language, "text": "x"}]));
        assert_eq!(block, format!("{fence}\nx\n```"), "{language}");
    }
    let long = "a".repeat(33);
    let block = paragraph(json!([{"tag": "code_block", "language": long, "text": "x"}]));
    assert_eq!(block, "```\nx\n```");
}

#[test]
fn emotions_render_only_plain_identifiers() {
    assert_eq!(
        paragraph(json!([{"tag": "emotion", "emoji_type": "THUMBSUP_2"}])),
        ":THUMBSUP_2:"
    );
    let long = "A".repeat(65);
    for invalid in [json!("a b"), json!(":x:"), json!(""), json!(long), json!(3)] {
        assert_eq!(
            paragraph(json!([{"tag": "emotion", "emoji_type": invalid}])),
            ""
        );
    }
}

#[test]
fn embedded_media_become_placeholders_that_cannot_be_retrieved() {
    let post = json!({"content": [[
        {"tag": "img", "image_key": "img_v3_a"},
        {"tag": "media", "file_key": "file_v3_b", "image_key": "img_v3_cover"},
        {"tag": "img", "image_key": "has space"},
        {"tag": "media"},
        {"tag": "note", "elements": []},
    ]]});
    let flattened = flatten(&post, &[], None).unwrap();
    assert_eq!(flattened.text, "");
    let placeholders: Vec<_> = flattened
        .placeholders
        .iter()
        .map(|item| {
            (
                item.reference.as_str(),
                item.kind,
                item.variant,
                item.source.is_none(),
            )
        })
        .collect();
    let embedded = Some(MediaVariant::Embedded);
    assert_eq!(
        placeholders,
        [
            ("lark:img_v3_a", AttachmentKind::Image, embedded, true),
            ("lark:file_v3_b", AttachmentKind::Video, embedded, true),
            ("lark:item-2", AttachmentKind::Image, embedded, true),
            ("lark:item-3", AttachmentKind::Video, embedded, true),
        ]
    );
}

#[test]
fn media_beyond_the_attachment_bound_are_noted_once() {
    let images: Vec<_> = (0..20)
        .map(|index| json!([{"tag": "img", "image_key": format!("img_{index}")}]))
        .collect();
    let mut paragraphs = vec![json!([{"tag": "text", "text": "photos"}])];
    paragraphs.extend(images);
    paragraphs.push(json!([{"tag": "text", "text": "thanks"}]));
    let flattened = flatten(&json!({ "content": paragraphs }), &[], None).unwrap();
    assert_eq!(flattened.placeholders.len(), 16);
    assert_eq!(flattened.placeholders[15].reference, "lark:img_15");
    assert_eq!(
        flattened.text,
        "photos\n[more embedded media omitted]\nthanks"
    );
}

#[test]
fn oversized_posts_are_truncated_on_a_char_boundary() {
    let text = "文".repeat(14_000);
    let post = json!({"title": "title", "content": [[{"tag": "text", "text": text}], [{"tag": "text", "text": "lost"}]]});
    let flattened = flatten(&post, &[], None).unwrap();
    assert!(flattened.text.len() <= MAX_TEXT_BYTES);
    assert!(flattened.text.ends_with(MARKER));
    assert!(flattened.text.starts_with("title\n文文"));
    assert!(!flattened.text.contains("lost"));
    // Exactly at the budget, nothing is cut.
    let fits = "a".repeat(MAX_TEXT_BYTES - MARKER.len());
    let exact = flatten(
        &json!({"content": [[{"tag": "text", "text": fits}]]}),
        &[],
        None,
    );
    assert_eq!(exact.unwrap().text, fits);
}

#[test]
fn malformed_and_empty_posts_are_none() {
    for content in [
        json!("text"),
        json!([[{"tag": "text", "text": "x"}]]),
        json!({"title": "x"}),
        json!({"content": "x"}),
        json!({"content": [{"tag": "text", "text": "x"}]}),
        json!({"content": [["x"]]}),
        json!({"title": 7, "content": [[{"tag": "text", "text": "x"}]]}),
        json!({"title": "", "content": []}),
        json!({"content": [[], []]}),
        json!({"content": [[{"tag": "unknown", "text": "x"}, {"tag": "text", "text": " \u{7} "}]]}),
    ] {
        assert!(flatten(&content, &[], None).is_none(), "{content}");
    }
    let titled = flatten(&json!({"title": "only a title", "content": []}), &[], None);
    assert_eq!(titled.unwrap().text, "only a title");
}

/// Sender text; only element and title text may be long enough to truncate.
fn text(rng: &mut Rng, long: bool) -> String {
    const PIECES: [&str; 14] = [
        "a", "Z", " ", "\t", "\n", "文", "😀", "\u{0}", "\u{7}", "\u{85}", "\u{202E}", "\u{2069}",
        "@_user_1", "```",
    ];
    let length = if long {
        rng.pick(&[0, 1, 12, 300, 6_000, 20_000])
    } else {
        rng.pick(&[0, 1, 12, 80])
    };
    let mut value = String::new();
    while value.len() < length {
        value.push_str(rng.pick(&PIECES));
    }
    value
}

fn key(rng: &mut Rng) -> Value {
    match rng.below(3) {
        0 => json!(text(rng, false)),
        1 => json!(format!("img_{}", rng.below(1_000))),
        _ => rng.pick(&[Value::Null, json!(7), json!("k".repeat(1_025))]),
    }
}

fn element(rng: &mut Rng) -> Value {
    let tag = rng.pick(&[
        "text",
        "text",
        "a",
        "at",
        "emotion",
        "code_block",
        "hr",
        "img",
        "img",
        "media",
        "note",
    ]);
    let mut element = json!({ "tag": tag });
    for field in [
        "text",
        "href",
        "user_id",
        "user_name",
        "emoji_type",
        "language",
    ] {
        if rng.below(2) == 0 {
            element[field] = match field {
                "href" => {
                    json!(rng.pick(&["https://a.example/x", "javascript:x", "http://b.example"]))
                }
                "user_id" => json!(rng.pick(&["@_user_1", "@_user_2", "@_all", "@_user_9"])),
                field => json!(text(rng, field == "text")),
            };
        }
    }
    element["image_key"] = key(rng);
    element["file_key"] = key(rng);
    element
}

fn post(rng: &mut Rng) -> (Value, Vec<Value>) {
    let paragraphs: Vec<Value> = (0..rng.below(12))
        .map(|_| Value::Array((0..rng.below(8)).map(|_| element(rng)).collect()))
        .collect();
    let mut post = json!({ "content": paragraphs });
    if rng.below(2) == 0 {
        post["title"] = json!(text(rng, true));
    }
    let mentions = (1..=rng.below(4))
        .map(|index| json!({"key": format!("@_user_{index}"), "id": {"open_id": rng.pick(&[BOT, "ou_x"])}, "name": text(rng, false)}))
        .collect();
    (post, mentions)
}

#[test]
fn flatten_invariant() {
    let source = SourceKind::NyxidRelay {
        platform: Platform::Feishu,
        route_id: "route-1".into(),
    };
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut flattened, mut truncated, mut omitted) = (0, 0, 0);
    for case in 0..2_000 {
        let (post, mentions) = post(&mut rng);
        let bot_id = rng.pick(&[Some(BOT), None]);
        let Some(output) = flatten(&post, &mentions, bot_id) else {
            continue;
        };
        flattened += 1;
        truncated += usize::from(output.text.ends_with(MARKER));
        omitted += usize::from(output.text.contains(OMITTED));
        assert!(output.text.len() <= MAX_TEXT_BYTES, "case {case}");
        assert!(
            !output
                .text
                .chars()
                .any(|c| !matches!(c, '\n' | '\t') && forbidden_text_char(c)),
            "case {case}"
        );
        assert!(output.placeholders.len() <= MAX_ATTACHMENTS, "case {case}");
        if let Err(error) = validate_attachments(&output.placeholders, &source, "/kind/attachments")
        {
            panic!("case {case}: {error}");
        }
    }
    // The generator must reach both bounds, not only small posts.
    assert!(flattened > 1_000, "{flattened} flattened");
    assert!(
        truncated > 20 && omitted > 0,
        "{truncated} truncated, {omitted} omitted"
    );
}
