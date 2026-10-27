//! Authoring proves app/bot custody before storing a separate native source.
use crate::channels::management::{ManagementError, ManagementState};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ValidationError,
};
fn invalid(reason: &'static str) -> ManagementError {
    ManagementError::Invalid(ValidationError::new("/sources/native_lark", reason))
}
pub(crate) fn initialize(
    state: &ManagementState,
    current: Option<&ChannelDefinition>,
    next: &mut ChannelDefinition,
) -> Result<(), ManagementError> {
    let Some(Source::NativeLark(source)) = next
        .sources
        .iter()
        .find(|s| matches!(s, Source::NativeLark(_)))
    else {
        return Ok(());
    };
    if next.sources.iter().any(|s| matches!(s,Source::NyxidRelay { bot_id:Some(bot), .. } if bot == &source.identity.bot_open_id)) {
        return Err(invalid("native Lark requires a separately configured app and bot; do not replace a NyxID app webhook"));
    }
    if current.is_some_and(|old| {
        old.routing.is_some()
            && old.sources == next.sources
            && old.secrets.native_lark == next.secrets.native_lark
    }) {
        return Ok(());
    }
    if !state.native_lark_writes_enabled
        || !state.native_lark_admission_enabled
        || !state.multi_consumer_writes_enabled
    {
        return Err(invalid(
            "native Lark requires authoring, durable admission, recovery and routing support",
        ));
    }
    if next.reply.thread_contract.is_some() {
        return Err(invalid("native Lark thread contracts are not supported"));
    }
    if !next.consumers().filter(|c| !c.pending).all(|c| {
        state
            .providers
            .supports_response_replay(&c.provider.slug)
            .unwrap_or(false)
    }) {
        return Err(invalid(
            "native Lark requires durable provider response recovery",
        ));
    }
    if next.routing.is_none() {
        next.routing = Some(serde_json::from_value(serde_json::json!({"version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]})).map_err(|_|invalid("native routing unavailable"))?);
    }
    Ok(())
}
pub(crate) async fn verify(
    state: &ManagementState,
    old: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
) -> Result<(), ManagementError> {
    let Some(source @ Source::NativeLark(native)) = next
        .sources
        .iter()
        .find(|s| matches!(s, Source::NativeLark(_)))
    else {
        return Ok(());
    };
    let credentials = next
        .secrets
        .native_lark
        .as_ref()
        .ok_or_else(|| invalid("native credentials required"))?;
    if old.is_some_and(|old| {
        old.sources.contains(source) && old.secrets.native_lark.as_ref() == Some(credentials)
    }) {
        return Ok(());
    }
    let client = match &state.native_lark {
        Some(c) => (**c).clone(),
        None => super::Client::production()
            .map_err(|_| invalid("native custody verification unavailable"))?,
    };
    client.verify_custody(&native.identity, credentials).await.map_err(|_|invalid("native app/bot custody could not be proved; check the separate app, region and bot open_id"))
}
