//! Leading chat addressing over the raw text: `@name` mentions and
//! `/to name[,name]`, each case pinned to its targets and `addressing_bytes`.
#[path = "routing_resolver/fixture.rs"]
mod fixture;
#[path = "routing_policy/fixture.rs"]
mod policy;

use cmaeg_core::{
    activity::{Activity, RouteResolution},
    channel::ChannelDefinition,
    routing::{
        addressing::{parse, validate_legacy_command, Addressed},
        RouteError,
    },
};
use fixture::{channel, command, input, lark, message, telegram, unrouted};
use policy::{NYX, REVIEWER};
use serde_json::{json, Value};

/// The Telegram relay's only route, distinct from the Lark relay's.
const TELEGRAM_ROUTE: &str = "7c1e2d3f-4a5b-4c6d-8e7f-9a0b1c2d3e4f";

type Parsed = Result<Option<Addressed>, RouteError>;

fn read(def: &ChannelDefinition, activity: &Activity) -> Parsed {
    parse(def, &input(def, activity))
}

fn routed() -> ChannelDefinition {
    channel(|_| {})
}

fn ids(targets: &[&str]) -> Vec<String> {
    targets.iter().map(|id| (*id).to_owned()).collect()
}

/// Assert `text`'s targets, form and `addressing_bytes`, and that the
/// remainder starts where the test says it does.
fn expect(parsed: Parsed, text: &str, targets: &[&str], form: RouteResolution, rest: &str) {
    let addressed = parsed
        .unwrap()
        .unwrap_or_else(|| panic!("{text:?} addressed no one"));
    let got: Vec<String> = addressed.targets.iter().map(|id| id.to_string()).collect();
    assert_eq!(got, ids(targets), "{text:?}");
    assert_eq!(addressed.resolution, form, "{text:?}");
    assert_eq!(
        &text[usize::from(addressed.addressing_bytes)..],
        rest,
        "{text:?}"
    );
}

fn mention(def: &ChannelDefinition, source: Value, text: &str, targets: &[&str], rest: &str) {
    let parsed = read(def, &message(source, text));
    expect(parsed, text, targets, RouteResolution::Mention, rest);
}

fn to(def: &ChannelDefinition, source: Value, text: &str, targets: &[&str], rest: &str) {
    let parsed = read(def, &message(source, text));
    expect(parsed, text, targets, RouteResolution::Command, rest);
}

fn plain(def: &ChannelDefinition, source: Value, text: &str) {
    assert_eq!(read(def, &message(source, text)), Ok(None), "{text:?}");
}

fn relay_telegram() -> Value {
    json!({"type": "nyxid_relay", "platform": "telegram", "route_id": TELEGRAM_ROUTE})
}

/// Lark and Telegram relays, each with its own bot username.
fn two_relays() -> ChannelDefinition {
    channel(|value| {
        let mut telegram = policy::relay("telegram", TELEGRAM_ROUTE);
        telegram["bot_username"] = "tgbot".into();
        value["sources"] = json!([policy::relay("lark", policy::ROUTE), telegram]);
    })
}

#[test]
fn legacy_relay_commands_require_the_explicit_default_target() {
    let def = two_relays();
    for (text, expected) in [
        ("ordinary message", Ok(())),
        ("/to default hello", Ok(())),
        ("/to main hello", Ok(())),
        ("/to@tgbot default hello", Ok(())),
        (
            "/to absent-fixture hello",
            Err(RouteError::TargetUnknown("absent-fixture".into())),
        ),
        ("/to reviewer hello", Err(RouteError::KillSwitchClosed)),
    ] {
        let activity = message(relay_telegram(), text);
        assert_eq!(
            validate_legacy_command(&def, &activity, def.source_of(&activity.source)),
            expected,
            "{text}"
        );
    }
}

#[test]
fn the_activitys_own_relay_supplies_the_bot_address() {
    let def = two_relays();
    let text = "@tgbot @reviewer x";
    mention(&def, relay_telegram(), text, &[REVIEWER], "x");
    // Lark addresses its bot with mention keys, never a username.
    plain(&def, lark(), text);
    // Another source's bot is not this activity's bot.
    plain(&def, relay_telegram(), "@lark_bot @reviewer x");
}

