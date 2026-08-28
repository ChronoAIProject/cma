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
//! Fenced mutation metadata travels in the object route's query because NyxID
//! preserves that route metadata but does not promise to forward caller-defined
//! `X-Chrono-*` headers.

use std::{fmt::Write as _, io::Write, sync::Arc};

use futures_util::StreamExt;
use layer2_core::{
    store::{
        validate_skill_packs, SkillPack, SkillPackStore, StoreError, SKILL_PACK_COUNT_LIMIT,
        SKILL_PACK_FILE_LIMIT,
    },
    SessionId,
};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use sha2::{Digest, Sha256};

use crate::store::token::TokenSource;

/// The largest encoded pack collection this process will upload or download.
///
/// The runtime supervisor accepts a four-MiB prelude. Holding the same hard
/// ceiling here keeps a corrupted or hostile object from becoming an unbounded
/// allocation while leaving ample room for the roughly 400-KiB baseline pack.
const ARCHIVE_LIMIT: usize = 4 * 1024 * 1024;
const ACK_LIMIT: usize = 16 * 1024;
const ARCHIVE_VERSION: u32 = 1;
const CONTENT_TYPE: &str = "application/octet-stream";
const FENCE_VERSION: u32 = 1;
const STORE_GENERATION: u64 = 1;
const DELETE_GENERATION: u64 = 2;

#[derive(serde::Deserialize)]
struct StorageEnvelope<T> {
    data: Option<T>,
    error: Option<StorageApiError>,
}

#[derive(serde::Deserialize)]
struct StorageApiError {
    code: String,
}

#[derive(serde::Deserialize)]
struct StoreAcknowledgement {
    key: String,
    generation: u64,
    digest: String,
    #[serde(rename = "fenceVersion")]
    fence_version: u32,
}

#[derive(serde::Deserialize)]
struct DeleteAcknowledgement {
    deleted: bool,
    generation: u64,
    #[serde(rename = "fenceVersion")]
    fence_version: u32,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StoreObjectQuery<'a> {
    key: &'a str,
    content_type: &'static str,
    mutation_generation: u64,
    content_sha256: &'a str,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteObjectQuery<'a> {
    key: &'a str,
    mutation_generation: u64,
}

#[derive(serde::Serialize)]
struct ArchiveHeader {
    #[serde(rename = "cmaSkillPacks")]
    format: u32,
    packs: Vec<ArchivedPack>,
}

#[derive(serde::Serialize)]
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

#[derive(serde::Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum ArchiveHeaderField {
    #[serde(rename = "cmaSkillPacks")]
    Format,
    Packs,
    #[serde(other)]
    Other,
}

#[derive(serde::Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum ArchivedPackField {
    Name,
    Version,
    Files,
    #[serde(other)]
    Other,
}

struct ArchiveHeaderSeed;

impl<'de> DeserializeSeed<'de> for ArchiveHeaderSeed {
    type Value = ArchiveHeader;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ArchiveHeader",
            &["cmaSkillPacks", "packs"],
            ArchiveHeaderVisitor,
        )
    }
}

struct ArchiveHeaderVisitor;

impl<'de> Visitor<'de> for ArchiveHeaderVisitor {
    type Value = ArchiveHeader;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a bounded skill pack archive header")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut format = None;
        let mut packs = None;
        let mut remaining_files = SKILL_PACK_FILE_LIMIT;
        while let Some(field) = map.next_key()? {
            match field {
                ArchiveHeaderField::Format => {
                    if format.is_some() {
                        return Err(de::Error::duplicate_field("cmaSkillPacks"));
                    }
                    format = Some(map.next_value()?);
                }
                ArchiveHeaderField::Packs => {
                    if packs.is_some() {
                        return Err(de::Error::duplicate_field("packs"));
                    }
                    packs = Some(map.next_value_seed(ArchivedPacksSeed {
                        remaining_files: &mut remaining_files,
                    })?);
                }
                ArchiveHeaderField::Other => {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
        }
        Ok(ArchiveHeader {
            format: format.ok_or_else(|| de::Error::missing_field("cmaSkillPacks"))?,
            packs: packs.ok_or_else(|| de::Error::missing_field("packs"))?,
        })
    }
}

struct ArchivedPacksSeed<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> DeserializeSeed<'de> for ArchivedPacksSeed<'_> {
    type Value = Vec<ArchivedPack>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(ArchivedPacksVisitor {
            remaining_files: self.remaining_files,
        })
    }
}

