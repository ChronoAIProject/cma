//! Recover an adopted native disk when publication of its attachment was lost.
//! The PVC carries its original claim/Sandbox/PVC UIDs even after claim deletion.
use std::collections::BTreeSet;

use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::{api::ListParams, Api, ResourceExt};

use super::{failure, identity, PersistentHomeVolumes};
use crate::{agent_sandbox::NATIVE_HOME_REQUEST_LABEL, AdapterError, HomeVolumeRequest, Result};

impl PersistentHomeVolumes {
    pub(crate) async fn recover_native_claim(
        claims: &Api<PersistentVolumeClaim>,
        namespace: &str,
        controller: &str,
        request: &HomeVolumeRequest,
    ) -> Result<Option<PersistentVolumeClaim>> {
        request.validate()?;
        let hash = request.computed_sha256();
        let mut params = ListParams::default().limit(200).labels(&format!(
            "cma/home-managed-by={controller},{NATIVE_HOME_REQUEST_LABEL}={}",
            &hash[..52]
        ));
        let mut cursors = BTreeSet::new();
        let mut found = None;
        // Complete the list before accepting a candidate: a later page may
        // contain a conflicting allocation or fail. Empty pages are bounded too.
        for _ in 0..64 {
            let page = claims
                .list(&params)
                .await
                .map_err(|error| failure("recover native durable home", error))?;
            for claim in page.items {
                identity::validate_claim(namespace, controller, request, &claim)?;
                if !claim
                    .annotations()
                    .contains_key(crate::agent_sandbox::NATIVE_HOME_ANNOTATION)
                    || claim
                        .labels()
                        .get(NATIVE_HOME_REQUEST_LABEL)
                        .map(String::as_str)
                        != Some(&hash[..52])
                    || found.is_some()
                {
                    return Err(AdapterError::Rejected(
                        "native home recovery has conflicting allocation records".into(),
                    ));
                }
                found = Some(claim);
            }
            let Some(cursor) = page.metadata.continue_.filter(|s| !s.is_empty()) else {
                return Ok(found);
            };
            if !cursors.insert(cursor.clone()) {
                return Err(AdapterError::Rejected(
                    "native home recovery inventory did not advance".into(),
                ));
            }
            params = params.continue_token(&cursor);
        }
        Err(AdapterError::Unavailable(
            "native home recovery inventory exceeds its bound".into(),
        ))
    }
}
