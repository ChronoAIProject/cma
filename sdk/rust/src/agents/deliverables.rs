use super::Agents;
use crate::{
    Error, Result,
    http::{Request, receive_bounded},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Method, StatusCode, header};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[doc = "An explicitly selected resource; live paths, publications and document revisions remain distinct."]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliverableResource {
    #[doc = "An immutable, explicitly published Agent artifact."]
    AgentArtifact {
        #[doc = "Registry artifact identity."]
        id: String,
    },
    #[doc = "One immutable source revision of an independently authorized document."]
    Document {
        #[doc = "Independent document identity."]
        id: String,
        #[doc = "Exact original source revision."]
        revision: u64,
    },
    #[doc = "A live file whose bytes must still match the reference."]
    LiveFile {
        #[doc = "Exact path relative to the mounted home."]
        path: String,
    },
}

#[doc = "Verified resource evidence recorded in one final answer."]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverableReference {
    #[doc = "Agent containing the recorded final answer."]
    pub agent_id: String,
    #[doc = "Explicit resource identity and version selector."]
    pub resource: DeliverableResource,
    #[doc = "Bounded display and download name without path separators."]
    pub name: String,
    #[doc = "Declared media type; downloads remain opaque bytes."]
    pub media_type: String,
    #[doc = "Exact byte count verified when this reference was produced."]
    pub size_bytes: u64,
    #[doc = "Lowercase SHA-256 of the referenced bytes."]
    pub sha256: String,
}

#[doc = "Current independent resource availability; a live file is unchecked until explicitly read."]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverableStatus {
    #[doc = "The selected immutable metadata or checked bytes match."]
    Ready,
    #[doc = "Current file authority is valid; bytes are checked on download or explicit retry."]
    Unchecked,
    #[doc = "The live resource no longer matches the original version."]
    Changed,
    #[doc = "The document has a newer revision; the selected source remains immutable."]
    VersionChanged,
    #[doc = "The selected file no longer exists."]
    Missing,
    #[doc = "Independent resource permission is unavailable."]
    Denied,
    #[doc = "The file runtime is not ready."]
    NotReady,
    #[doc = "The resource exceeds its existing transfer limit."]
    TooLarge,
    #[doc = "The resource could not currently be checked."]
    Unavailable,
}

#[doc = "One deduplicated delivery with its original reference and current availability."]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerDeliverable {
    #[doc = "Stable delivery identity within the selected answer."]
    pub id: String,
    #[doc = "Original version evidence, never replaced with current live bytes."]
    pub reference: DeliverableReference,
    #[doc = "Current independent availability."]
    pub status: DeliverableStatus,
}

#[doc = "Authorized, bounded deliveries belonging to one recorded final answer."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerDeliverables {
    #[doc = "Exact Agent identity."]
    pub agent_id: String,
    #[doc = "Exact event:sequence answer identity."]
    pub answer_id: String,
    #[doc = "Durable sequence of the containing turn."]
    pub turn_sequence: u64,
    #[doc = "At most 32 distinct explicitly referenced resources."]
    pub items: Vec<AnswerDeliverable>,
}

#[doc = "One current delivery read with the exact original answer identity."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerDeliverableReading {
    #[doc = "Exact Agent identity."]
    pub agent_id: String,
    #[doc = "Exact event:sequence answer identity."]
    pub answer_id: String,
    #[doc = "Durable sequence of the containing turn."]
    pub turn_sequence: u64,
    #[doc = "The requested resource with current independent availability."]
    pub item: AnswerDeliverable,
}

#[doc = "A server-verified reference and its unchanged standalone final-answer marker."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverableDescription {
    #[doc = "The exact verified resource and version evidence."]
    pub reference: DeliverableReference,
    #[doc = "Copy unchanged onto its own line in the final answer delivering this resource."]
    pub marker: String,
}

#[derive(Clone, Copy)]
struct DeliveryTarget<'a> {
    agent: &'a str,
    answer: &'a str,
    id: &'a str,
}

