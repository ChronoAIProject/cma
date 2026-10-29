//! Preserve the historical scalar shape while bounding new ordered upload facts.
use super::{discord::Identity, discord_media::Media, Error};
use crate::media::outbound::PreparedFile;
use cmaeg_core::message::{OutboundMessage, MAX_NATIVE_ATTACHMENTS};

impl Identity {
    pub(crate) fn media_items(&self) -> Result<&[Media], Error> {
        match (&self.media, &self.media_set) {
            (None, None) => Ok(&[]),
            (Some(media), None) => Ok(std::slice::from_ref(media)),
            (None, Some(items)) if (2..=MAX_NATIVE_ATTACHMENTS).contains(&items.len()) => Ok(items),
            _ => Err(Error::Denied),
        }
    }

    pub(crate) fn set_media(&mut self, mut items: Vec<Media>) -> Result<(), Error> {
        if items.len() > MAX_NATIVE_ATTACHMENTS {
            return Err(Error::Denied);
        }
        self.media = None;
        self.media_set = None;
        if items.len() == 1 {
            self.media = items.pop();
        } else if !items.is_empty() {
            self.media_set = Some(items);
        }
        Ok(())
    }

    pub(crate) fn confirm_media(
        &mut self,
        snapshots: &[crate::native_media::model::Snapshot],
    ) -> Result<(), Error> {
        let mut items = self.media_items()?.to_vec();
        if items.len() != snapshots.len() {
            return Err(Error::Denied);
        }
        for (item, snapshot) in items.iter_mut().zip(snapshots) {
            item.confirm(snapshot)?;
        }
        self.set_media(items)
    }
}

impl Media {
    pub(super) fn prepare_set(
        message: &OutboundMessage,
        files: &[PreparedFile],
        replacement: bool,
    ) -> Result<Vec<Self>, Error> {
        message.validate().map_err(|_| Error::Denied)?;
        if files.len() != message.native_attachments.len()
            || files.len() > MAX_NATIVE_ATTACHMENTS
            || (files.is_empty() && !(replacement && message.clear_native_attachments))
        {
            return Err(Error::Denied);
        }
        files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let mut item = Self::prepare_item(message, index, file)?;
                item.pending_replacement = replacement;
                Ok(item)
            })
            .collect()
    }
}
