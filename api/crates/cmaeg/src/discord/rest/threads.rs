//! Discord v10 lifecycle semantics pinned in docs/CMAEG_Protocol.md.
//! Preflight reads are separate so current custody can be checked immediately before a write.
use super::{valid_id, DiscordRest, RestError};
use crate::discord::thread_authority::Identity;
use cmaeg_core::{
    channel::Secret,
    thread_lifecycle::{CreateThreadRequest, ThreadAction, ThreadListView},
};
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) const LIST_LIMIT: usize = 25;
#[derive(Debug, Clone)]
pub(crate) struct Thread {
    pub identity: Identity,
    pub name: String,
    pub archived: bool,
    pub locked: bool,
    pub duration: u16,
    pub owner: String,
}
impl Thread {
    pub(crate) fn parse(value: &Value, expected: &Identity) -> Result<Self, RestError> {
        if !expected.thread() || !matches_channel(value, expected) {
            return Err(RestError::Rejected);
        }
        let metadata = &value["thread_metadata"];
        let name = value["name"]
            .as_str()
            .filter(|name| {
                !name.trim().is_empty()
                    && name.chars().count() <= 100
                    && !name.chars().any(cmaeg_core::activity::forbidden_text_char)
            })
            .ok_or(RestError::Rejected)?
            .to_owned();
        let owner = value["owner_id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or(RestError::Rejected)?
            .into();
        let duration = metadata["auto_archive_duration"]
            .as_u64()
            .filter(|v| matches!(v, 60 | 1440 | 4320 | 10080))
            .ok_or(RestError::Rejected)? as u16;
        Ok(Self {
            identity: expected.clone(),
            name,
            owner,
            duration,
            archived: metadata["archived"].as_bool().ok_or(RestError::Rejected)?,
            locked: metadata["locked"].as_bool().ok_or(RestError::Rejected)?,
        })
    }
}
fn matches_channel(value: &Value, expected: &Identity) -> bool {
    value["id"].as_str() == Some(expected.id.as_str())
        && value["type"].as_u64() == Some(u64::from(expected.kind))
        && value["guild_id"].as_str() == Some(expected.guild.as_str())
        && (!expected.thread() || value["parent_id"].as_str() == expected.parent.as_deref())
}
impl DiscordRest {
    pub(crate) async fn thread_channel(
        &self,
        token: &Secret,
        id: &str,
    ) -> Result<Value, RestError> {
        if !valid_id(id) {
            return Err(RestError::Rejected);
        }
        self.thread_get(token, &format!("channels/{id}")).await
    }
    pub(crate) async fn check_thread_origin(
        &self,
        token: &Secret,
        origin: &Identity,
    ) -> Result<Option<Thread>, RestError> {
        let value = self.thread_channel(token, &origin.id).await?;
        if !matches_channel(&value, origin) {
            return Err(RestError::Rejected);
        }
        if origin.thread() {
            Thread::parse(&value, origin).map(Some)
        } else {
            Ok(None)
        }
    }
    pub(crate) async fn list_threads(
        &self,
        token: &Secret,
        origin: &Identity,
        view: ThreadListView,
    ) -> Result<(Vec<Thread>, bool), RestError> {
        if let Some(thread) = self.check_thread_origin(token, origin).await? {
            let visible = thread.archived == (view == ThreadListView::Archived);
            return Ok((if visible { vec![thread] } else { vec![] }, false));
        }
        let path = match view {
            ThreadListView::Active => format!("guilds/{}/threads/active", origin.guild),
            ThreadListView::Archived => {
                format!("channels/{}/threads/archived/public?limit=26", origin.id)
            }
        };
        let value = self.thread_get(token, &path).await?;
        let rows = value["threads"].as_array().ok_or(RestError::Rejected)?;
        let mut threads = Vec::new();
        let mut seen = BTreeSet::new();
        let expected_type = origin.child(&origin.id).ok_or(RestError::Rejected)?.kind;
        for row in rows {
            // Active guild results include private threads and unrelated parents. Never expose them,
            // their membership data, their counts, or their effect authority to the original actor.
            if row["guild_id"].as_str() != Some(origin.guild.as_str())
                || row["parent_id"].as_str() != Some(origin.id.as_str())
                || row["type"].as_u64() != Some(u64::from(expected_type))
            {
                continue;
            }
            let id = row["id"]
                .as_str()
                .filter(|id| valid_id(id) && *id != origin.id)
                .ok_or(RestError::Rejected)?;
            if !seen.insert(id) {
                return Err(RestError::Rejected);
            }
            let identity = origin.child(id).ok_or(RestError::Rejected)?;
            let thread = Thread::parse(row, &identity)?;
            if thread.archived != (view == ThreadListView::Archived) {
                return Err(RestError::Rejected);
            }
            threads.push(thread);
        }
        let has_more = match view {
            ThreadListView::Active => false,
            ThreadListView::Archived => value["has_more"].as_bool().ok_or(RestError::Rejected)?,
        };
        let truncated = has_more || threads.len() > LIST_LIMIT;
        threads.truncate(LIST_LIMIT);
        Ok((threads, truncated))
    }
    async fn thread_get(&self, token: &Secret, path: &str) -> Result<Value, RestError> {
        self.ready(token)?;
        let response = self
            .client
            .get(self.url(path)?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .send()
            .await
            .map_err(|_| RestError::Unavailable)?;
        self.decode(response, token, false).await
    }
    pub(crate) async fn create_thread(
        &self,
        token: &Secret,
        target: &Identity,
        bot: &str,
        request: &CreateThreadRequest,
    ) -> Result<Thread, RestError> {
        request.validate().map_err(|_| RestError::Rejected)?;
        let parent = target.parent.as_ref().ok_or(RestError::Rejected)?;
        self.ready(token)?;
        let response = self
            .client
            .post(self.url(&format!("channels/{parent}/messages/{}/threads", target.id))?)
            .header("Authorization", format!("Bot {}", token.as_str()))
            .json(request)
            .send()
            .await
            .map_err(|_| RestError::Unknown)?;
        let value = self.decode(response, token, true).await?;
        created(&value, target, bot, request)
    }
    /// Safe deterministic reconciliation: this endpoint creates at source message ID, once.
    /// A read may confirm the exact bot-owned result; absence never permits another POST.
    pub(crate) async fn reconcile_created_thread(
        &self,
        token: &Secret,
        target: &Identity,
        bot: &str,
        request: &CreateThreadRequest,
    ) -> Result<Thread, RestError> {
        let value = self.thread_channel(token, &target.id).await?;
        created(&value, target, bot, request)
    }
    pub(crate) async fn manage_thread(
        &self,
        token: &Secret,
        target: &Identity,
        action: ThreadAction,
    ) -> Result<Option<Thread>, RestError> {
        self.ready(token)?;
        match action {
            ThreadAction::Archive | ThreadAction::Reopen => {
                let archived = action == ThreadAction::Archive;
                let response = self
                    .client
                    .patch(self.url(&format!("channels/{}", target.id))?)
                    .header("Authorization", format!("Bot {}", token.as_str()))
                    .json(&json!({"archived":archived}))
                    .send()
                    .await
                    .map_err(|_| RestError::Unknown)?;
                let value = self.decode(response, token, true).await?;
                let thread = Thread::parse(&value, target).map_err(|_| RestError::Unknown)?;
                if thread.archived != archived {
                    return Err(RestError::Unknown);
                }
                Ok(Some(thread))
            }
            ThreadAction::Join | ThreadAction::Leave => {
                let method = if action == ThreadAction::Join {
                    Method::PUT
                } else {
                    Method::DELETE
                };
                self.effect_request(
                    token,
                    method,
                    self.url(&format!("channels/{}/thread-members/@me", target.id))?,
                )
                .await?;
                Ok(None)
            }
        }
    }
}
fn created(
    value: &Value,
    target: &Identity,
    bot: &str,
    request: &CreateThreadRequest,
) -> Result<Thread, RestError> {
    let thread = Thread::parse(value, target).map_err(|_| RestError::Unknown)?;
    if thread.owner != bot
        || thread.name != request.name
        || thread.duration != request.auto_archive_duration
    {
        return Err(RestError::Unknown);
    }
    Ok(thread)
}
