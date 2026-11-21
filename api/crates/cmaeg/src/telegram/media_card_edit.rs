//! Media-card presentation uses the same file facts as ordinary media edits.
use super::{media_edit::PreparedEdit, Error, TelegramClient};
use crate::native_media::model::Snapshot;
use cmaeg_core::message::{NativeMediaKind, NativeMediaUpdate};
use serde_json::Value;

pub(crate) fn observe(
    value: &Value,
    bot: &str,
    chat: &str,
    thread: Option<&str>,
    message: &str,
    kind: NativeMediaKind,
    markup: &Value,
) -> Result<Snapshot, Error> {
    let empty = serde_json::json!({"inline_keyboard":[]});
    if value.get("reply_markup").unwrap_or(&empty) != markup {
        return Err(Error::Unconfirmed);
    }
    // Validate the keyboard explicitly, then reuse every existing file, topic,
    // bot, album and presentation check without granting plain-media authority.
    let mut plain = value.clone();
    plain
        .as_object_mut()
        .ok_or(Error::Unconfirmed)?
        .remove("reply_markup");
    let observed = super::media_edit::observe(&plain, bot, chat, thread, message, kind)?;
    if !observed.eligible
        || serde_json::to_vec(&observed.snapshot)
            .map_err(|_| Error::Unconfirmed)?
            .len()
            > 16384
    {
        return Err(Error::Unconfirmed);
    }
    Ok(observed.snapshot)
}

impl TelegramClient {
    pub(crate) async fn prepare_media_card_edit(
        &self,
        chat: &str,
        message: &str,
        current: &Snapshot,
        caption: &str,
        markup: &Value,
    ) -> Result<PreparedEdit, Error> {
        let patch = NativeMediaUpdate {
            replacement: None,
            caption: (caption != current.caption).then(|| caption.to_owned()),
            text: None,
        };
        let mut edit = self
            .prepare_media_edit(chat, message, current, &patch, None)
            .await?;
        let mut body: Value = serde_json::from_slice(&edit.body).map_err(|_| Error::Rejected)?;
        body["reply_markup"] = markup.clone();
        edit.body = serde_json::to_vec(&body).map_err(|_| Error::Rejected)?;
        Ok(edit)
    }
}