struct ArchivedPacksVisitor<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> Visitor<'de> for ArchivedPacksVisitor<'_> {
    type Value = Vec<ArchivedPack>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a bounded sequence of skill packs")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if sequence
            .size_hint()
            .is_some_and(|length| length > SKILL_PACK_COUNT_LIMIT)
        {
            return Err(de::Error::custom(
                "skill pack archive declares too many packs",
            ));
        }
        let mut packs = Vec::with_capacity(
            sequence
                .size_hint()
                .unwrap_or_default()
                .min(SKILL_PACK_COUNT_LIMIT),
        );
        loop {
            if packs.len() == SKILL_PACK_COUNT_LIMIT {
                return match sequence.next_element::<de::IgnoredAny>()? {
                    Some(_) => Err(de::Error::custom(
                        "skill pack archive declares too many packs",
                    )),
                    None => Ok(packs),
                };
            }
            match sequence.next_element_seed(ArchivedPackSeed {
                remaining_files: self.remaining_files,
            })? {
                Some(pack) => packs.push(pack),
                None => return Ok(packs),
            }
        }
    }
}

struct ArchivedPackSeed<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> DeserializeSeed<'de> for ArchivedPackSeed<'_> {
    type Value = ArchivedPack;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ArchivedPack",
            &["name", "version", "files"],
            ArchivedPackVisitor {
                remaining_files: self.remaining_files,
            },
        )
    }
}

struct ArchivedPackVisitor<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> Visitor<'de> for ArchivedPackVisitor<'_> {
    type Value = ArchivedPack;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a skill pack archive entry")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name = None;
        let mut version = None;
        let mut saw_version = false;
        let mut files = None;
        while let Some(field) = map.next_key()? {
            match field {
                ArchivedPackField::Name => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                ArchivedPackField::Version => {
                    if saw_version {
                        return Err(de::Error::duplicate_field("version"));
                    }
                    saw_version = true;
                    version = map.next_value()?;
                }
                ArchivedPackField::Files => {
                    if files.is_some() {
                        return Err(de::Error::duplicate_field("files"));
                    }
                    files = Some(map.next_value_seed(ArchivedFilesSeed {
                        remaining_files: self.remaining_files,
                    })?);
                }
                ArchivedPackField::Other => {
                    map.next_value::<de::IgnoredAny>()?;
                }
            }
        }
        Ok(ArchivedPack {
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            version,
            files: files.ok_or_else(|| de::Error::missing_field("files"))?,
        })
    }
}

struct ArchivedFilesSeed<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> DeserializeSeed<'de> for ArchivedFilesSeed<'_> {
    type Value = Vec<ArchivedFile>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(ArchivedFilesVisitor {
            remaining_files: self.remaining_files,
        })
    }
}

struct ArchivedFilesVisitor<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> Visitor<'de> for ArchivedFilesVisitor<'_> {
    type Value = Vec<ArchivedFile>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a globally bounded sequence of skill files")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if sequence
            .size_hint()
            .is_some_and(|length| length > *self.remaining_files)
        {
            return Err(too_many_archived_files());
        }
        let mut files = Vec::with_capacity(
            sequence
                .size_hint()
                .unwrap_or_default()
                .min(*self.remaining_files),
        );
        loop {
            if *self.remaining_files == 0 {
                return match sequence.next_element::<de::IgnoredAny>()? {
                    Some(_) => Err(too_many_archived_files()),
                    None => Ok(files),
                };
            }
            match sequence.next_element_seed(ArchivedFileSeed {
                remaining_files: self.remaining_files,
            })? {
                Some(file) => files.push(file),
                None => return Ok(files),
            }
        }
    }
}

struct ArchivedFileSeed<'budget> {
    remaining_files: &'budget mut usize,
}

impl<'de> DeserializeSeed<'de> for ArchivedFileSeed<'_> {
    type Value = ArchivedFile;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        *self.remaining_files -= 1;
        <ArchivedFile as serde::Deserialize>::deserialize(deserializer)
    }
}

fn too_many_archived_files<E: de::Error>() -> E {
    E::custom(format_args!(
        "skill pack archive declares more than {SKILL_PACK_FILE_LIMIT} files"
    ))
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
        Self::bounded_response_body(response, ARCHIVE_LIMIT, "skill pack body", too_large).await
    }

    async fn bounded_ack(response: reqwest::Response) -> Result<Vec<u8>, StoreError> {
        Self::bounded_response_body(response, ACK_LIMIT, "storage acknowledgement", || {
            StoreError::Malformed(format!(
                "storage acknowledgement exceeds the {ACK_LIMIT}-byte limit"
            ))
        })
        .await
    }

    async fn bounded_response_body(
        response: reqwest::Response,
        limit: usize,
        label: &str,
        too_large: impl Fn() -> StoreError,
    ) -> Result<Vec<u8>, StoreError> {
        if response
            .content_length()
            .is_some_and(|length| length > limit as u64)
        {
            return Err(too_large());
        }
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or_default()
                .min(limit),
        );
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                StoreError::Unavailable(format!("{label} response body: {error}"))
            })?;
            if body
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > limit)
            {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    async fn mutation_error(
        operation: &str,
        status: reqwest::StatusCode,
        response: reqwest::Response,
    ) -> StoreError {
        let code = match Self::bounded_ack(response).await {
            Ok(body) => serde_json::from_slice::<StorageEnvelope<serde_json::Value>>(&body)
                .ok()
                .and_then(|envelope| envelope.error.map(|error| error.code)),
            Err(error) => return error,
        };
        let detail = code.as_deref().unwrap_or("UNKNOWN_STORAGE_ERROR");
        if (status == reqwest::StatusCode::CONFLICT && detail != "MUTATION_CONTENTION")
            || status == reqwest::StatusCode::BAD_REQUEST
        {
            StoreError::Malformed(format!(
                "skill pack {operation} was rejected by the mutation fence: {detail}"
            ))
        } else {
            StoreError::Unavailable(format!(
                "skill pack {operation} returned {status} ({detail})"
            ))
        }
    }
}

