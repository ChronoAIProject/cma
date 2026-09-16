//! Read only the exact GKE backing disk captured during original allocation.
use sha2::{Digest, Sha256};

use crate::{
    AdapterError, HomeBackingAbsence, HomeBackingIdentity, HomeBackingObserver,
    HomeBackingReference, Result,
};

#[path = "gke_pd_transport.rs"]
mod transport;

/// Uses the existing workload identity. Missing read permission keeps cleanup held.
/// This observer never lists disks, changes IAM, or deletes provider resources.
pub struct GkePdBackingObserver {
    transport: transport::Transport,
}

impl GkePdBackingObserver {
    pub fn new() -> Result<Self> {
        Ok(Self {
            transport: transport::Transport::new()?,
        })
    }
}

#[async_trait::async_trait]
impl HomeBackingObserver for GkePdBackingObserver {
    async fn capture_original(
        &self,
        reference: &HomeBackingReference,
    ) -> Result<HomeBackingIdentity> {
        let path = disk_path(reference)?;
        let reply = self.transport.read(&reference.volume_handle).await?;
        if reply.status != 200 {
            return Err(unavailable("original backing disk could not be observed"));
        }
        let backing_generation = disk_generation(&path, &reply.value)?.to_owned();
        Ok(HomeBackingIdentity {
            reference: reference.clone(),
            backing_generation,
        })
    }

    async fn observe_original_absence(
        &self,
        original: &HomeBackingIdentity,
    ) -> Result<HomeBackingAbsence> {
        original.validate()?;
        let path = disk_path(&original.reference)?;
        if !numeric_id(&original.backing_generation) {
            return Err(rejected("original disk generation is invalid"));
        }
        let reply = self
            .transport
            .read(&original.reference.volume_handle)
            .await?;
        if reply.status == 200 {
            if disk_generation(&path, &reply.value)? != original.backing_generation {
                return Err(rejected(
                    "backing disk path changed its recorded generation",
                ));
            }
            return Err(unavailable("original backing disk still exists"));
        }
        if reply.status != 404
            || reply.value.pointer("/error/code").and_then(|v| v.as_u64()) != Some(404)
            || !reply
                .value
                .pointer("/error/errors")
                .and_then(|v| v.as_array())
                .is_some_and(|errors| errors.iter().any(|error| error["reason"] == "notFound"))
        {
            return Err(unavailable(
                "authoritative backing disk absence is unavailable",
            ));
        }
        let mut hash = Sha256::new();
        hash.update(b"cma-gke-pd-absence-v1\0GET\0");
        hash.update(reply.url.as_bytes());
        hash.update(b"\0 404\0");
        hash.update(&reply.body);
        Ok(HomeBackingAbsence {
            original: original.clone(),
            // A CMA read-observation correlation ID, not a cloud delete operation.
            request_id: reply.observation_id,
            response_sha256: format!("{:x}", hash.finalize()),
        })
    }
}

struct DiskPath<'a> {
    handle: &'a str,
    location: &'a str,
    location_field: &'static str,
    name: &'a str,
}

fn disk_path(reference: &HomeBackingReference) -> Result<DiskPath<'_>> {
    reference.validate()?;
    let parts: Vec<_> = reference.volume_handle.split('/').collect();
    if reference.csi_driver != "pd.csi.storage.gke.io"
        || parts.len() != 6
        || parts[0] != "projects"
        || !project(parts[1])
        || !matches!(parts[2], "zones" | "regions")
        || !name(parts[3])
        || parts[4] != "disks"
        || !name(parts[5])
    {
        return Err(rejected(
            "recorded backing is not an exact supported GKE disk path",
        ));
    }
    Ok(DiskPath {
        handle: &reference.volume_handle,
        location: reference.volume_handle.rsplit_once("/disks/").unwrap().0,
        location_field: if parts[2] == "zones" {
            "zone"
        } else {
            "region"
        },
        name: parts[5],
    })
}

fn disk_generation<'a>(path: &DiskPath<'_>, value: &'a serde_json::Value) -> Result<&'a str> {
    let generation = value["id"].as_str().filter(|value| numeric_id(value));
    if value["kind"] != "compute#disk"
        || value["name"] != path.name
        || !value["selfLink"]
            .as_str()
            .is_some_and(|link| exact_link(link, path.handle))
        || !value[path.location_field]
            .as_str()
            .is_some_and(|link| exact_link(link, path.location))
        || generation.is_none()
    {
        return Err(rejected(
            "provider disk response does not match the original backing path",
        ));
    }
    Ok(generation.unwrap())
}

fn exact_link(link: &str, path: &str) -> bool {
    [
        "https://www.googleapis.com/compute/v1/",
        "https://compute.googleapis.com/compute/v1/",
    ]
    .iter()
    .any(|prefix| link.strip_prefix(prefix) == Some(path))
}

fn numeric_id(value: &str) -> bool {
    !value.starts_with('0')
        && value.bytes().all(|c| c.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|id| id > 0)
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

fn project(value: &str) -> bool {
    (6..=30).contains(&value.len()) && (name(value) || numeric_id(value))
}

fn rejected(message: &str) -> AdapterError {
    AdapterError::Rejected(message.into())
}

fn unavailable(message: &str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

#[cfg(test)]
#[path = "gke_pd_finality_tests.rs"]
mod tests;