#[test]
fn a_command_may_carry_the_bot_suffix_or_follow_a_bot_address() {
    let def = routed();
    to(
        &def,
        telegram(),
        "/to@golden_bot reviewer x",
        &[REVIEWER],
        "x",
    );
    to(&def, telegram(), "/to@GOLDEN_BOT reviewer", &[REVIEWER], "");
    let text = "@golden_bot /to reviewer,main x";
    to(&def, telegram(), text, &[REVIEWER, "default"], "x");
    let text = "@_user_1 /to reviewer x";
    to(&def, lark(), text, &[REVIEWER], "x");
    to(
        &def,
        lark(),
        " @_user_1  @_user_2\t/to\tnyx  later",
        &[NYX],
        "later",
    );
}

#[test]
fn native_discord_bot_mentions_precede_consumer_commands_and_mentions() {
    let def = channel(|value| {
        value["sources"] = json!([{
            "type": "discord", "bot_id": "77", "intents": 37377,
            "admission": {"type": "all"}
        }]);
        value["secrets"]["discord_bot_token"] = "discord-routing-fixture".into();
    });
    let source = json!({"type": "discord", "bot_id": "77"});
    for text in [
        "<@77> /to reviewer 你好",
        "<@!77> /to reviewer 你好",
        "\u{3000}<@77>\t<@!77> /to reviewer 你好",
    ] {
        to(&def, source.clone(), text, &[REVIEWER], "你好");
    }
    mention(
        &def,
        source.clone(),
        "<@77> @reviewer please",
        &[REVIEWER],
        "please",
    );
    for text in [
        "<@78> /to reviewer please",
        "<@770> /to reviewer please",
        "<@&77> /to reviewer please",
        "<#77> /to reviewer please",
        "<@77 /to reviewer please",
        "hi <@77> /to reviewer please",
        "`<@77>` /to reviewer please",
    ] {
        plain(&def, source.clone(), text);
    }
    // Native addresses do not acquire meaning for another transport.
    plain(&routed(), telegram(), "<@77> /to reviewer please");
}

#[test]
fn malformed_commands_are_ordinary_text() {
    let def = routed();
    for text in [
        "/today x",
        "/to",
        "/to   ",
        "/to@other_bot reviewer",
        "/to@golden_botx reviewer",
        "/toreviewer",
        "please /to reviewer",
    ] {
        plain(&def, telegram(), text);
    }
}

#[test]
fn mentions_form_a_leading_run_that_may_end_the_message() {
    let def = routed();
    mention(&def, telegram(), "@reviewer", &[REVIEWER], "");
    mention(&def, telegram(), "  @reviewer  ", &[REVIEWER], "");
    let text = "@reviewer, @main: text";
    mention(&def, telegram(), text, &[REVIEWER, "default"], "text");
    mention(&def, telegram(), "@REVIEWER x", &[REVIEWER], "x");
    mention(
        &def,
        telegram(),
        &format!("@{REVIEWER} x"),
        &[REVIEWER],
        "x",
    );
    mention(&def, telegram(), "@default x", &["default"], "x");
    mention(&def, telegram(), "@golden_bot @nyx", &[NYX], "");
}

#[test]
fn the_run_stops_at_the_first_token_naming_no_consumer() {
    let def = routed();
    plain(&def, telegram(), "@x hello");
    plain(&def, telegram(), "@reviewer. done");
    plain(&def, telegram(), "@reviewer:now");
    plain(&def, telegram(), "@ reviewer");
    let text = "@reviewer @alice please look";
    mention(&def, telegram(), text, &[REVIEWER], "@alice please look");
    let text = "@reviewer @golden_bot hi";
    mention(&def, telegram(), text, &[REVIEWER], "@golden_bot hi");
}

#[test]
fn mentions_in_the_middle_of_the_text_never_route() {
    let def = routed();
    for text in ["hello @reviewer", "hi /to reviewer", "x@reviewer y"] {
        plain(&def, telegram(), text);
    }
}

#[test]
fn targets_keep_their_order_without_duplicates() {
    let def = routed();
    to(
        &def,
        telegram(),
        "/to nyx,reviewer,nyx x",
        &[NYX, REVIEWER],
        "x",
    );
    mention(&def, telegram(), "@reviewer @REVIEWER x", &[REVIEWER], "x");
}

