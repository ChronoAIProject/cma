//! The payload is bound to this session, Console purpose and format version.
//! Database projections hold clocks and quotas; identity and secrets stay sealed.
use super::*;
use crate::console::session_backend::Stored;
use cmaeg_core::seal::{open_connection, seal_connection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Data {
    pub subject: String,
    pub selected: OwnerSelection,
    pub csrf: [u8; 32],
    pub flash: Option<Flash>,
    pub access: String,
    pub refresh: String,
    pub revision: String,
    pub token_revision: String,
    pub refreshing: bool,
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pub data: Data,
    pub stored: Stored,
}

impl SharedSessions {
    pub(super) fn seal(&self, id: &str, data: &Data) -> Result<Vec<u8>, StoreError> {
        let clear = serde_json::to_vec(data).map_err(|_| StoreError::Invalid)?;
        seal_connection(&self.keys, &binding(id), &clear).map_err(|_| StoreError::Invalid)
    }

    pub(super) fn open(&self, id: &str, stored: Stored) -> Result<Snapshot, StoreError> {
        let data = self.open_data(id, &stored.payload)?;
        if codec_subject(&data.subject) != stored.subject_hash {
            return Err(StoreError::Invalid);
        }
        Ok(Snapshot { data, stored })
    }

    pub(super) fn open_data(&self, id: &str, payload: &[u8]) -> Result<Data, StoreError> {
        let clear =
            open_connection(&self.keys, &binding(id), payload).map_err(|_| StoreError::Invalid)?;
        let data: Data = serde_json::from_slice(&clear.payload).map_err(|_| StoreError::Invalid)?;
        if data.subject.is_empty()
            || data.subject.len() > 512
            || data.access.is_empty()
            || data.access.len() > 16_384
            || data.refresh.is_empty()
            || data.refresh.len() > 16_384
            || data.revision.len() != 43
            || data.token_revision.len() != 43
            || matches!(&data.selected, OwnerSelection::Organization(id) if !crate::console::orgs::valid_id(id))
        {
            return Err(StoreError::Invalid);
        }
        Ok(data)
    }
}

pub(super) fn random() -> Result<String, StoreError> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| StoreError::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(super) fn id(key: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(key)
}
fn codec_subject(subject: &str) -> String {
    id(&digest(subject))
}

fn binding(id: &str) -> String {
    format!("console-session:v1:{id}")
}

impl SessionCell {
    pub(super) fn snapshot(&self) -> Result<Snapshot, StoreError> {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(StoreError::Invalid)
    }

    pub(super) fn apply(&self, snapshot: Snapshot, auth: &mut SessionAuth, now: Instant) {
        let data = &snapshot.data;
        {
            let mut meta = self.meta();
            meta.subject.clone_from(&data.subject);
            meta.selected = data.selected.clone();
            meta.csrf_key = data.csrf;
            meta.flash = data.flash.clone();
        }
        auth.access = Secret::new(data.access.clone());
        auth.refresh = Secret::new(data.refresh.clone());
        let remaining = snapshot
            .stored
            .access_expires_ms
            .saturating_sub(snapshot.stored.now_ms)
            .max(0);
        auth.access_expires_at = now + Duration::from_millis(remaining as u64);
        auth.refresh_retry_at = None;
        *self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(snapshot);
    }
}
