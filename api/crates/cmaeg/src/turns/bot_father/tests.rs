use super::input::{draft, is_setup_activity};
use super::{input::request, view::SetupView};
use cmaeg_core::activity::{
    Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
    InteractionResult, SourceKind,
};
use cmaeg_core::channel::Platform;
use cmaeg_core::ids::EventId;
use cmaeg_core::message::{Block, OutboundMessage};
use serde_json::json;

pub(super) fn activity(text: &str) -> Activity {
    Activity {
        schema_version: 1,
        event_id: EventId::new("event").unwrap(),
        kind: ActivityKind::Message {
            text: text.into(),
            attachments: vec![],
            mentions_bot: false,
            reply_to: None,
        },
        actor: Actor {
            id: "actor".into(),
            display_name: None,
            kind: ActorKind::Human,
        },
        conversation: ActivityConversation {
            id: "chat".into(),
            kind: ConversationKind::Private,
            thread_id: None,
        },
        occurred_at: "2026-01-01T00:00:00Z".into(),
        source: SourceKind::NyxidRelay {
            platform: Platform::Telegram,
            route_id: "route".into(),
        },
    }
}

#[test]
fn ordinary_bot_father_messages_remain_provider_turns() {
    for text in [
        "hi",
        "How's the weather today?",
        "Search the web for today's news",
        "Help me create a Telegram bot",
        "帮我创建一个机器人",
        "",
        "create",
        "done",
        "back",
        "personal",
        "instructions: explain this code",
        "name: this is part of my document",
        "{\"agent\":{}}",
        "```json\n{\"agent\":{}}\n```",
        "/start",
        "/create",
        "/setupish create",
        "link_",
        "link_abc contains spaces",
        "/start link_abc extra",
        "/starting link_abc",
        "/start setup extra",
    ] {
        assert!(
            !is_setup_activity(&activity(text)),
            "unexpected setup routing: {text}"
        );
    }
}

#[test]
fn explicit_setup_controls_keep_the_durable_workflow() {
    for text in [
        "/setup",
        "/start setup",
        "/setup create",
        "/setup name: Paper assistant",
        "/setup instructions: Explain papers",
        "/setup {\"agent\":{}}",
        "/setup ```json\n{\"agent\":{}}\n```",
        "link_abcdef",
        "/start link_abc-123",
    ] {
        assert!(
            is_setup_activity(&activity(text)),
            "missing setup routing: {text}"
        );
    }
    assert!(!is_setup_activity(&activity(&format!(
        "link_{}",
        "a".repeat(129)
    ))));
}

#[test]
fn only_setup_commands_and_setup_callback_namespace_are_intercepted() {
    for (name, args, expected) in [
        ("setup", "", true),
        ("setup", "create", true),
        ("start", "setup", true),
        ("start", "link_abc-123", true),
        ("start", "", false),
        ("create", "", false),
        ("start", "link_abc extra", false),
        ("setupish", "create", false),
    ] {
        let mut input = activity("");
        input.kind = ActivityKind::Command {
            name: name.into(),
            args: args.into(),
        };
        assert_eq!(is_setup_activity(&input), expected, "{name} {args}");
    }
    for (block_id, expected) in [
        ("bst_0123456789abcdef0123456789abcdef.1", true),
        ("bst_0123456789abcdef0123456789abcdef.12", true),
        ("weather.1", false),
        ("bst_other.1", false),
        ("bst_0123456789abcdef0123456789abcdef.0", false),
        ("bst_0123456789abcdef0123456789abcdef.1.extra", false),
    ] {
        let mut input = activity("");
        input.kind = ActivityKind::Interaction(InteractionResult {
            block_id: block_id.into(),
            action_id: "create".into(),
            values: Default::default(),
        });
        assert_eq!(is_setup_activity(&input), expected, "{block_id}");
    }
}

#[test]
fn explicit_name_control_becomes_a_draft_name() {
    assert_eq!(
        draft(&activity("/setup name: Paper assistant"))["name"],
        "Paper assistant"
    );
}

#[test]
fn initial_request_extracts_name_and_instructions_without_copying_login_code() {
    let value = draft(&activity(
        "/setup 帮我创建一个叫 Paper assistant 的 Bot。Agent instructions：Explain papers.",
    ));
    assert_eq!(value["name"], "Paper assistant");
    assert_eq!(value["instructions"], "Explain papers.");
    assert_eq!(draft(&activity("link_abc")), json!({}));
    assert_eq!(draft(&activity("/start")), json!({}));
}

#[test]
fn advanced_json_is_forwarded_as_customization_and_never_as_a_bot_name() {
    let (draft, customization) = request(&activity(
        "/setup ```json\n{\"agent\": {\"system_prompt\": \"Explain papers\"}}\n```",
    ))
    .unwrap();
    assert_eq!(draft, json!({}));
    assert_eq!(customization["agent"]["system_prompt"], "Explain papers");
    // Individual field semantics belong to the CMA API, including rejection
    // of incomplete or unknown profile fields.
    let (_, customization) = request(&activity(
        "/setup {\"sandbox\": {\"cpu_millis\": 2000}, \"availability\": {\"always_keep_alive\": true}}",
    ))
    .unwrap();
    assert_eq!(customization["sandbox"]["cpu_millis"], 2000);
    assert_eq!(customization["availability"]["always_keep_alive"], true);
}

