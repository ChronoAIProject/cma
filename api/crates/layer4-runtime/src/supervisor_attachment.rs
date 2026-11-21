use layer3_adapter::{AgentAttachmentReceipt, AgentAttachmentUpload};

use super::Supervisor;
use crate::failure::{Result, RuntimeError};

impl Supervisor {
    pub(super) async fn upload_attachment(
        &self,
        upload: AgentAttachmentUpload,
    ) -> Result<AgentAttachmentReceipt> {
        let request = &upload.request;
        request
            .validate()
            .map_err(|_| refused("attachment identity is invalid"))?;
        if request.sandbox_id != self.config.sandbox_id
            || request.runtime_generation != self.config.generation
            || request.boot_id != self.boot_id
        {
            return Err(refused(
                "attachment request does not name this runtime process",
            ));
        }
        if !crate::agent_attachment::supported(&self.config) {
            return Err(refused("attachments require the complete persistent home"));
        }
        self.require_credentialed().await?;
        if self
            .agents
            .read()
            .await
            .get(&request.agent_id)
            .is_none_or(|agent| agent.bridge.is_ended())
        {
            return Err(refused("attachment agent is not running in this runtime"));
        }
        let content = upload
            .content()
            .map_err(|_| refused("attachment content is invalid"))?;
        crate::agent_attachment::publish(std::path::Path::new("/home"), request, &content)
    }
}
fn refused(detail: &str) -> RuntimeError {
    RuntimeError::rejected("attachment_upload_refused", detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest as _, Sha256};
    #[tokio::test]
    async fn attachment_identity_is_bound_to_the_exact_process_before_home_mutation() {
        let root = std::env::temp_dir().join(format!(
            "cma-upload-authority-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let config = crate::config::RuntimeConfig::for_test(root.join("home"));
        let supervisor = Supervisor::new(
            config.clone(),
            crate::writer_fence::SupervisorBoundary::default(),
        )
        .unwrap();
        let original = layer3_adapter::AgentAttachmentRequest {
            sandbox_id: config.sandbox_id.clone(),
            runtime_generation: config.generation,
            boot_id: supervisor.boot_id.clone(),
            agent_id: layer3_adapter::AgentSlot::new("agt_test"),
            attachment_id: format!("att_{}", "b".repeat(64)),
            name: "owner.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: 1,
            sha256: format!("{:x}", Sha256::digest(b"x")),
        };
        for case in 0..3 {
            let mut request = original.clone();
            match case {
                0 => request.sandbox_id = "foreign".into(),
                1 => request.runtime_generation += 1,
                _ => request.boot_id = "f".repeat(32),
            }
            let upload = AgentAttachmentUpload::new(request, b"x").unwrap();
            assert!(supervisor
                .upload_attachment(upload)
                .await
                .unwrap_err()
                .detail
                .contains("does not name this runtime process"));
        }
        let upload = AgentAttachmentUpload::new(original, b"x").unwrap();
        assert!(supervisor
            .upload_attachment(upload)
            .await
            .unwrap_err()
            .detail
            .contains("complete persistent home"));
        assert!(!root.join("attachments").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
