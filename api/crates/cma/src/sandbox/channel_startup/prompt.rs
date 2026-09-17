//! Prompt material only for startup plans backed by Channel creation provenance.
use layer2_core::bridge::allocation::AllocationError;

pub(crate) const SCHEMA_VERSION: u16 = 3;

// Immutable schema-2 material: retained home requests keep their original hash.
const CHANNEL_SYSTEM_INSTRUCTIONS_V2: &str = r#"[Channel delivery]
This Agent was created by a channel. Apply these instructions only to an active inbound channel input identified by its trusted channel delivery instructions; bootstrap and manual inputs do not initiate channel replies.
For each inbound channel input, your first actions must acknowledge the user through the native MCP transport tools before substantive reasoning, planning, skill or workspace reads, shell commands, or other work:
1. Call read_channel_context with {}.
2. Immediately call reply_to_channel with {"reply_context":"<returned reply_context>","request_key":"<unique message key>","text":"Received — ..."}. Replace the placeholders with the current context, a unique key, and a brief acknowledgement in the user's language.
No NyxID CLI, skill loading, bootstrap, or network discovery is needed for these native tools. Aim to issue the acknowledgement within 3 seconds of starting the input. If the answer needs more than 5 seconds of work, include a brief progress update in that first reply and continue working, sending further updates as needed. These are response targets, not guaranteed delivery times.
Use reply_to_channel for the eventual answer too. Final assistant text is visible only in CMA and is not automatically sent. Never reuse a reply_context from another input. Use a new request_key for each intentional message and the same request_key and text for retries. A queued receipt does not mean delivery has completed.
[/Channel delivery]"#;

const CHANNEL_PROGRESS_INSTRUCTIONS: &str = r#"[Channel work progress]
Apply this guidance only to the active inbound channel input described above. While work continues, proactively use reply_to_channel to explain every substantive tool call: before calling, give its safe tool name and concise purpose; afterward, give its status and a brief outcome or result summary. One update may cover tools launched together. Exclude read_channel_context, reply_to_channel, and edit_channel_reply from these notifications to avoid recursive updates.
Share brief user-facing progress summaries of findings, decisions, and next steps; never disclose hidden chain-of-thought, private deliberation, credentials, or raw authentication arguments. Summarize only safe details.
Aim to send progress about every 5 seconds during long-running work. Use background or asynchronous execution and short polling/yield intervals where supported so you can keep replying. Announce an expected delay before a blocking tool. This cadence cannot be guaranteed while the tool, model, or provider blocks.
[/Channel work progress]"#;

pub(crate) fn system_prompt(
    schema_version: u16,
    profile_prompt: &str,
) -> Result<String, AllocationError> {
    match schema_version {
        // Retained requests may already have materialized this exact content.
        1 => Ok(profile_prompt.to_owned()),
        2 => Ok(format!("{profile_prompt}\n\n{CHANNEL_SYSTEM_INSTRUCTIONS_V2}")),
        SCHEMA_VERSION => Ok(format!(
            "{profile_prompt}\n\n{CHANNEL_SYSTEM_INSTRUCTIONS_V2}\n\n{CHANNEL_PROGRESS_INSTRUCTIONS}"
        )),
        _ => Err(AllocationError::IdentityMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer3_adapter::{
        AgentHomeRequest, AgentSlot, PermissionLevel, ReasoningEffort, RuntimeOptions,
    };

    fn home(schema: u16, profile: &str) -> AgentHomeRequest {
        AgentHomeRequest::new(
            "agent:channel-created:home:create:v1",
            RuntimeOptions {
                model: None,
                reasoning_effort: ReasoningEffort::High,
                permission: PermissionLevel::Autonomous,
                own_credential: false,
                working_directory: "/work/channel-created".into(),
                home_directory: "/home/channel-created".into(),
                slot: AgentSlot::new("channel-created"),
            },
            system_prompt(schema, profile).unwrap(),
            Vec::new(),
        )
    }

    #[test]
    fn channel_home_prompt_preserves_profile_and_requires_tools_before_work() {
        let profile = "Use concise replies.\nPreserve this profile text exactly.\n";
        let current = home(SCHEMA_VERSION, profile);
        assert!(current.has_valid_sha256());
        assert!(current.system_prompt.starts_with(profile));
        for required in [
            "only to an active inbound channel input",
            "bootstrap and manual inputs do not initiate channel replies",
            "before substantive reasoning",
            "1. Call read_channel_context with {}.",
            "2. Immediately call reply_to_channel with",
            "\"reply_context\":\"<returned reply_context>\"",
            "\"request_key\":\"<unique message key>\"",
            "\"text\":\"Received — ...\"",
            "within 3 seconds",
            "more than 5 seconds",
            "not guaranteed delivery times",
            "same request_key and text for retries",
            "explain every substantive tool call",
            "before calling, give its safe tool name and concise purpose",
            "afterward, give its status and a brief outcome",
            "Exclude read_channel_context, reply_to_channel, and edit_channel_reply",
            "never disclose hidden chain-of-thought, private deliberation, credentials",
            "progress about every 5 seconds",
            "background or asynchronous execution and short polling/yield intervals",
            "Announce an expected delay before a blocking tool",
        ] {
            assert!(
                current.system_prompt.contains(required),
                "missing: {required}"
            );
        }
    }

    #[test]
    fn retained_channel_home_requests_keep_exact_legacy_prompt_and_digest() {
        let profile = "Use concise replies.\n";
        let legacy = home(1, profile);
        let retained: AgentHomeRequest =
            serde_json::from_slice(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(retained, legacy);
        assert_eq!(retained.system_prompt, profile);
        assert!(retained.has_valid_sha256());
        let current = home(SCHEMA_VERSION, profile);
        assert_ne!(current.request_sha256, retained.request_sha256);
        assert!(system_prompt(0, profile).is_err());
        assert!(system_prompt(SCHEMA_VERSION + 1, profile).is_err());
    }

    #[test]
    fn retained_schema_two_channel_home_keeps_original_acknowledgement_prompt() {
        let profile = "Use concise replies.\n";
        let original = home(2, profile);
        let retained: AgentHomeRequest =
            serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(retained, original);
        assert_eq!(
            retained.system_prompt,
            format!("{profile}\n\n{CHANNEL_SYSTEM_INSTRUCTIONS_V2}")
        );
        assert!(retained.has_valid_sha256());
        assert!(!retained.system_prompt.contains("[Channel work progress]"));
        let current = home(SCHEMA_VERSION, profile);
        assert!(current.system_prompt.starts_with(&retained.system_prompt));
        assert_ne!(current.request_sha256, retained.request_sha256);
        assert!(current.has_valid_sha256());
    }
}