#[test]
fn malformed_or_ambiguous_customization_has_an_actionable_error() {
    for input in [
        "{\"agent\":",
        "```json\n{}",
        "{\"name\":\"Bot\"}",
        "{\"agent\":{},\"sandbox\":{}}",
        "{\"agent\":{},\"availability\":{}}",
        "{\"workspace\":null}",
    ] {
        let error = request(&activity(&format!("/setup {input}"))).unwrap_err();
        assert!(error.contains("current Customize step"), "{input}: {error}");
        assert!(error.contains("cancel"));
    }
}

fn setup(step: &str) -> serde_json::Value {
    json!({
        "step": step,
        "name": "Research Bot",
        "owner": {"kind": "organization", "subject": "org-chrono"},
        "configuration": {
            "agent": {"model": "research-model", "thinking_effort": "high", "permission_level": "guarded", "skills": [{"source":"ornn", "id":"paper-skill"}], "system_prompt": "Explain research papers"},
            "workspace": {"name": "research", "location": {"type": "git_repository", "url": "https://github.com/example/research", "reference": "main"}, "setup_commands": ["cargo build"], "agent_profiles": []},
            "sandbox": {"cpu_millis": 2000, "memory_mib": 4096, "nyxid_services": ["service-llm"], "agent_provider":"codex", "model_access":"chronoai_managed", "max_workspaces":1, "max_agents_per_workspace":1, "workspace_profiles":[], "software_install_commands":["apt-get update"], "environment_variables":[{"name":"PUBLIC_SETTING","value":"research"}]},
            "availability": {"warmup_enabled": false, "warmup_count": 1, "always_keep_alive": true}
        },
        "services": [
            {"id":"service-llm","slug":"llm","name":"Managed LLM","connected":true,"selected":true,"required":true},
            {"id":"service-excluded","slug":"excluded","name":"Unselected service","connected":true,"selected":false,"required":false}
        ],
        "external_bot_id": "selected-channel-bot",
        "admission": {"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"deny"},
        "identity": {"grant_id":"private-grant-sentinel", "subject":"private-subject-sentinel"},
        "choices": [{"id":"create:plan-digest", "label":"Create Bot", "selected":false}]
    })
}

fn rendered(message: &OutboundMessage) -> String {
    let mut parts = vec![message.text.clone().unwrap_or_default()];
    for block in &message.blocks {
        match block {
            Block::Text { text } => parts.push(text.clone()),
            Block::Card { title, body, .. } => {
                parts.push(title.clone());
                parts.push(body.clone().unwrap_or_default());
            }
            _ => {}
        }
    }
    parts.join("\n")
}

#[test]
fn review_shows_the_effective_plan_before_creation_without_identity_credentials() {
    let message = serde_json::from_value::<SetupView>(setup("review"))
        .unwrap()
        .message()
        .unwrap();
    message.validate().unwrap();
    let text = rendered(&message);
    for expected in [
        "org-chrono",
        "selected-channel-bot",
        "Managed LLM",
        "service-llm",
        "connected; required",
        "Admission policy",
        "research-model",
        "paper-skill",
        "https://github.com/example/research",
        "cargo build",
        "2000",
        "4096",
        "apt-get update",
        "PUBLIC_SETTING",
        "always_keep_alive",
        "Reply `/setup create`",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
    assert!(!text.contains("Unselected service"));
    assert!(!text.contains("private-grant-sentinel"));
    assert!(!text.contains("private-subject-sentinel"));
    assert!(
        text.find("Agent Profile — selected configuration").unwrap()
            < text.find("Create Bot —").unwrap()
    );
}

#[test]
fn custom_steps_show_basic_edits_and_complete_current_json_templates() {
    for (step, key, command) in [
        ("agent_custom", "agent", "instructions:"),
        ("workspace_custom", "workspace", "repository:"),
        ("sandbox_custom", "sandbox", "resources:"),
    ] {
        let message = serde_json::from_value::<SetupView>(setup(step))
            .unwrap()
            .message()
            .unwrap();
        let text = rendered(&message);
        assert!(text.contains(command));
        assert!(
            text.contains("reply `/setup done`")
                || text.contains("Reply `/setup done`")
                || text.contains("then `/setup done`")
        );
        assert!(text.contains(&format!("\"{key}\": {{")));
        if key == "sandbox" {
            assert!(text.contains("\"availability\": {"));
        }
    }
}

#[test]
fn review_keeps_a_maximum_size_unicode_prompt_intact_in_valid_blocks() {
    let mut value = setup("review");
    let prompt = "汉".repeat(10_922);
    value["configuration"]["agent"]["system_prompt"] = json!(prompt);
    let message = serde_json::from_value::<SetupView>(value)
        .unwrap()
        .message()
        .unwrap();
    message.validate().unwrap();
    let texts: String = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(texts.contains(&prompt));
}

#[test]
fn login_link_remains_visible_in_plain_text_for_existing_relays() {
    let mut value = setup("login");
    value["connection_url"] = json!("https://cma.example/auth/start");
    let message = serde_json::from_value::<SetupView>(value)
        .unwrap()
        .message()
        .unwrap();
    assert!(rendered(&message).contains("Sign in to NyxID: https://cma.example/auth/start"));
    assert!(!message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Link { .. })));
}
