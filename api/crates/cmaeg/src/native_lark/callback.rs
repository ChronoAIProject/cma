//! Verified v2 card facts only; neither callback credential is retained.
use cmaeg_core::channel::{native_lark::safe_id, NativeLarkIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Callback {
    pub event_id: String,
    pub identity: NativeLarkIdentity,
    pub actor: String,
    pub chat: String,
    pub message: String,
    pub locator: String,
    pub form: Option<Map<String, Value>>,
    pub occurred_at: i64,
}
impl Callback {
    pub fn extract(raw: &Value, identity: &NativeLarkIdentity, now: i64) -> Result<Self, ()> {
        let header = &raw["header"];
        let event = &raw["event"];
        let action = &event["action"];
        let id = |value: &Value, prefix: &str| {
            value
                .as_str()
                .filter(|s| safe_id(s) && s.starts_with(prefix))
                .map(str::to_owned)
                .ok_or(())
        };
        if raw["schema"] != "2.0"
            || header["event_type"] != "card.action.trigger"
            || header["app_id"].as_str() != Some(&identity.app_id)
            || header["tenant_key"].as_str() != Some(&identity.tenant_key)
            || event["operator"]["tenant_key"].as_str() != Some(&identity.tenant_key)
            || event["host"] != "im_message"
            || event.get("delivery_type").is_some()
            || event["context"].get("url").is_some()
            || event["context"].get("preview_token").is_some()
            || action["tag"] != "button"
            || ["input_value", "option", "options", "checked"]
                .iter()
                .any(|key| action.get(*key).is_some())
            || action["value"].as_object().is_none_or(|v| v.len() != 1)
        {
            return Err(());
        }
        // Callback create_time is a decimal microsecond timestamp in the v2 contract.
        let stamp = header["create_time"].as_str().ok_or(())?;
        let micros = stamp.parse::<i64>().map_err(|_| ())?;
        let occurred_at = micros / 1_000_000;
        if micros.to_string() != stamp || occurred_at <= now - 86400 || occurred_at > now + 300 {
            return Err(());
        }
        let locator = action["value"]["cma_action"]
            .as_str()
            .filter(|s| s.len() <= 64)
            .ok_or(())?
            .to_owned();
        let form = match action.get("form_value") {
            None => None,
            Some(Value::Object(values))
                if values.len() <= 12
                    && serde_json::to_vec(values).map_err(|_| ())?.len() <= 16 * 1024 =>
            {
                Some(values.clone())
            }
            _ => return Err(()),
        };
        if (form.is_some() && (action["name"] != "submit" || !locator.starts_with("cnlf1:")))
            || (form.is_none() && (!locator.starts_with("cnl1:") || action.get("name").is_some()))
        {
            return Err(());
        }
        let result = Self {
            event_id: id(&header["event_id"], "")?,
            identity: identity.clone(),
            actor: id(&event["operator"]["open_id"], "ou_")?,
            chat: id(&event["context"]["open_chat_id"], "oc_")?,
            message: id(&event["context"]["open_message_id"], "om_")?,
            locator,
            form,
            occurred_at,
        };
        if result.actor == identity.bot_open_id {
            return Err(());
        }
        Ok(result)
    }
}