#[test]
fn unknown_and_non_mentionable_targets_are_refused() {
    let def = routed();
    let unknown = read(&def, &message(telegram(), "/to reviewer,ghost x"));
    assert_eq!(unknown, Err(RouteError::TargetUnknown("ghost".into())));
    let empty = read(&def, &message(telegram(), "/to reviewer, nyx"));
    assert_eq!(empty, Err(RouteError::TargetUnknown(String::new())));
    let quiet = channel(|value| value["routing"]["consumers"][0]["mentionable"] = false.into());
    for text in [
        "/to reviewer x",
        "/to main,reviewer x",
        "@reviewer x",
        "@main @reviewer",
    ] {
        let refused = read(&quiet, &message(telegram(), text));
        assert_eq!(
            refused,
            Err(RouteError::TargetNotMentionable("reviewer".into())),
            "{text}"
        );
    }
}

#[test]
fn an_unknown_token_is_cut_to_64_bytes_on_a_char_boundary() {
    let def = routed();
    let token = format!("{}é{}", "a".repeat(63), "b".repeat(10));
    let refused = read(&def, &message(telegram(), &format!("/to {token}")));
    assert_eq!(refused, Err(RouteError::TargetUnknown("a".repeat(63))));
}

#[test]
fn pending_consumers_are_unknown() {
    let def = channel(|value| value["routing"]["consumers"][1]["pending"] = true.into());
    plain(&def, telegram(), "@nyx hello");
    let refused = read(&def, &message(telegram(), "/to nyx hello"));
    assert_eq!(refused, Err(RouteError::TargetUnknown("nyx".into())));
}

#[test]
fn max_targets_bounds_both_forms() {
    let def = channel(|value| value["routing"]["max_targets"] = 2.into());
    for text in ["/to reviewer,nyx,main x", "@reviewer @nyx @main x"] {
        let refused = read(&def, &message(telegram(), text));
        assert_eq!(refused, Err(RouteError::TooManyTargets), "{text}");
    }
    to(
        &def,
        telegram(),
        "/to reviewer,nyx,reviewer x",
        &[REVIEWER, NYX],
        "x",
    );
}

#[test]
fn each_form_can_be_switched_off() {
    let no_mentions = channel(|value| value["routing"]["addressing"] = json!({"mentions": false}));
    plain(&no_mentions, telegram(), "@reviewer x");
    to(&no_mentions, telegram(), "/to reviewer x", &[REVIEWER], "x");
    let no_command = channel(|value| value["routing"]["addressing"] = json!({"command": false}));
    plain(&no_command, telegram(), "/to reviewer x");
    mention(&no_command, telegram(), "@reviewer x", &[REVIEWER], "x");
    let off = command(telegram(), "to", "reviewer x");
    assert_eq!(read(&no_command, &off), Ok(None));
}

#[test]
fn a_to_command_is_parsed_over_its_args() {
    let def = routed();
    for (args, targets, rest) in [
        ("reviewer,nyx please", &[REVIEWER, NYX][..], "please"),
        ("  main", &["default"][..], ""),
    ] {
        let parsed = read(&def, &command(telegram(), "to", args));
        expect(parsed, args, targets, RouteResolution::Command, rest);
    }
    for (name, args) in [
        ("to", ""),
        ("to", "   "),
        ("status", "reviewer"),
        ("To", "reviewer"),
    ] {
        assert_eq!(
            read(&def, &command(telegram(), name, args)),
            Ok(None),
            "{name}"
        );
    }
}

#[test]
fn unicode_around_the_addressing_keeps_char_boundaries() {
    let def = routed();
    let text = " \u{3000}@golden_bot\u{3000}@reviewer 你好 @main";
    mention(&def, telegram(), text, &[REVIEWER], "你好 @main");
    to(
        &def,
        telegram(),
        "/to reviewer\u{2003}éè",
        &[REVIEWER],
        "éè",
    );
    plain(&def, telegram(), "你好 @reviewer");
    plain(&def, telegram(), "@révieweur x");
}

#[test]
fn channels_without_routing_have_no_addressing() {
    let def = unrouted();
    plain(&def, telegram(), "@reviewer x");
    plain(&def, telegram(), "/to reviewer x");
}
