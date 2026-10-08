//! Bot-address and mention-key stripping shared by routing and turn rendering.

use cmaeg_core::{
    activity::SourceKind,
    channel::Platform,
    chat::{bot_address_len, leading_address_end, strip_address, strip_mention_keys},
};

fn telegram() -> SourceKind {
    SourceKind::Telegram {
        platform: Platform::Telegram,
        bot_id: "42".into(),
    }
}

fn relay(platform: Platform) -> SourceKind {
    SourceKind::NyxidRelay {
        platform,
        route_id: "route".into(),
    }
}

#[test]
fn strips_a_leading_middle_or_trailing_bot_address() {
    for (text, expected) in [
        ("@bot hello", "hello"),
        ("hi @bot there", "hi  there"),
        ("hello @bot", "hello"),
        ("@bot, are you there?", ", are you there?"),
    ] {
        assert_eq!(
            strip_address(text, &telegram(), Some("bot")),
            expected,
            "{text}"
        );
    }
}

#[test]
fn matches_the_bot_address_case_insensitively() {
    assert_eq!(
        strip_address("@BoT hello", &telegram(), Some("bot")),
        "hello"
    );
    assert_eq!(
        strip_address("@bot hello", &telegram(), Some("BOT")),
        "hello"
    );
}

#[test]
fn keeps_addresses_that_are_not_word_bounded() {
    for text in ["@botx hi", "x@bot hi", "@bot_ hi", "@@bot hi", "@bot@ hi"] {
        assert_eq!(
            strip_address(text, &telegram(), Some("bot")),
            text,
            "{text}"
        );
    }
}

#[test]
fn strips_every_occurrence() {
    assert_eq!(
        strip_address("@bot a @bot b @BOT", &telegram(), Some("bot")),
        "a  b"
    );
}

#[test]
fn applies_the_username_to_relay_platforms_without_mention_keys() {
    for platform in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        assert_eq!(
            strip_address("@bot status", &relay(platform), Some("bot")),
            "status"
        );
    }
}

#[test]
fn leaves_text_untouched_without_a_username() {
    for text in ["@bot hello", " padded @bot ", ""] {
        assert_eq!(strip_address(text, &telegram(), None), text, "{text}");
    }
    assert_eq!(
        strip_address("@bot hello", &SourceKind::Custom, None),
        "@bot hello"
    );
}

#[test]
fn strips_only_leading_lark_and_feishu_mention_keys() {
    for platform in [Platform::Lark, Platform::Feishu] {
        for (text, expected) in [
            ("@_user_1 @_user_2 text", "text"),
            (
                " @_user_1  @_user_12 ask @_user_2 later",
                "ask @_user_2 later",
            ),
            ("@_user_ keep", "@_user_ keep"),
            ("@_user_1x keep", "@_user_1x keep"),
            ("@_user_1", ""),
        ] {
            assert_eq!(
                strip_address(text, &relay(platform), None),
                expected,
                "{text}"
            );
        }
        // The username never applies on these platforms: their keys replace it.
        assert_eq!(
            strip_address("@_user_1 @bot text", &relay(platform), Some("bot")),
            "@bot text"
        );
    }
}

#[test]
fn mention_keys_need_digits_and_a_following_space() {
    assert_eq!(strip_mention_keys("@_user_1 @_user_2 text"), "text");
    assert_eq!(strip_mention_keys("@_user_ text"), "@_user_ text");
    assert_eq!(strip_mention_keys("@_user_1x text"), "@_user_1x text");
    assert_eq!(strip_mention_keys("text @_user_1"), "text @_user_1");
    assert_eq!(strip_mention_keys("  @_user_3\ttext  "), "text");
}

#[test]
fn an_address_alone_leaves_an_empty_remainder() {
    assert_eq!(strip_address("@bot", &telegram(), Some("bot")), "");
    assert_eq!(strip_address("  @bot  ", &telegram(), Some("bot")), "");
    assert_eq!(strip_address("@bot @bot", &telegram(), Some("bot")), "");
    assert_eq!(strip_mention_keys("@_user_1 @_user_2"), "");
    assert_eq!(strip_mention_keys(""), "");
}

/// Every bot-address fixture above, with where the routing resolver's
/// leading-run scanner says the leading run of addresses ends.
fn scanned_fixtures() -> Vec<(&'static str, SourceKind, Option<&'static str>, usize)> {
    let mut fixtures = vec![
        ("@bot hello", telegram(), Some("bot"), 5),
        ("hi @bot there", telegram(), Some("bot"), 0),
        ("hello @bot", telegram(), Some("bot"), 0),
        ("@bot, are you there?", telegram(), Some("bot"), 4),
        ("@BoT hello", telegram(), Some("bot"), 5),
        ("@bot hello", telegram(), Some("BOT"), 5),
        ("@botx hi", telegram(), Some("bot"), 0),
        ("x@bot hi", telegram(), Some("bot"), 0),
        ("@bot_ hi", telegram(), Some("bot"), 0),
        ("@@bot hi", telegram(), Some("bot"), 0),
        ("@bot@ hi", telegram(), Some("bot"), 0),
        ("@bot a @bot b @BOT", telegram(), Some("bot"), 5),
        ("@bot hello", telegram(), None, 0),
        (" padded @bot ", telegram(), None, 1),
        ("", telegram(), None, 0),
        ("@bot hello", SourceKind::Custom, None, 0),
        ("@bot", telegram(), Some("bot"), 4),
        ("  @bot  ", telegram(), Some("bot"), 8),
        ("@bot @bot", telegram(), Some("bot"), 9),
    ];
    for platform in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        fixtures.push(("@bot status", relay(platform), Some("bot"), 5));
    }
    for platform in [Platform::Lark, Platform::Feishu] {
        fixtures.extend([
            ("@_user_1 @_user_2 text", relay(platform), None, 18),
            (
                " @_user_1  @_user_12 ask @_user_2 later",
                relay(platform),
                None,
                21,
            ),
            ("@_user_ keep", relay(platform), None, 0),
            ("@_user_1x keep", relay(platform), None, 0),
            ("@_user_1", relay(platform), None, 8),
            ("@_user_1 @bot text", relay(platform), Some("bot"), 9),
        ]);
    }
    fixtures
}

#[test]
fn the_leading_run_scanner_agrees_with_strip_address() {
    for (text, source, username, expected) in scanned_fixtures() {
        let end = leading_address_end(text, &source, username);
        assert_eq!(end, expected, "{text:?}");
        // Everything the scanner skips, the renderer strips too...
        assert_eq!(
            strip_address(&text[..end], &source, username).trim(),
            "",
            "{text:?}"
        );
        // ...and it skips nothing the renderer keeps: stripping the rest
        // alone renders exactly what stripping the whole text renders.
        assert_eq!(
            strip_address(&text[end..], &source, username).trim(),
            strip_address(text, &source, username).trim(),
            "{text:?}"
        );
    }
}

#[test]
fn a_bot_address_at_the_start_is_measured_with_the_same_rules() {
    assert_eq!(bot_address_len("@bot reviewer", "bot"), Some(4));
    assert_eq!(bot_address_len("@BOT", "bot"), Some(4));
    for text in ["@botx reviewer", "@bot_ x", "@bot@x", "bot x", "", " @bot"] {
        assert_eq!(bot_address_len(text, "bot"), None, "{text:?}");
    }
}
