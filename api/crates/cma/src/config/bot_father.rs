//! Explicit opt-in for an actor-authorized provisioning integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BotFatherConfig {
    pub binding_id: String,
    pub username: String,
}
impl BotFatherConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let binding = lookup("CHRONO_BOT_FATHER_BINDING_ID")?.filter(|v| !v.is_empty());
        let username = lookup("CHRONO_BOT_FATHER_TELEGRAM_USERNAME")?.filter(|v| !v.is_empty());
        match (binding, username) {
            (None, None) => Ok(None),
            (Some(binding_id), Some(username))
                if binding_id.starts_with("bnd_")
                    && binding_id.len() <= 128
                    && binding_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && username.len() >= 5
                    && username.len() <= 32
                    && username
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
            {
                Ok(Some(Self {
                    binding_id,
                    username,
                }))
            }
            _ => Err("Bot Father requires both a valid binding ID and Telegram username".into()),
        }
    }
}
