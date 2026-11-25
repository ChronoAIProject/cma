//! Generation-specific storage acknowledgments for document reclamation.
use super::{unavailable, StorageMaintenance};
use crate::document_storage::{bounded, valid_key, OPERATION_TIMEOUT};
use layer2_core::document_artifacts::{BlobMaintenanceError, DocumentError};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use tokio::time::Instant;

#[derive(Deserialize)]
struct Reply {
    data: Deletion,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Deletion {
    deleted: bool,
    generation: u64,
    fence_version: u64,
    key: String,
}

#[derive(Debug)]
pub(crate) struct ConfirmedDeletion {
    key: String,
    generation: u64,
    namespace: String,
}

impl ConfirmedDeletion {
    pub(crate) fn matches(&self, key: &str, generation: u64, namespace: &str) -> bool {
        self.key == key && self.generation == generation && self.namespace == namespace
    }

    #[cfg(test)]
    pub(crate) fn fixture(key: String, generation: u64, namespace: String) -> Self {
        Self {
            key,
            generation,
            namespace,
        }
    }
}

fn verify(body: &[u8], key: &str, generation: u64) -> Result<(), DocumentError> {
    let reply: Reply = serde_json::from_slice(body).map_err(|_| DocumentError::Corrupt)?;
    if reply.error.is_some()
        || !reply.data.deleted
        || reply.data.generation != generation
        || reply.data.fence_version != 2
        || reply.data.key != key
    {
        return Err(DocumentError::Corrupt);
    }
    Ok(())
}

impl StorageMaintenance {
    pub(crate) async fn delete_fenced(
        &self,
        key: &str,
        generation: u64,
    ) -> Result<ConfirmedDeletion, BlobMaintenanceError> {
        if !self.enabled {
            return Err(DocumentError::Unavailable.into());
        }
        if !valid_key(key)
            || generation <= 1
            || generation > layer2_core::document_artifacts::MAX_SOURCE_GENERATION
        {
            return Err(DocumentError::Invalid.into());
        }
        let deadline = Instant::now() + OPERATION_TIMEOUT;
        tokio::time::timeout_at(deadline, async {
            let mut url = self.objects.clone();
            url.path_segments_mut()
                .map_err(|_| DocumentError::Unavailable)?
                .push("fenced-v2");
            url.query_pairs_mut()
                .append_pair("key", key)
                .append_pair("mutationGeneration", &generation.to_string());
            let response = self.send(Method::DELETE, url, deadline).await?;
            if response.status() != StatusCode::OK {
                return Err(unavailable(&response));
            }
            verify(&bounded(response, 16 * 1024).await?, key, generation)?;
            Ok(ConfirmedDeletion {
                key: key.into(),
                generation,
                namespace: self.namespace_sha256(),
            })
        })
        .await
        .map_err(|_| BlobMaintenanceError::from(DocumentError::Unavailable))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delete_receipt_must_prove_the_requested_generation_and_fence() {
        let exact = br#"{"data":{"deleted":true,"generation":3,"fenceVersion":2,"key":"fixture"},"error":null}"#;
        assert_eq!(verify(exact, "fixture", 3), Ok(()));
        assert_eq!(verify(exact, "fixture", 4), Err(DocumentError::Corrupt));
        for body in [
            &br#"{"data":{"deleted":true,"generation":3,"fenceVersion":1,"key":"fixture"}}"#[..],
            &br#"{"data":{"deleted":true,"generation":3,"key":"fixture"}}"#[..],
            br#"{"data":{"deleted":false,"generation":3,"fenceVersion":2,"key":"fixture"}}"#,
            br#"{"data":{"deleted":true,"generation":3,"fenceVersion":2}}"#,
            br#"{"data":{"deleted":true,"generation":3,"fenceVersion":2,"key":"different"}}"#,
            br#"{"data":{"deleted":true,"generation":3,"fenceVersion":2,"key":"fixture"},"error":{"code":"refused"}}"#,
        ] {
            assert_eq!(verify(body, "fixture", 3), Err(DocumentError::Corrupt));
        }
    }
}
