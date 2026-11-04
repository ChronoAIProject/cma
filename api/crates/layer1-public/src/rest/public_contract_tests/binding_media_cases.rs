//! Exercise the public media handlers; durable source and admission fences live
//! in the Manager's real-Mongo continuation fixtures.
use super::Scenario;
use crate::{rest::binding_media::*, AuthenticatedResourceCaller};
use axum::Extension;
use layer2_core::{
    ids::{AgentCommandId, AgentResponseId},
    resource_access::CredentialSource,
    sandbox::{agent_attachments::AgentAttachment, agent_io::*},
    AgentId,
};
use std::{collections::BTreeMap, sync::Arc};

const DIGEST: &str = "ef71589075ccf9332917b0d8d711d1a8d205560f96842f9221de70e6c29454e0";

struct Service(String);
impl Service {
    fn check(
        &self,
        caller: &AuthenticatedResourceCaller,
        path: &BindingMediaPath,
    ) -> Result<(), BindingMediaError> {
        assert_eq!(caller.context.credential_source, CredentialSource::Binding);
        assert_eq!(caller.context.caller.subject.as_str(), "binding-owner");
        assert_eq!(path.binding_id, "bnd_contract");
        assert_eq!(path.conversation_id, "chat-contract");
        assert_eq!(path.event_id, "event-contract");
        assert_eq!(path.attachment_index, 0);
        match self.0.as_str() {
            "invalid" => Err(BindingMediaError::Invalid),
            "forbidden" => Err(BindingMediaError::Forbidden),
            "conflict" => Err(BindingMediaError::Conflict),
            "not_ready" => Err(BindingMediaError::NotReady),
            "missing" => Err(BindingMediaError::NotFound),
            "unavailable" => Err(BindingMediaError::Unavailable),
            "success" | "body_mismatch" => Ok(()),
            "invalid_headers" => panic!("invalid headers must fail before upload authorization"),
            value => panic!("unknown media contract scenario {value}"),
        }
    }
}

fn receipt() -> AgentAttachment {
    AgentAttachment {
        id: format!("att_{}", "e".repeat(64)),
        agent_id: AgentId("agt_contract".into()),
        name: "voice.ogg".into(),
        path: format!(
            "/home/attachments/agt_contract/att_{}/voice.ogg",
            "e".repeat(64)
        ),
        size_bytes: 11,
        media_type: "audio/ogg".into(),
        sha256: DIGEST.into(),
    }
}

#[async_trait::async_trait]
impl BindingMediaUpload for Service {
    async fn upload(self: Box<Self>, bytes: Vec<u8>) -> Result<AgentAttachment, BindingMediaError> {
        assert_ne!(
            self.0, "body_mismatch",
            "incomplete body cannot be published"
        );
        assert_eq!(bytes, b"audio bytes");
        Ok(receipt())
    }
}

#[async_trait::async_trait]
impl BindingMediaService for Service {
    async fn attachment(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
    ) -> Result<AgentAttachment, BindingMediaError> {
        self.check(&caller, &path)?;
        Ok(receipt())
    }

    async fn prepare_upload(
        self: Arc<Self>,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        params: BindingMediaUploadParams,
        media_type: String,
        size_bytes: u64,
    ) -> Result<Box<dyn BindingMediaUpload>, BindingMediaError> {
        self.check(&caller, &path)?;
        assert_eq!(params.name, "voice.ogg");
        assert_eq!(params.sha256, DIGEST);
        assert_eq!(params.gateway_binding_generation, None);
        assert_eq!(params.event_expires_at, 1_800_000_000);
        assert_eq!(media_type, "audio/ogg");
        assert_eq!(size_bytes, 11);
        Ok(Box::new(Service(self.0.clone())))
    }

    async fn supplement(
        &self,
        caller: AuthenticatedResourceCaller,
        path: BindingMediaPath,
        body: BindingMediaSupplement,
    ) -> Result<AgentResponseReading, BindingMediaError> {
        self.check(&caller, &path)?;
        assert_eq!(body.gateway_binding_generation, None);
        assert_eq!(body.event_expires_at, 1_800_000_000);
        assert_eq!(body.derived_text, "Hello from the recording.");
        assert_eq!(
            body.artifact,
            serde_json::json!({
                "attachment_index":0,"kind":"transcript","content_sha256":"a".repeat(64),
                "source_sha256":DIGEST,"name":"voice.ogg","media_type":"audio/ogg",
                "size_bytes":11,"text_sha256":"b".repeat(64)
            })
        );
        Ok(AgentResponseReading {
            deferred_failure_detail: None,
            command: CommandStatusView {
                receipt: CommandReceiptView {
                    command_id: AgentCommandId("cmd_media_contract".into()),
                    agent_id: AgentId("agt_contract".into()),
                    response_id: Some(AgentResponseId("resp_media_contract".into())),
                    admitted_at: "2026-10-29T00:00:00Z".into(),
                    agent_order: 2,
                    event_cursor: 10,
                },
                status: AgentResponseStatus::Queued,
                queue_deadline: "2026-10-30T00:00:00Z".into(),
                turn_sequence: None,
                observed_cursor: None,
                delivery: None,
                failure: None,
            },
            metadata: BTreeMap::from([
                ("cma_parent_command_id".into(), "cmd_parent_contract".into()),
                (
                    "cma_parent_response_id".into(),
                    "resp_parent_contract".into(),
                ),
                ("cma_parent_fingerprint".into(), "c".repeat(64)),
                ("cma_artifact_id".into(), "0:transcript".into()),
                ("cma_command_fingerprint".into(), "d".repeat(64)),
            ]),
        })
    }
}

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("binding_media_")?;
    let mut caller = crate::rest::resource_access_tests::context("binding-owner");
    caller.credential_source = CredentialSource::Binding;
    Some(Scenario {
        router: binding_media_router(Arc::new(Service(mode.into())))
            .layer(Extension(caller.clone())),
        subject: Some("binding-owner"),
        authorization: caller.caller.authorization,
        symbols: BTreeMap::new(),
    })
}
