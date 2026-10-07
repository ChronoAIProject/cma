//! Bot-address and mention-key stripping shared by routing and turn rendering.

use cmaeg_core::{
    activity::SourceKind,
    channel::Platform,
    chat::{strip_address, strip_mention_keys},
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
