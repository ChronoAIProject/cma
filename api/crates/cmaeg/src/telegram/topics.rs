//! Exact forum-topic operations. No caller-selected chat or topic reaches this adapter.
use super::{Error, TelegramClient};
use cmaeg_core::channel::Secret;
use serde_json::json;

/// Keep unit-variant bytes stable: existing close/reopen ledger hashes use them.
/// Rename includes the exact name, so reusing its key with another name conflicts.
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mutation<'a> {
    Close,
    Reopen,
    Rename { name: &'a str },
}

/// General is proven from the retained native forum envelope, never a missing ID alone.
pub(crate) enum Scope {
    Numbered(String),
    General,
}
impl Scope {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Numbered(id) => id,
            Self::General => "1",
        }
    }
}

impl TelegramClient {
    /// Requires administrator rights even for a creator-owned topic.
    /// Reads are completed before the shared tool performs its final custody check.
    pub(crate) async fn topic_permissions(
        &self,
        token: &Secret,
        bot: &str,
        chat: &str,
    ) -> Result<(), Error> {
        if !super::media_token_matches(token, bot) || !valid_chat(chat) {
            return Err(Error::Rejected);
        }
        let identity = self.get_me(token).await?;
        if identity.id != bot {
            return Err(Error::Rejected);
        }
        let value = self
            .call(token, "getChat", &json!({"chat_id":chat}))
            .await?;
        if value["id"].as_i64().map(|v| v.to_string()).as_deref() != Some(chat)
            || value["type"] != "supergroup"
            || value["is_forum"] != true
        {
            return Err(Error::Rejected);
        }
        let value = self
            .call(
                token,
                "getChatMember",
                &json!({"chat_id":chat,"user_id":bot.parse::<i64>().map_err(|_| Error::Rejected)?}),
            )
            .await?;
        if value["user"]["id"]
            .as_i64()
            .map(|v| v.to_string())
            .as_deref()
            != Some(bot)
            || value["user"]["is_bot"] != true
            || !(value["status"] == "creator"
                || (value["status"] == "administrator" && value["can_manage_topics"] == true))
        {
            return Err(Error::Rejected);
        }
        Ok(())
    }

    pub(crate) async fn manage_topic(
        &self,
        token: &Secret,
        bot: &str,
        chat: &str,
        topic: &Scope,
        action: Mutation<'_>,
    ) -> Result<(), Error> {
        if !super::media_token_matches(token, bot)
            || !valid_chat(chat)
            || matches!(topic, Scope::Numbered(id) if !valid_topic(id))
        {
            return Err(Error::Rejected);
        }
        let mut body = json!({"chat_id":chat});
        if let Scope::Numbered(id) = topic {
            body["message_thread_id"] = json!(id.parse::<i64>().map_err(|_| Error::Rejected)?);
        }
        let method = match (action, topic) {
            (Mutation::Close, Scope::General) => "closeGeneralForumTopic",
            (Mutation::Reopen, Scope::General) => "reopenGeneralForumTopic",
            (Mutation::Close, Scope::Numbered(_)) => "closeForumTopic",
            (Mutation::Reopen, Scope::Numbered(_)) => "reopenForumTopic",
            (Mutation::Rename { name }, scope) => {
                // Telegram accepts 128 characters; the shared public contract is 100.
                if name.trim().is_empty()
                    || name.chars().count() > 100
                    || name.chars().any(cmaeg_core::activity::forbidden_text_char)
                {
                    return Err(Error::Rejected);
                }
                body["name"] = json!(name);
                match scope {
                    Scope::General => "editGeneralForumTopic",
                    Scope::Numbered(_) => "editForumTopic",
                }
            }
        };
        let result = self.call(token, method, &body).await?;
        if result != true {
            return Err(Error::Unconfirmed);
        }
        Ok(())
    }
}
pub(crate) fn valid_chat(chat: &str) -> bool {
    chat.parse::<i64>()
        .is_ok_and(|id| id < 0 && id.to_string() == chat)
}
pub(crate) fn valid_topic(topic: &str) -> bool {
    // General is topic 1 and has distinct Bot API operations. Never infer it from no topic.
    topic
        .parse::<i64>()
        .is_ok_and(|id| id > 1 && id.to_string() == topic)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn topic_consent_preserves_original_typed_source_bytes_and_chat_local_scope_rules() {
        let legacy = r#"{"type":"telegram","bot_id":"77","admission":{"type":"all"}}"#;
        let mut source: cmaeg_core::channel::Source = serde_json::from_str(legacy).unwrap();
        assert_eq!(serde_json::to_string(&source).unwrap(), legacy);
        let cmaeg_core::channel::Source::Telegram {
            topic_lifecycle, ..
        } = &mut source
        else {
            panic!()
        };
        *topic_lifecycle = true;
        assert_eq!(
            serde_json::to_string(&crate::telegram::messaging_source(&source)).unwrap(),
            legacy
        );
        assert!(valid_chat("-100"));
        assert!(valid_topic("123"));
        for topic in ["1", "0", "-1", "0123", "+123"] {
            assert!(!valid_topic(topic));
        }
        for chat in ["0", "100", "-0100", "+100"] {
            assert!(!valid_chat(chat));
        }
    }
}
