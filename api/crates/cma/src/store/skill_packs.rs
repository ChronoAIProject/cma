//! A session's resolved skill packs, kept in the platform object store.
//!
//! These are the exact bytes fetched while the caller who could see them was
//! present. A replacement runtime replays them instead of asking the registry
//! under a different identity and possibly receiving fewer or newer skills.
//!
//! The archive contains names, versions, relative paths, and file bodies only.
//! It has no field for the credential used to fetch them, and this adapter is
//! never given that credential. The service's own short-lived storage token is
//! obtained separately for each object-store operation and is never serialized.

use std::{io::Write, sync::Arc};

use futures_util::StreamExt;
use layer2_core::{
    store::{SkillPack, SkillPackStore, StoreError},
    SessionId,
};

use crate::store::token::TokenSource;

/// The largest encoded pack collection this process will upload or download.
///
/// The runtime supervisor accepts a four-MiB prelude. Holding the same hard
/// ceiling here keeps a corrupted or hostile object from becoming an unbounded
/// allocation while leaving ample room for the roughly 400-KiB baseline pack.
const ARCHIVE_LIMIT: usize = 4 * 1024 * 1024;
const ARCHIVE_VERSION: u32 = 1;
const CONTENT_TYPE: &str = "application/octet-stream";

#[derive(serde::Serialize, serde::Deserialize)]
struct ArchiveHeader {
    #[serde(rename = "cmaSkillPacks")]
    format: u32,
    packs: Vec<ArchivedPack>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ArchivedPack {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    files: Vec<ArchivedFile>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ArchivedFile {
    path: String,
    len: u64,
}

/// A serialization target that cannot grow beyond the archive budget.
struct BoundedArchive {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl BoundedArchive {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        }
    }
}

impl Write for BoundedArchive {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > self.limit)
        {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "skill pack archive limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Object-store custody for session skill packs.
pub struct ChronoStorageSkillPackStore {
    base_url: String,
    bucket: String,
    tokens: Arc<dyn TokenSource>,
    http: reqwest::Client,
}

impl ChronoStorageSkillPackStore {
    /// Build a skill-pack store over an existing deployment-owned bucket.
    pub fn new(
        base_url: impl Into<String>,
        bucket: impl Into<String>,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bucket: bucket.into(),
            tokens,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("http client builds"),
        }
    }

    /// Stable and distinct from the transcript object for the same session.
    pub fn key_for(id: &SessionId) -> String {
        format!("sessions/{}/skill-packs.v1", id.0)
    }

    fn objects_url(&self) -> String {
        format!("{}/api/buckets/{}/objects", self.base_url, self.bucket)
    }

    fn download_url(&self) -> String {
        format!("{}/download", self.objects_url())
    }

    async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, StoreError> {
        if response
            .content_length()
            .is_some_and(|length| length > ARCHIVE_LIMIT as u64)
        {
            return Err(too_large());
        }
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or_default()
                .min(ARCHIVE_LIMIT),
        );
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk
                .map_err(|error| StoreError::Unavailable(format!("skill pack body: {error}")))?;
            if body
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > ARCHIVE_LIMIT)
            {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[async_trait::async_trait]
impl SkillPackStore for ChronoStorageSkillPackStore {
    async fn put(&self, id: &SessionId, packs: &[SkillPack]) -> Result<(), StoreError> {
        let body = encode(packs)?;
        let key = Self::key_for(id);
        let url = self.objects_url();
        let bytes = body.len();
        let response = self
            .http
            .post(&url)
            .query(&[("key", key.as_str()), ("contentType", CONTENT_TYPE)])
            .bearer_auth(self.tokens.token().await?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable(format!("POST {url}: {error}")))?;
        if !response.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "skill pack upload returned {}",
                response.status()
            )));
        }
        tracing::info!(session_id = %id, skills = packs.len(), bytes, "session skills stored");
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<Vec<SkillPack>>, StoreError> {
        let key = Self::key_for(id);
        let url = self.download_url();
        let response = self
            .http
            .get(&url)
            .query(&[("key", key.as_str())])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable(format!("GET {url}: {error}")))?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(StoreError::Unavailable(format!(
                "skill pack download returned {}",
                response.status()
            )));
        }
        let body = Self::bounded_body(response).await?;
        decode(&body).map(Some)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        let key = Self::key_for(id);
        let url = self.objects_url();
        let response = self
            .http
            .delete(&url)
            .query(&[("key", key.as_str())])
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable(format!("DELETE {url}: {error}")))?;
        let status = response.status();
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        Err(StoreError::Unavailable(format!(
            "skill pack delete returned {status}"
        )))
    }
}