impl Agents<'_> {
    #[doc = "Verify one explicit resource under its own authority. This does not publish files, grant sharing or create a snapshot."]
    pub async fn describe_deliverable(
        &self,
        agent_id: &str,
        resource: &DeliverableResource,
        document_token: Option<&str>,
    ) -> Result<DeliverableDescription> {
        if !valid_resource(resource) {
            return Err(Error::validation("invalid explicit deliverable selector"));
        }
        let segments = ["agents", agent_id, "deliverable-references"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(resource), StatusCode::OK)?;
        add_token(&mut request, document_token)?;
        let description: DeliverableDescription = self.client.request(request).await?;
        if !valid_reference(&description.reference, agent_id)
            || description.reference.resource != *resource
        {
            return Err(Error::protocol(
                "verified descriptor does not match the selected resource",
            ));
        }
        let encoded = description
            .marker
            .strip_prefix("<!--cma-deliverable:v1:")
            .and_then(|value| value.strip_suffix("-->"))
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or_else(|| Error::protocol("invalid deliverable marker"))?;
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| Error::protocol("invalid deliverable marker"))?;
        let reference: DeliverableReference = serde_json::from_slice(&bytes)
            .map_err(|_| Error::protocol("invalid deliverable marker metadata"))?;
        if reference != description.reference || URL_SAFE_NO_PAD.encode(&bytes) != encoded {
            return Err(Error::protocol(
                "deliverable marker does not match its verified metadata",
            ));
        }
        Ok(description)
    }

    #[doc = "Read one item's metadata, or explicitly verify that item's bytes on retry."]
    pub async fn read_answer_deliverable(
        &self,
        agent_id: &str,
        answer_id: &str,
        id: &str,
        verify: bool,
        document_token: Option<&str>,
    ) -> Result<AnswerDeliverableReading> {
        self.read_delivery(
            DeliveryTarget {
                agent: agent_id,
                answer: answer_id,
                id,
            },
            verify,
            document_token,
            None,
        )
        .await
    }

    #[doc = "Return bounded opaque bytes only after version/hash verification and a fresh independent authorization read. Dropping this future cancels the operation."]
    pub async fn download_answer_deliverable(
        &self,
        agent_id: &str,
        answer_id: &str,
        expected: &AnswerDeliverable,
        document_token: Option<&str>,
    ) -> Result<Vec<u8>> {
        if !valid_delivery_id(&expected.id) || !valid_reference(&expected.reference, agent_id) {
            return Err(Error::validation("invalid expected delivery version"));
        }
        let deadline = Instant::now()
            .checked_add(self.client.inner.request_timeout)
            .ok_or_else(|| Error::validation("delivery timeout exceeds the clock range"))?;
        let target = DeliveryTarget {
            agent: agent_id,
            answer: answer_id,
            id: &expected.id,
        };
        require_download(
            &self
                .read_delivery(target, false, document_token, Some(deadline))
                .await?
                .item,
            expected,
        )?;
        let segments = [
            "agents",
            agent_id,
            "answers",
            answer_id,
            "deliverables",
            &expected.id,
            "download",
        ];
        let mut request = Request::get(&segments);
        add_token(&mut request, document_token)?;
        request.timeout = Some(remaining(deadline)?);
        let response = receive_bounded(
            &self.client.inner,
            request,
            (expected.reference.size_bytes as usize).max(8192),
        )
        .await?;
        let media = response
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if !media.is_some_and(|value| value.eq_ignore_ascii_case("application/octet-stream"))
            || response.body.len() as u64 != expected.reference.size_bytes
        {
            return Err(Error::protocol(
                "download did not return this delivery's bounded opaque bytes",
            ));
        }
        let digest: String = ring::digest::digest(&ring::digest::SHA256, &response.body)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if digest != expected.reference.sha256 {
            return Err(Error::protocol(
                "download digest does not match this answer version",
            ));
        }
        require_download(
            &self
                .read_delivery(target, false, document_token, Some(deadline))
                .await?
                .item,
            expected,
        )?;
        remaining(deadline)?;
        Ok(response.body)
    }

    async fn read_delivery(
        &self,
        target: DeliveryTarget<'_>,
        verify: bool,
        document_token: Option<&str>,
        deadline: Option<Instant>,
    ) -> Result<AnswerDeliverableReading> {
        let sequence = answer_sequence(target.answer)?;
        if !valid_delivery_id(target.id) {
            return Err(Error::validation("invalid delivery identity"));
        }
        let segments = [
            "agents",
            target.agent,
            "answers",
            target.answer,
            "deliverables",
            target.id,
        ];
        let query = if verify {
            vec![("verify", "true".into())]
        } else {
            vec![]
        };
        let mut request = Request::get(&segments);
        add_token(&mut request, document_token)?;
        request.query = &query;
        request.timeout = deadline.map(remaining).transpose()?;
        let reading: AnswerDeliverableReading = self.client.request(request).await?;
        if reading.agent_id != target.agent
            || reading.answer_id != target.answer
            || reading.turn_sequence == 0
            || reading.turn_sequence >= sequence
            || reading.item.id != target.id
            || !valid_reference(&reading.item.reference, target.agent)
        {
            return Err(Error::protocol(
                "delivery does not belong to the requested answer",
            ));
        }
        Ok(reading)
    }

    #[doc = "Read metadata for explicit references in one completed final answer without downloading resource bodies."]
    pub async fn answer_deliverables(
        &self,
        agent_id: &str,
        answer_id: &str,
        document_token: Option<&str>,
    ) -> Result<AnswerDeliverables> {
        let sequence = answer_sequence(answer_id)?;
        let segments = ["agents", agent_id, "answers", answer_id, "deliverables"];
        let mut request = Request::get(&segments);
        add_token(&mut request, document_token)?;
        let value: AnswerDeliverables = self.client.request(request).await?;
        if value.agent_id != agent_id
            || value.answer_id != answer_id
            || value.turn_sequence == 0
            || value.turn_sequence >= sequence
            || value.items.len() > 32
        {
            return Err(Error::protocol(
                "deliveries do not belong to the requested answer",
            ));
        }
        let mut identities = std::collections::BTreeSet::new();
        for item in &value.items {
            if !valid_delivery_id(&item.id)
                || !identities.insert(&item.id)
                || !valid_reference(&item.reference, agent_id)
            {
                return Err(Error::protocol("invalid answer delivery metadata"));
            }
        }
        Ok(value)
    }
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| Error::transport("delivery download deadline expired"))
}

