//! Preserve Telegram's anonymous identity and aggregate facts without inventing a user.
use super::{positive, reactions, Error};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::{Actor, ActorKind},
    channel::{AdmissionPolicy, IdentityAdmission},
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) fn normalize(
    name: &str,
    value: &Value,
    chat: &str,
    admission: &AdmissionPolicy,
) -> Result<(Actor, Value), Error> {
    if name == "message_reaction_count" {
        if value.get("user").is_some() || value.get("actor_chat").is_some() {
            return Err(Error::Invalid);
        }
        open_senders(admission)?;
        return Ok((
            Actor {
                id: format!("telegram:anonymous:{chat}"),
                kind: ActorKind::Unknown,
                display_name: None,
            },
            json!({"observation":"aggregate", "reaction_counts":counts(&value["reactions"])?}),
        ));
    }
    let actor = if let Some(actor_chat) = value.get("actor_chat") {
        // Telegram's actor_chat names the represented chat, never the hidden
        // human. No numeric human allowlist may authorize that hidden identity.
        if value.get("user").is_some() {
            return Err(Error::Invalid);
        }
        open_senders(admission)?;
        let id = actor_chat["id"]
            .as_i64()
            .filter(|id| *id < 0)
            .ok_or(Error::Invalid)?;
        if !matches!(
            actor_chat["type"].as_str(),
            Some("group" | "supergroup" | "channel")
        ) {
            return Err(Error::Invalid);
        }
        Actor {
            id: format!("telegram:chat:{id}"),
            kind: ActorKind::Unknown,
            display_name: actor_chat["title"].as_str().map(display_name),
        }
    } else {
        let user = &value["user"];
        if user["is_bot"] != false {
            return Err(Error::Refused(Refusal::UnknownSender));
        }
        Actor {
            id: positive(&user["id"])?.to_string(),
            kind: ActorKind::Human,
            display_name: user["first_name"].as_str().map(display_name),
        }
    };
    let mut change = json!({"old_reactions":reactions(&value["old_reaction"])?,
        "new_reactions":reactions(&value["new_reaction"])?});
    if let Some(actor_chat) = value.get("actor_chat") {
        change["actor_chat"] = json!({"id":actor_chat["id"],"type":actor_chat["type"]});
    }
    Ok((actor, change))
}
fn display_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(128).collect()
}
fn open_senders(policy: &AdmissionPolicy) -> Result<(), Error> {
    match policy {
        AdmissionPolicy::All {}
        | AdmissionPolicy::Scoped {
            senders: IdentityAdmission::Open,
            ..
        } => Ok(()),
        _ => Err(Error::Refused(Refusal::UnknownSender)),
    }
}
fn counts(value: &Value) -> Result<Vec<Value>, Error> {
    let entries = value
        .as_array()
        .filter(|v| v.len() <= 64)
        .ok_or(Error::Invalid)?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for item in entries {
        if item.as_object().is_none_or(|obj| obj.len() != 2) {
            return Err(Error::Invalid);
        }
        let count = item["total_count"]
            .as_u64()
            .filter(|n| *n <= i32::MAX as u64)
            .ok_or(Error::Invalid)?;
        let reaction = reactions(&json!([item["type"]]))?.remove(0);
        let key = cmaeg_core::profile::canonical_json(reaction.clone());
        if !seen.insert(key) {
            return Err(Error::Invalid);
        }
        result.push(json!({"type":reaction,"total_count":count}));
    }
    result.sort_by_key(|v| cmaeg_core::profile::canonical_json(v["type"].clone()));
    Ok(result)
}