fn encode(packs: &[SkillPack]) -> Result<Vec<u8>, StoreError> {
    let mut bodies_len = 0usize;
    let mut archived = Vec::with_capacity(packs.len());
    for pack in packs {
        if pack.name.trim().is_empty() || pack.files.is_empty() {
            return Err(StoreError::Malformed(
                "a stored skill pack must have a name and at least one file".into(),
            ));
        }
        let mut files = Vec::with_capacity(pack.files.len());
        for (path, body) in &pack.files {
            if path.trim().is_empty() {
                return Err(StoreError::Malformed(
                    "a stored skill pack file must have a path".into(),
                ));
            }
            bodies_len = bodies_len
                .checked_add(body.len())
                .filter(|length| *length <= ARCHIVE_LIMIT)
                .ok_or_else(too_large)?;
            files.push(ArchivedFile {
                path: path.clone(),
                len: u64::try_from(body.len()).map_err(|_| too_large())?,
            });
        }
        archived.push(ArchivedPack {
            name: pack.name.clone(),
            version: pack.version.clone(),
            files,
        });
    }
    let header_limit = ARCHIVE_LIMIT
        .checked_sub(bodies_len)
        .and_then(|remaining| remaining.checked_sub(1))
        .ok_or_else(too_large)?;
    let mut encoded = BoundedArchive::new(header_limit);
    if let Err(error) = serde_json::to_writer(
        &mut encoded,
        &ArchiveHeader {
            format: ARCHIVE_VERSION,
            packs: archived,
        },
    ) {
        return Err(match encoded.exceeded {
            true => too_large(),
            false => StoreError::Malformed(format!("skill pack header: {error}")),
        });
    }
    let mut archive = encoded.bytes;
    archive.reserve_exact(1 + bodies_len);
    archive.push(b'\n');
    for pack in packs {
        for (_, body) in &pack.files {
            archive.extend_from_slice(body);
        }
    }
    Ok(archive)
}

fn decode(archive: &[u8]) -> Result<Vec<SkillPack>, StoreError> {
    if archive.len() > ARCHIVE_LIMIT {
        return Err(too_large());
    }
    let split = archive
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| StoreError::Malformed("skill pack archive has no header line".into()))?;
    let header: ArchiveHeader = serde_json::from_slice(&archive[..split])
        .map_err(|error| StoreError::Malformed(format!("skill pack header: {error}")))?;
    if header.format != ARCHIVE_VERSION {
        return Err(StoreError::Malformed(format!(
            "unsupported skill pack archive version {}",
            header.format
        )));
    }

    let mut at = split + 1;
    let mut packs = Vec::with_capacity(header.packs.len());
    for pack in header.packs {
        if pack.name.trim().is_empty() || pack.files.is_empty() {
            return Err(StoreError::Malformed(
                "a stored skill pack must have a name and at least one file".into(),
            ));
        }
        let mut files = Vec::with_capacity(pack.files.len());
        for file in pack.files {
            if file.path.trim().is_empty() {
                return Err(StoreError::Malformed(
                    "a stored skill pack file must have a path".into(),
                ));
            }
            let len = usize::try_from(file.len).map_err(|_| too_large())?;
            let end = at
                .checked_add(len)
                .filter(|end| *end <= archive.len())
                .ok_or_else(|| {
                    StoreError::Malformed("skill pack file extends beyond its archive".into())
                })?;
            files.push((file.path, archive[at..end].to_vec()));
            at = end;
        }
        packs.push(SkillPack {
            name: pack.name,
            version: pack.version,
            files,
        });
    }
    if at != archive.len() {
        return Err(StoreError::Malformed(
            "skill pack archive has unclaimed trailing bytes".into(),
        ));
    }
    Ok(packs)
}

fn too_large() -> StoreError {
    StoreError::Malformed(format!(
        "skill pack archive exceeds the {ARCHIVE_LIMIT}-byte limit"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> SkillPack {
        SkillPack {
            name: "repository-review".into(),
            version: Some("2.1".into()),
            files: vec![
                ("SKILL.md".into(), b"# Review".to_vec()),
                ("references/checks.bin".into(), vec![0, 1, 127, 128, 255]),
            ],
        }
    }

    #[test]
    fn the_archive_round_trips_exact_names_versions_paths_and_bytes() {
        let packs = vec![pack()];
        assert_eq!(decode(&encode(&packs).expect("encodes")).unwrap(), packs);
    }

    #[test]
    fn an_empty_resolution_has_a_round_trippable_marker() {
        assert_eq!(decode(&encode(&[]).expect("encodes")).unwrap(), Vec::new());
    }

    #[test]
    fn an_oversized_archive_is_refused_before_any_upload() {
        let mut oversized = pack();
        oversized.files[0].1 = vec![0; ARCHIVE_LIMIT];
        assert!(matches!(
            encode(&[oversized]),
            Err(StoreError::Malformed(_))
        ));
    }

    #[test]
    fn oversized_header_metadata_is_refused_while_it_is_serialized() {
        let mut oversized = pack();
        oversized.name = "x".repeat(ARCHIVE_LIMIT);
        assert!(matches!(
            encode(&[oversized]),
            Err(StoreError::Malformed(_))
        ));
    }

    #[test]
    fn a_declared_length_beyond_the_archive_is_refused() {
        let archive =
            br#"{"cmaSkillPacks":1,"packs":[{"name":"one","files":[{"path":"SKILL.md","len":2}]}]}
x"#;
        assert!(matches!(decode(archive), Err(StoreError::Malformed(_))));
    }

    #[test]
    fn unclaimed_trailing_bytes_are_refused() {
        let mut archive = encode(&[pack()]).expect("encodes");
        archive.push(b'x');
        assert!(matches!(decode(&archive), Err(StoreError::Malformed(_))));
    }

    #[test]
    fn the_key_is_stable_and_cannot_overwrite_the_transcript() {
        let id = SessionId("sesn_1".into());
        assert_eq!(
            ChronoStorageSkillPackStore::key_for(&id),
            "sessions/sesn_1/skill-packs.v1"
        );
        assert_ne!(
            ChronoStorageSkillPackStore::key_for(&id),
            crate::store::ChronoStorageContextStore::key_for(&id)
        );
    }
}
