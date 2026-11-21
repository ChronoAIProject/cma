//! Existing group custody and sends; no group-management mutations.
use super::{Client, Error};
use cmaeg_core::channel::{
    whatsapp::{valid_group_id, valid_meta_id},
    Source, WhatsappCredentials,
};
use reqwest::Method;
use serde_json::{json, Value};
use std::{collections::BTreeSet, time::Duration};

impl Client {
    /// Bounded read-only proof against the exact configured phone. Pagination
    /// uses only an opaque cursor on our pinned endpoint, never a returned URL.
    pub(crate) async fn verify_groups(
        &self,
        credentials: &WhatsappCredentials,
        phone: &str,
        groups: &[String],
        participant: Option<&str>,
    ) -> Result<(), Error> {
        if groups.is_empty()
            || groups.len() > 16
            || groups.iter().any(|id| !valid_group_id(id))
            || !valid_meta_id(phone)
            || credentials.validate().is_err()
        {
            return Err(Error::Invalid);
        }
        tokio::time::timeout(Duration::from_secs(12), async {
            let mut oba = Self::endpoint(phone)?;
            oba.query_pairs_mut()
                .append_pair("fields", "official_business_account");
            let answer = self
                .call(Method::GET, oba, credentials.access_token.as_str(), None)
                .await?;
            if answer["id"] != phone
                || answer["official_business_account"]["oba_status"] != "APPROVED"
            {
                return Err(Error::CustodyUnproved);
            }
            let mut missing: BTreeSet<_> = groups.iter().map(String::as_str).collect();
            let mut after: Option<String> = None;
            let mut seen = BTreeSet::new();
            // At most 1,024 active groups are inspected; later unproved groups
            // refuse rather than expand this request into unbounded discovery.
            for _ in 0..4 {
                let mut url = Self::endpoint(&format!("{phone}/groups"))?;
                url.query_pairs_mut().append_pair("limit", "256");
                if let Some(cursor) = &after {
                    url.query_pairs_mut().append_pair("after", cursor);
                }
                let answer = self
                    .call(Method::GET, url, credentials.access_token.as_str(), None)
                    .await?;
                let rows = answer["data"]["groups"]
                    .as_array()
                    .filter(|rows| rows.len() <= 256)
                    .ok_or(Error::CustodyUnproved)?;
                for row in rows {
                    let id = row["id"]
                        .as_str()
                        .filter(|id| valid_group_id(id))
                        .ok_or(Error::CustodyUnproved)?;
                    missing.remove(id);
                }
                if missing.is_empty() {
                    break;
                }
                if answer["paging"].get("next").is_none() {
                    break;
                }
                let cursor = answer["paging"]["cursors"]["after"]
                    .as_str()
                    .filter(|value| !value.is_empty() && value.len() <= 2048)
                    .ok_or(Error::CustodyUnproved)?;
                if !seen.insert(cursor.to_owned()) {
                    return Err(Error::CustodyUnproved);
                }
                after = Some(cursor.to_owned());
            }
            if !missing.is_empty() {
                return Err(Error::CustodyUnproved);
            }
            for group in groups {
                let mut url = Self::endpoint("")?;
                url.path_segments_mut()
                    .map_err(|_| Error::Invalid)?
                    .pop_if_empty()
                    .push(group);
                url.query_pairs_mut()
                    .append_pair("fields", "suspended,participants,total_participant_count");
                let answer = self
                    .call(Method::GET, url, credentials.access_token.as_str(), None)
                    .await?;
                let participants = answer["participants"]
                    .as_array()
                    .filter(|rows| !rows.is_empty() && rows.len() <= 8)
                    .ok_or(Error::CustodyUnproved)?;
                let mut ids = BTreeSet::new();
                if answer["id"] != *group
                    || answer["messaging_product"] != "whatsapp"
                    || answer["suspended"] != false
                    || answer["total_participant_count"].as_u64() != Some(participants.len() as u64)
                    || participants.iter().any(|value| {
                        !value["wa_id"]
                            .as_str()
                            .is_some_and(|id| valid_meta_id(id) && ids.insert(id))
                    })
                    || participant.is_some_and(|id| !ids.contains(id))
                {
                    return Err(Error::CustodyUnproved);
                }
            }
            Ok(())
        })
        .await
        .unwrap_or(Err(Error::Unconfirmed))
    }
    pub(crate) async fn verify_source_groups(
        &self,
        source: &Source,
        credentials: &WhatsappCredentials,
    ) -> Result<(), Error> {
        let Source::Whatsapp {
            phone_number_id,
            group_ids,
            ..
        } = source
        else {
            return Err(Error::Invalid);
        };
        if group_ids.is_empty() {
            return Ok(());
        }
        self.verify_groups(credentials, phone_number_id, group_ids, None)
            .await
    }

    pub(super) fn group_message_identity(result: &Value, group: &str) -> Result<String, Error> {
        let messages = result["messages"]
            .as_array()
            .filter(|messages| messages.len() == 1)
            .ok_or(Error::Unconfirmed)?;
        if result["messaging_product"] != "whatsapp" || messages[0]["group_id"] != group {
            return Err(Error::Unconfirmed);
        }
        messages[0]["id"]
            .as_str()
            .filter(|id| cmaeg_core::channel::whatsapp::valid_message_id(id))
            .map(str::to_owned)
            .ok_or(Error::Unconfirmed)
    }
    pub(crate) async fn send_group_text(
        &self,
        credentials: &WhatsappCredentials,
        reply: &cmaeg_core::channel::whatsapp::WhatsappGroupReply,
        text: &str,
    ) -> Result<String, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        if text.is_empty() || text.chars().count() > 4096 {
            return Err(Error::Invalid);
        }
        let result = self
            .call(
                Method::POST,
                Self::endpoint(&format!("{}/messages", reply.phone_number_id))?,
                credentials.access_token.as_str(),
                Some(json!({
                    "messaging_product": "whatsapp", "recipient_type": "group",
                    "to": reply.group_id, "context": {"message_id": reply.message_id},
                    "type": "text", "text": {"preview_url": false, "body": text}
                })),
            )
            .await?;
        Self::group_message_identity(&result, &reply.group_id)
    }
}

/// A revocation-only source update never needs provider availability or new-write authority.
pub(crate) fn retained_or_reduced(previous: &Source, next: &Source) -> bool {
    let (Source::Whatsapp { group_ids: old, .. }, Source::Whatsapp { group_ids: new, .. }) =
        (previous, next)
    else {
        return false;
    };
    if !new.iter().all(|id| old.contains(id)) {
        return false;
    }
    let mut stripped = next.clone();
    if let Source::Whatsapp {
        group_ids,
        forms_enabled,
        message_events,
        ..
    } = &mut stripped
    {
        *group_ids = old.clone();
        if !*message_events {
            if let Source::Whatsapp {
                message_events: previous_events,
                ..
            } = previous
            {
                *message_events = *previous_events;
            }
        }
        if !*forms_enabled {
            if let Source::Whatsapp {
                forms_enabled: previous_forms,
                ..
            } = previous
            {
                *forms_enabled = *previous_forms;
            }
        }
    }
    &stripped == previous
}

#[cfg(test)]
#[path = "group_tests.rs"]
pub(crate) mod tests;