fn require_download(current: &AnswerDeliverable, expected: &AnswerDeliverable) -> Result<()> {
    if current.id != expected.id
        || current.reference != expected.reference
        || !matches!(
            current.status,
            DeliverableStatus::Ready
                | DeliverableStatus::Unchecked
                | DeliverableStatus::VersionChanged
        )
    {
        return Err(Error::protocol(
            "delivery is unavailable or no longer matches the selected version",
        ));
    }
    Ok(())
}

fn answer_sequence(answer: &str) -> Result<u64> {
    answer
        .strip_prefix("event:")
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .filter(|sequence| *sequence > 0 && answer == format!("event:{sequence}"))
        .ok_or_else(|| Error::validation("invalid final answer identity"))
}

fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_delivery_id(id: &str) -> bool {
    id.strip_prefix("delivery_")
        .is_some_and(|value| hex(value, 64))
}

fn valid_resource(resource: &DeliverableResource) -> bool {
    match resource {
        DeliverableResource::AgentArtifact { id } => {
            id.strip_prefix("art_").is_some_and(|value| hex(value, 64))
        }
        DeliverableResource::Document { id, revision } => {
            *revision > 0 && id.strip_prefix("doc_").is_some_and(|value| hex(value, 32))
        }
        DeliverableResource::LiveFile { path } => {
            path.len() <= 1024
                && !path
                    .chars()
                    .any(|character| character.is_control() || character == '\\')
                && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
        }
    }
}

fn valid_reference(reference: &DeliverableReference, agent_id: &str) -> bool {
    if reference.agent_id != agent_id
        || !valid_resource(&reference.resource)
        || reference.name.is_empty()
        || reference.name.len() > 160
        || reference.name.trim() != reference.name
        || matches!(reference.name.as_str(), "." | "..")
        || reference
            .name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        || !hex(&reference.sha256, 64)
    {
        return false;
    }
    match reference.resource {
        DeliverableResource::LiveFile { .. } => {
            reference.media_type == "application/octet-stream"
                && reference.size_bytes <= 32 * 1024 * 1024
        }
        DeliverableResource::Document { .. } => {
            reference.media_type == "text/markdown"
                && (1..=1024 * 1024).contains(&reference.size_bytes)
        }
        DeliverableResource::AgentArtifact { .. } => {
            (1..=1024 * 1024).contains(&reference.size_bytes)
                && matches!(
                    reference.media_type.as_str(),
                    "text/plain"
                        | "text/markdown"
                        | "application/json"
                        | "application/pdf"
                        | "application/octet-stream"
                        | "image/png"
                        | "image/jpeg"
                        | "image/webp"
                )
        }
    }
}

fn add_token(request: &mut Request<'_>, token: Option<&str>) -> Result<()> {
    if let Some(token) = token {
        if !hex(token, 64) {
            return Err(Error::validation("invalid document share token"));
        }
        request.headers.insert(
            "x-cma-document-share-token",
            header::HeaderValue::from_str(token)
                .map_err(|_| Error::validation("invalid document share token"))?,
        );
    }
    Ok(())
}