#[async_trait::async_trait]
impl SkillPackStore for ChronoStorageSkillPackStore {
    fn validate(&self, packs: &[SkillPack]) -> Result<(), StoreError> {
        encode(packs).map(|_| ())
    }

    async fn put(&self, id: &SessionId, packs: &[SkillPack]) -> Result<(), StoreError> {
        let body = encode(packs)?;
        let digest = sha256_hex(&body);
        let key = Self::key_for(id);
        let url = self.objects_url();
        let bytes = body.len();
        let response = self
            .http
            .post(&url)
            .query(&StoreObjectQuery {
                key: &key,
                content_type: CONTENT_TYPE,
                mutation_generation: STORE_GENERATION,
                content_sha256: &digest,
            })
            .bearer_auth(self.tokens.token().await?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable(format!("POST {url}: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Self::mutation_error("upload", status, response).await);
        }
        let acknowledgement = parse_ack::<StoreAcknowledgement>(
            &Self::bounded_ack(response).await?,
            "skill pack upload",
        )?;
        if acknowledgement.key != key
            || acknowledgement.generation != STORE_GENERATION
            || acknowledgement.digest != digest
            || acknowledgement.fence_version != FENCE_VERSION
        {
            return Err(StoreError::Malformed(
                "skill pack upload acknowledgement does not match its mutation".into(),
            ));
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
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            let body = Self::bounded_ack(response).await.map_err(|error| {
                StoreError::Unavailable(format!(
                    "skill pack download 404 response could not prove absence: {error}"
                ))
            })?;
            let envelope: StorageEnvelope<serde_json::Value> = serde_json::from_slice(&body)
                .map_err(|error| {
                    StoreError::Unavailable(format!(
                        "skill pack download 404 response could not prove absence: {error}"
                    ))
                })?;
            let code = envelope.error.as_ref().map(|error| error.code.as_str());
            if envelope.data.is_none() && code == Some("OBJECT_NOT_FOUND") {
                return Ok(None);
            }
            return Err(StoreError::Unavailable(format!(
                "skill pack download returned {status} ({})",
                code.unwrap_or("UNKNOWN_STORAGE_ERROR")
            )));
        }
        if !status.is_success() {
            return Err(StoreError::Unavailable(format!(
                "skill pack download returned {status}"
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
            .query(&DeleteObjectQuery {
                key: &key,
                mutation_generation: DELETE_GENERATION,
            })
            .bearer_auth(self.tokens.token().await?)
            .send()
            .await
            .map_err(|error| StoreError::Unavailable(format!("DELETE {url}: {error}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Self::mutation_error("deletion", status, response).await);
        }
        let acknowledgement = parse_ack::<DeleteAcknowledgement>(
            &Self::bounded_ack(response).await?,
            "skill pack deletion",
        )?;
        if !acknowledgement.deleted
            || acknowledgement.generation != DELETE_GENERATION
            || acknowledgement.fence_version != FENCE_VERSION
        {
            return Err(StoreError::Malformed(
                "skill pack deletion acknowledgement does not prove its mutation fence".into(),
            ));
        }
        Ok(())
    }
}

fn parse_ack<T: for<'de> serde::Deserialize<'de>>(
    body: &[u8],
    operation: &str,
) -> Result<T, StoreError> {
    let envelope: StorageEnvelope<T> = serde_json::from_slice(body)
        .map_err(|error| StoreError::Malformed(format!("{operation} acknowledgement: {error}")))?;
    if envelope.error.is_some() {
        return Err(StoreError::Malformed(format!(
            "{operation} success carried an error"
        )));
    }
    envelope
        .data
        .ok_or_else(|| StoreError::Malformed(format!("{operation} acknowledgement has no data")))
}

fn sha256_hex(body: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(body) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

fn encode(packs: &[SkillPack]) -> Result<Vec<u8>, StoreError> {
    validate_skill_packs(packs)
        .map_err(|error| StoreError::Malformed(format!("invalid skill packs: {error}")))?;
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
    let mut deserializer = serde_json::Deserializer::from_slice(&archive[..split]);
    let header = ArchiveHeaderSeed
        .deserialize(&mut deserializer)
        .and_then(|header| {
            deserializer.end()?;
            Ok(header)
        })
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
    validate_skill_packs(&packs)
        .map_err(|error| StoreError::Malformed(format!("invalid stored skill packs: {error}")))?;
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
    use std::{convert::Infallible, sync::Arc};

    use axum::{
        body::{Body, Bytes},
        extract::{Query, State},
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::{get, post},
        Json, Router,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedRequest {
        method: Method,
        path: String,
        query: String,
        key: String,
        query_content_type: Option<String>,
        query_mutation_generation: Option<String>,
        query_content_sha256: Option<String>,
        query_allow_recreate: Option<String>,
        authorization: Option<String>,
        header_content_type: Option<String>,
        x_chrono_headers: Vec<String>,
        body: Vec<u8>,
    }

    #[derive(Clone)]
    enum DownloadPlan {
        StoredObject,
        Status(StatusCode),
        Error(StatusCode, String),
        Body(StatusCode, Vec<u8>),
        Chunked(Vec<Bytes>),
    }

    #[derive(Clone)]
    struct StorageFixture {
        object: Arc<tokio::sync::Mutex<Option<Vec<u8>>>>,
        requests: Arc<tokio::sync::Mutex<Vec<RecordedRequest>>>,
        upload_status: Arc<tokio::sync::Mutex<StatusCode>>,
        download_plan: Arc<tokio::sync::Mutex<DownloadPlan>>,
        delete_status: Arc<tokio::sync::Mutex<StatusCode>>,
        mutation_error_code: Arc<tokio::sync::Mutex<String>>,
        acknowledgement_fence_version: Arc<tokio::sync::Mutex<u32>>,
        acknowledgement_data_override: Arc<tokio::sync::Mutex<Option<serde_json::Value>>>,
    }

    impl Default for StorageFixture {
        fn default() -> Self {
            Self {
                object: Arc::new(tokio::sync::Mutex::new(None)),
                requests: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                upload_status: Arc::new(tokio::sync::Mutex::new(StatusCode::OK)),
                download_plan: Arc::new(tokio::sync::Mutex::new(DownloadPlan::StoredObject)),
                delete_status: Arc::new(tokio::sync::Mutex::new(StatusCode::OK)),
                mutation_error_code: Arc::new(tokio::sync::Mutex::new("STORAGE_ERROR".into())),
                acknowledgement_fence_version: Arc::new(tokio::sync::Mutex::new(FENCE_VERSION)),
                acknowledgement_data_override: Arc::new(tokio::sync::Mutex::new(None)),
            }
        }
    }

    #[derive(serde::Deserialize)]
    struct ObjectQuery {
        key: String,
        #[serde(rename = "contentType")]
        content_type: Option<String>,
        #[serde(rename = "mutationGeneration")]
        mutation_generation: Option<String>,
        #[serde(rename = "contentSha256")]
        content_sha256: Option<String>,
        #[serde(rename = "allowRecreate")]
        allow_recreate: Option<String>,
    }

    fn header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn recorded_request(
        method: Method,
        uri: Uri,
        query: ObjectQuery,
        headers: &HeaderMap,
        body: Vec<u8>,
    ) -> RecordedRequest {
        RecordedRequest {
            method,
            path: uri.path().to_string(),
            query: uri.query().unwrap_or_default().to_string(),
            key: query.key,
            query_content_type: query.content_type,
            query_mutation_generation: query.mutation_generation,
            query_content_sha256: query.content_sha256,
            query_allow_recreate: query.allow_recreate,
            authorization: header(headers, reqwest::header::AUTHORIZATION),
            header_content_type: header(headers, reqwest::header::CONTENT_TYPE),
            x_chrono_headers: headers
                .keys()
                .filter(|name| name.as_str().starts_with("x-chrono-"))
                .map(|name| name.as_str().to_string())
                .collect(),
            body,
        }
    }

    async fn fixture_error(fixture: &StorageFixture, status: StatusCode) -> Response {
        let code = fixture.mutation_error_code.lock().await.clone();
        fixture_error_response(status, &code)
    }

    fn fixture_error_response(status: StatusCode, code: &str) -> Response {
        (
            status,
            Json(serde_json::json!({
                "data": null,
                "error": { "code": code, "message": "fixture error" }
            })),
        )
            .into_response()
    }

    async fn fixture_upload(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let request = recorded_request(Method::POST, uri, query, &headers, body.to_vec());
        let key = request.key.clone();
        let digest = request.query_content_sha256.clone().unwrap_or_default();
        let generation = request
            .query_mutation_generation
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        fixture.requests.lock().await.push(request);
        let status = *fixture.upload_status.lock().await;
        if !status.is_success() {
            return fixture_error(&fixture, status).await;
        }
        *fixture.object.lock().await = Some(body.to_vec());
        let fence_version = *fixture.acknowledgement_fence_version.lock().await;
        let data = fixture
            .acknowledgement_data_override
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| {
                serde_json::json!({
                    "key": key,
                    "generation": generation,
                    "digest": digest,
                    "fenceVersion": fence_version
                })
            });
        Json(serde_json::json!({
            "data": data,
            "error": null
        }))
        .into_response()
    }

    async fn fixture_download(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
        fixture.requests.lock().await.push(recorded_request(
            Method::GET,
            uri,
            query,
            &headers,
            Vec::new(),
        ));
        match fixture.download_plan.lock().await.clone() {
            DownloadPlan::StoredObject => match fixture.object.lock().await.clone() {
                Some(body) => (StatusCode::OK, body).into_response(),
                None => fixture_error_response(StatusCode::NOT_FOUND, "OBJECT_NOT_FOUND"),
            },
            DownloadPlan::Status(status) => status.into_response(),
            DownloadPlan::Error(status, code) => fixture_error_response(status, &code),
            DownloadPlan::Body(status, body) => (status, body).into_response(),
            DownloadPlan::Chunked(chunks) => {
                let stream =
                    futures_util::stream::iter(chunks.into_iter().map(Ok::<Bytes, Infallible>));
                Response::builder()
                    .status(StatusCode::OK)
                    .header(reqwest::header::CONTENT_TYPE, CONTENT_TYPE)
                    .body(Body::from_stream(stream))
                    .expect("chunked fixture response builds")
            }
        }
    }

    async fn fixture_delete(
        State(fixture): State<StorageFixture>,
        Query(query): Query<ObjectQuery>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
        let request = recorded_request(Method::DELETE, uri, query, &headers, Vec::new());
        let generation = request
            .query_mutation_generation
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default();
        fixture.requests.lock().await.push(request);
        let configured = *fixture.delete_status.lock().await;
        if !configured.is_success() {
            return fixture_error(&fixture, configured).await;
        }
        fixture.object.lock().await.take();
        let fence_version = *fixture.acknowledgement_fence_version.lock().await;
        let data = fixture
            .acknowledgement_data_override
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| {
                serde_json::json!({
                    "deleted": true,
                    "generation": generation,
                    "fenceVersion": fence_version
                })
            });
        Json(serde_json::json!({
            "data": data,
            "error": null
        }))
        .into_response()
    }

    async fn storage_fixture() -> (String, StorageFixture, tokio::task::JoinHandle<()>) {
        let fixture = StorageFixture::default();
        let app = Router::new()
            .route(
                "/api/buckets/bkt/objects",
                post(fixture_upload).delete(fixture_delete),
            )
            .route("/api/buckets/bkt/objects/download", get(fixture_download))
            .with_state(fixture.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind storage fixture");
        let address = listener.local_addr().expect("storage fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve storage fixture");
        });
        (format!("http://{address}"), fixture, server)
    }

    fn store(base_url: &str) -> ChronoStorageSkillPackStore {
        ChronoStorageSkillPackStore::new(
            format!("{base_url}/"),
            "bkt",
            Arc::new(crate::store::token::StaticToken("tok".into())),
        )
    }

    fn assert_unavailable_status(error: StoreError, status: StatusCode) {
        assert!(
            matches!(error, StoreError::Unavailable(ref detail) if detail.contains(status.as_str())),
            "got {error:?}"
        );
    }

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

    fn archive(header: serde_json::Value) -> Vec<u8> {
        let mut archive = serde_json::to_vec(&header).expect("archive header serializes");
        archive.push(b'\n');
        archive
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
    fn archive_decode_bounds_pack_and_file_sequences() {
        let packs = (0..=SKILL_PACK_COUNT_LIMIT)
            .map(|index| {
                serde_json::json!({
                    "name": format!("pack-{index}"),
                    "files": [{ "path": "SKILL.md", "len": 0 }]
                })
            })
            .collect::<Vec<_>>();
        let too_many_packs = archive(serde_json::json!({
            "cmaSkillPacks": ARCHIVE_VERSION,
            "packs": packs,
        }));
        assert!(matches!(
            decode(&too_many_packs),
            Err(StoreError::Malformed(ref detail)) if detail.contains("too many packs")
        ));

        let one_oversized_file_list = (0..=SKILL_PACK_FILE_LIMIT)
            .map(|index| serde_json::json!({ "path": format!("file-{index}"), "len": 0 }))
            .collect::<Vec<_>>();
        let too_many_files_in_one_pack = archive(serde_json::json!({
            "cmaSkillPacks": ARCHIVE_VERSION,
            "packs": [{ "name": "one", "files": one_oversized_file_list }],
        }));
        assert!(matches!(
            decode(&too_many_files_in_one_pack),
            Err(StoreError::Malformed(ref detail)) if detail.contains("4096 files")
        ));

        let first = (0..SKILL_PACK_FILE_LIMIT / 2)
            .map(|index| serde_json::json!({ "path": format!("first-{index}"), "len": 0 }))
            .collect::<Vec<_>>();
        let mut second = (0..SKILL_PACK_FILE_LIMIT / 2)
            .map(|index| serde_json::json!({ "path": format!("second-{index}"), "len": 0 }))
            .collect::<Vec<_>>();
        second.push(serde_json::json!(
            "entry 4097 is intentionally not an archived file"
        ));
        let too_many_files = archive(serde_json::json!({
            "cmaSkillPacks": ARCHIVE_VERSION,
            "packs": [
                { "name": "first", "files": first },
                { "name": "second", "files": second }
            ],
        }));
        assert!(matches!(
            decode(&too_many_files),
            Err(StoreError::Malformed(ref detail)) if detail.contains("4096 files")
        ));
    }

    #[test]
    fn archive_decode_rejects_unsafe_or_ambiguous_targets() {
        for files in [
            serde_json::json!([{ "path": "../secret", "len": 0 }]),
            serde_json::json!([
                { "path": "SKILL.md", "len": 0 },
                { "path": "SKILL.md", "len": 0 }
            ]),
            serde_json::json!([{ "path": "bad\u{1}", "len": 0 }]),
        ] {
            let malformed = archive(serde_json::json!({
                "cmaSkillPacks": ARCHIVE_VERSION,
                "packs": [{ "name": "one", "files": files }],
            }));
            assert!(
                matches!(decode(&malformed), Err(StoreError::Malformed(_))),
                "invalid archive target must be refused"
            );
        }

        let duplicate_names = vec![pack(), pack()];
        assert!(matches!(
            encode(&duplicate_names),
            Err(StoreError::Malformed(_))
        ));
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

    #[tokio::test]
    async fn transport_uses_exact_fenced_queries_without_caller_headers() {
        let (base_url, fixture, server) = storage_fixture().await;
        let store = store(&base_url);
        let id = SessionId("sesn_wire".into());
        let expected = vec![pack()];

        SkillPackStore::put(&store, &id, &expected)
            .await
            .expect("upload succeeds");
        assert_eq!(
            SkillPackStore::get(&store, &id)
                .await
                .expect("download succeeds"),
            Some(expected.clone())
        );
        SkillPackStore::delete(&store, &id)
            .await
            .expect("first delete succeeds");
        SkillPackStore::delete(&store, &id)
            .await
            .expect("the same fenced deletion is idempotent");

        let requests = fixture.requests.lock().await;
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0].method, Method::POST);
        assert_eq!(requests[0].path, "/api/buckets/bkt/objects");
        assert_eq!(requests[0].key, "sessions/sesn_wire/skill-packs.v1");
        let encoded_key = "sessions%2Fsesn_wire%2Fskill-packs.v1";
        let body_digest = sha256_hex(&requests[0].body);
        assert_eq!(
            requests[0].query,
            format!(
                "key={encoded_key}&contentType=application%2Foctet-stream&mutationGeneration=1&contentSha256={body_digest}"
            )
        );
        assert_eq!(
            requests[0].query_content_type.as_deref(),
            Some(CONTENT_TYPE)
        );
        assert_eq!(
            requests[0].header_content_type.as_deref(),
            Some(CONTENT_TYPE)
        );
        assert_eq!(requests[0].authorization.as_deref(), Some("Bearer tok"));
        assert_eq!(requests[0].query_mutation_generation.as_deref(), Some("1"));
        assert_eq!(
            requests[0].query_content_sha256.as_deref(),
            Some(body_digest.as_str())
        );
        assert_eq!(requests[0].query_allow_recreate, None);
        assert_eq!(
            decode(&requests[0].body).expect("wire body is an archive"),
            expected
        );

        assert_eq!(requests[1].method, Method::GET);
        assert_eq!(requests[1].path, "/api/buckets/bkt/objects/download");
        assert_eq!(requests[1].query, format!("key={encoded_key}"));
        assert_eq!(requests[1].key, requests[0].key);
        assert_eq!(requests[1].query_content_type, None);
        assert_eq!(requests[1].authorization.as_deref(), Some("Bearer tok"));
        assert_eq!(requests[1].query_mutation_generation, None);
        assert_eq!(requests[1].query_content_sha256, None);
        assert_eq!(requests[1].query_allow_recreate, None);

        for request in &requests[2..] {
            assert_eq!(request.method, Method::DELETE);
            assert_eq!(request.path, "/api/buckets/bkt/objects");
            assert_eq!(
                request.query,
                format!("key={encoded_key}&mutationGeneration=2")
            );
            assert_eq!(request.key, requests[0].key);
            assert_eq!(request.authorization.as_deref(), Some("Bearer tok"));
            assert_eq!(request.query_mutation_generation.as_deref(), Some("2"));
            assert_eq!(request.query_content_sha256, None);
            assert_eq!(request.query_allow_recreate, None);
        }
        for request in requests.iter() {
            assert!(
                request.x_chrono_headers.is_empty(),
                "caller-defined mutation headers escaped into the request: {:?}",
                request.x_chrono_headers
            );
        }
        drop(requests);
        server.abort();
    }

    #[tokio::test]
    async fn transport_distinguishes_typed_mutation_conflicts_from_outages() {
        let (base_url, fixture, server) = storage_fixture().await;
        let store = store(&base_url);
        let id = SessionId("sesn_status".into());

        assert_eq!(
            SkillPackStore::get(&store, &id)
                .await
                .expect("typed object-not-found is an absent object"),
            None
        );

        *fixture.download_plan.lock().await = DownloadPlan::Status(StatusCode::NOT_FOUND);
        assert_unavailable_status(
            SkillPackStore::get(&store, &id)
                .await
                .expect_err("a bare route 404 must not masquerade as an absent object"),
            StatusCode::NOT_FOUND,
        );

        *fixture.download_plan.lock().await =
            DownloadPlan::Error(StatusCode::NOT_FOUND, "BUCKET_NOT_FOUND".into());
        let missing_bucket = SkillPackStore::get(&store, &id)
            .await
            .expect_err("a missing bucket is a storage failure, not an absent skill pack");
        assert!(
            matches!(missing_bucket, StoreError::Unavailable(ref detail) if detail.contains("BUCKET_NOT_FOUND")),
            "got {missing_bucket:?}"
        );

        for body in [
            b"{".to_vec(),
            vec![b'x'; ACK_LIMIT + 1],
            serde_json::to_vec(&serde_json::json!({
                "data": { "key": "sessions/another/skill-packs.v1" },
                "error": { "code": "OBJECT_NOT_FOUND" }
            }))
            .expect("the contradictory envelope serializes"),
        ] {
            *fixture.download_plan.lock().await = DownloadPlan::Body(StatusCode::NOT_FOUND, body);
            let unproved_absence = SkillPackStore::get(&store, &id)
                .await
                .expect_err("an untrusted 404 body cannot prove object absence");
            assert!(
                matches!(unproved_absence, StoreError::Unavailable(_)),
                "got {unproved_absence:?}"
            );
        }

        *fixture.upload_status.lock().await = StatusCode::UNAUTHORIZED;
        assert_unavailable_status(
            SkillPackStore::put(&store, &id, &[pack()])
                .await
                .expect_err("upload status must propagate"),
            StatusCode::UNAUTHORIZED,
        );

        *fixture.download_plan.lock().await = DownloadPlan::Status(StatusCode::SERVICE_UNAVAILABLE);
        assert_unavailable_status(
            SkillPackStore::get(&store, &id)
                .await
                .expect_err("download status must propagate"),
            StatusCode::SERVICE_UNAVAILABLE,
        );

        *fixture.delete_status.lock().await = StatusCode::CONFLICT;
        *fixture.mutation_error_code.lock().await = "STALE_MUTATION_GENERATION".into();
        let stale = SkillPackStore::delete(&store, &id)
            .await
            .expect_err("a stale generation must fail closed");
        assert!(
            matches!(stale, StoreError::Malformed(ref detail) if detail.contains("STALE_MUTATION_GENERATION")),
            "got {stale:?}"
        );

        *fixture.mutation_error_code.lock().await = "MUTATION_CONTENTION".into();
        assert_unavailable_status(
            SkillPackStore::delete(&store, &id)
                .await
                .expect_err("bounded contention remains retryable"),
            StatusCode::CONFLICT,
        );
        server.abort();
    }

    #[tokio::test]
    async fn success_without_the_exact_fence_acknowledgement_is_refused() {
        let (base_url, fixture, server) = storage_fixture().await;
        *fixture.acknowledgement_fence_version.lock().await = 0;
        let store = store(&base_url);
        let id = SessionId("sesn_old_storage".into());

        let upload = SkillPackStore::put(&store, &id, &[pack()])
            .await
            .expect_err("an old or malformed success cannot transfer custody");
        assert!(matches!(upload, StoreError::Malformed(_)), "got {upload:?}");

        let deletion = SkillPackStore::delete(&store, &id)
            .await
            .expect_err("cleanup needs a durable fence acknowledgement");
        assert!(
            matches!(deletion, StoreError::Malformed(_)),
            "got {deletion:?}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn success_acknowledgements_must_match_every_non_fence_mutation_field() {
        let (base_url, fixture, server) = storage_fixture().await;
        let store = store(&base_url);
        let id = SessionId("sesn_wrong_acknowledgement".into());
        let packs = [pack()];
        let key = ChronoStorageSkillPackStore::key_for(&id);
        let digest = sha256_hex(&encode(&packs).expect("archive encodes"));

        for (field, data) in [
            (
                "key",
                serde_json::json!({
                    "key": "sessions/another/skill-packs.v1",
                    "generation": STORE_GENERATION,
                    "digest": digest,
                    "fenceVersion": FENCE_VERSION
                }),
            ),
            (
                "generation",
                serde_json::json!({
                    "key": key,
                    "generation": DELETE_GENERATION,
                    "digest": digest,
                    "fenceVersion": FENCE_VERSION
                }),
            ),
            (
                "digest",
                serde_json::json!({
                    "key": key,
                    "generation": STORE_GENERATION,
                    "digest": "0".repeat(64),
                    "fenceVersion": FENCE_VERSION
                }),
            ),
        ] {
            *fixture.acknowledgement_data_override.lock().await = Some(data);
            let error = SkillPackStore::put(&store, &id, &packs)
                .await
                .expect_err("a mismatched upload acknowledgement must fail closed");
            assert!(
                matches!(error, StoreError::Malformed(_)),
                "mismatched upload {field} yielded {error:?}"
            );
        }

        for (field, data) in [
            (
                "deleted",
                serde_json::json!({
                    "deleted": false,
                    "generation": DELETE_GENERATION,
                    "fenceVersion": FENCE_VERSION
                }),
            ),
            (
                "generation",
                serde_json::json!({
                    "deleted": true,
                    "generation": STORE_GENERATION,
                    "fenceVersion": FENCE_VERSION
                }),
            ),
        ] {
            *fixture.acknowledgement_data_override.lock().await = Some(data);
            let error = SkillPackStore::delete(&store, &id)
                .await
                .expect_err("a mismatched delete acknowledgement must fail closed");
            assert!(
                matches!(error, StoreError::Malformed(_)),
                "mismatched delete {field} yielded {error:?}"
            );
        }
        server.abort();
    }

    #[tokio::test]
    async fn exact_archive_preflight_rejects_before_any_storage_request() {
        let (base_url, fixture, server) = storage_fixture().await;
        let mut oversized = pack();
        oversized.files[0].1 = vec![0; ARCHIVE_LIMIT];

        let error = SkillPackStore::validate(&store(&base_url), &[oversized])
            .expect_err("the exact stored representation must be bounded");
        assert!(matches!(error, StoreError::Malformed(_)), "got {error:?}");
        assert!(
            fixture.requests.lock().await.is_empty(),
            "preflight must not acquire a token or touch object storage"
        );
        assert!(fixture.object.lock().await.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn direct_upload_revalidates_before_touching_storage() {
        let (base_url, fixture, server) = storage_fixture().await;
        let mut unsafe_pack = pack();
        unsafe_pack.files[0].0 = "../secret".into();

        let error = SkillPackStore::put(
            &store(&base_url),
            &SessionId("sesn_invalid_pack".into()),
            &[unsafe_pack],
        )
        .await
        .expect_err("the adapter must not trust its caller's preflight");
        assert!(matches!(error, StoreError::Malformed(_)), "got {error:?}");
        assert!(fixture.requests.lock().await.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn a_valid_archive_can_be_downloaded_without_a_content_length() {
        let (base_url, fixture, server) = storage_fixture().await;
        let expected = vec![pack()];
        let archive = encode(&expected).expect("archive encodes");
        *fixture.download_plan.lock().await =
            DownloadPlan::Chunked(archive.chunks(7).map(Bytes::copy_from_slice).collect());

        assert_eq!(
            SkillPackStore::get(&store(&base_url), &SessionId("sesn_chunks".into()))
                .await
                .expect("chunked archive downloads"),
            Some(expected)
        );
        server.abort();
    }

    #[tokio::test]
    async fn a_chunked_archive_over_the_limit_is_stopped_while_streaming() {
        let (base_url, fixture, server) = storage_fixture().await;
        *fixture.download_plan.lock().await = DownloadPlan::Chunked(vec![
            Bytes::from(vec![b'x'; ARCHIVE_LIMIT]),
            Bytes::from_static(b"x"),
        ]);

        let error = SkillPackStore::get(
            &store(&base_url),
            &SessionId("sesn_oversized_chunks".into()),
        )
        .await
        .expect_err("the streamed body must remain bounded");
        assert!(
            matches!(error, StoreError::Malformed(ref detail) if detail.contains("exceeds")),
            "got {error:?}"
        );
        server.abort();
    }
}
