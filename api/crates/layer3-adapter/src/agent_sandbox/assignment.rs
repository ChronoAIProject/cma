//! Credential delivery only after the caller durably adopts a native pool member.
use kube::{api::AttachParams, ResourceExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::{api_error, rejected, unavailable, AgentSandboxClient, PodBinding};
use crate::{
    runtime_bootstrap::{RuntimeBootstrapReceipt, RuntimeBootstrapRequest},
    Result, RuntimeHandle,
};

pub(super) use crate::runtime_bootstrap::NATIVE_ASSIGNMENT_DEADLINE as ASSIGN_TIMEOUT;
const RECEIPT_LIMIT: usize = 1024;
const STDERR_LIMIT: usize = 4096;

impl AgentSandboxClient {
    /// The caller must validate/adopt the claim, immutable template and PVC first.
    /// This delivery step cannot create or transfer resource ownership. Its exact
    /// request is safe to retry after a lost exec response, while this Pod lives.
    pub async fn assign_runtime(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        request: &RuntimeBootstrapRequest,
    ) -> Result<RuntimeBootstrapReceipt> {
        self.validate_assignment(runtime, expected, request)?;
        tokio::time::timeout(ASSIGN_TIMEOUT, async {
            let before = self.assignment_identity(runtime, expected, request).await?;
            let receipt = self.deliver_assignment(&expected.pod_name, request).await?;
            let after = self.assignment_identity(runtime, expected, request).await?;
            // Status/resourceVersion updates are normal; spec/authority changes
            // during delivery cannot turn its receipt into valid provisioning.
            if before.annotations() != after.annotations()
                || before.data["spec"] != after.data["spec"]
            {
                return Err(rejected("Sandbox changed during runtime assignment"));
            }
            Ok(receipt)
        })
        .await
        .map_err(|_| unavailable("runtime assignment timed out"))?
    }

    pub(crate) async fn confirm_staging_startup_failure(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        mut bootstrap: RuntimeBootstrapRequest,
        receipt: &RuntimeBootstrapReceipt,
    ) -> Result<()> {
        receipt.validate_for(&bootstrap)?;
        self.validate_assignment(runtime, expected, &bootstrap)?;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            if self.pod_binding(runtime).await? != *expected {
                return Err(rejected(
                    "runtime incarnation changed before staging confirmation",
                ));
            }
            bootstrap.staging_startup_confirmed = true;
            self.deliver_assignment(&expected.pod_name, &bootstrap)
                .await?;
            Ok(())
        })
        .await
        .map_err(|_| unavailable("staging startup confirmation timed out"))?
    }

    pub(super) fn validate_assignment(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        request: &RuntimeBootstrapRequest,
    ) -> Result<()> {
        request.validate()?;
        let (name, uid) = super::parse_handle(runtime)?;
        if expected.sandbox_name != name
            || expected.sandbox_uid != uid
            || expected.pod_uid != request.pod_uid
            || request.environment.get("CHRONO_RUNTIME_CONTROL_TOKEN")
                != Some(&self.control_token(name))
        {
            return Err(rejected("runtime assignment authority mismatch"));
        }
        Ok(())
    }

    async fn assignment_identity(
        &self,
        runtime: &RuntimeHandle,
        expected: &PodBinding,
        request: &RuntimeBootstrapRequest,
    ) -> Result<kube::api::DynamicObject> {
        let object = self
            .get(runtime)
            .await?
            .ok_or_else(|| unavailable("Sandbox disappeared during runtime assignment"))?;
        if let Some(encoded) = object.annotations().get(super::NATIVE_HOME_ANNOTATION) {
            let origin: super::NativePoolHomeOrigin = serde_json::from_str(encoded)
                .map_err(|_| rejected("invalid native runtime origin"))?;
            origin.validate_for(
                &self.config.namespace,
                &self.config.deployment,
                &origin.claim.request.home,
            )?;
            self.pools.observe_claim(&origin.claim).await?;
            self.pools.member_owned(&object, &origin.claim)?;
            if object.uid().as_deref() != Some(&origin.sandbox_uid) {
                return Err(rejected("native Sandbox changed during runtime assignment"));
            }
        }
        for (annotation, environment) in [
            ("cma/sandbox", "CHRONO_SANDBOX_ID"),
            ("cma/runtime-generation", "CHRONO_RUNTIME_GENERATION"),
            ("cma/home-identity", "CHRONO_RUNTIME_HOME_IDENTITY"),
        ] {
            if object.annotations().get(annotation) != request.environment.get(environment) {
                return Err(rejected(
                    "runtime assignment differs from Sandbox authority",
                ));
            }
        }
        if self.pod_binding(runtime).await? != *expected {
            return Err(rejected("Sandbox Pod changed during runtime assignment"));
        }
        Ok(object)
    }

    pub(super) async fn deliver_assignment(
        &self,
        pod_name: &str,
        request: &RuntimeBootstrapRequest,
    ) -> Result<RuntimeBootstrapReceipt> {
        let parameters = AttachParams::default()
            .container("runtime")
            .stdin(true)
            .stdout(true)
            .stderr(true)
            .tty(false);
        // Fixed executable/arguments: no shell, request, token or environment
        // values in exec query parameters, Kubernetes objects or audit records.
        let mut process = self
            .pods
            .exec(
                pod_name,
                ["/usr/local/bin/layer4-runtime", "--assign-runtime"],
                &parameters,
            )
            .await
            .map_err(api_error)?;
        let mut stdin = process
            .stdin()
            .ok_or_else(|| unavailable("runtime assignment stdin missing"))?;
        let stdout = process
            .stdout()
            .ok_or_else(|| unavailable("runtime assignment stdout missing"))?;
        let stderr = process
            .stderr()
            .ok_or_else(|| unavailable("runtime assignment stderr missing"))?;
        let status = process
            .take_status()
            .ok_or_else(|| unavailable("runtime assignment status missing"))?;
        let bytes =
            serde_json::to_vec(request).map_err(|_| rejected("invalid runtime assignment"))?;
        let send = async {
            stdin
                .write_all(&bytes)
                .await
                .map_err(|_| unavailable("runtime assignment write failed"))?;
            stdin
                .shutdown()
                .await
                .map_err(|_| unavailable("runtime assignment EOF failed"))?;
            drop(stdin);
            Ok(())
        };
        let exit = async {
            if status.await.as_ref().and_then(|s| s.status.as_deref()) != Some("Success") {
                return Err(unavailable("runtime assignment command did not succeed"));
            }
            Ok(())
        };
        let (_, stdout, _, ()) = tokio::try_join!(
            send,
            bounded_output(stdout, RECEIPT_LIMIT),
            bounded_output(stderr, STDERR_LIMIT),
            exit
        )?;
        process
            .join()
            .await
            .map_err(|_| unavailable("runtime assignment stream failed"))?;
        let receipt: RuntimeBootstrapReceipt = serde_json::from_slice(&stdout)
            .map_err(|_| rejected("runtime assignment receipt is invalid"))?;
        receipt.validate_for(request)?;
        Ok(receipt)
    }
}

async fn bounded_output(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable("runtime assignment output failed"))?;
    if bytes.len() > limit {
        return Err(rejected("runtime assignment output exceeds its bound"));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "assignment_tests.rs"]
mod tests;
