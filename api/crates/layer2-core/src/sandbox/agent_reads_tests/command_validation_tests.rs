use super::*;

#[tokio::test]
async fn model_mismatch_and_unsupported_attachment_do_not_create_a_receipt() {
    let (store, service, _, _) = setup(Authority::new()).await;
    let mut input = submission("model");
    input.requested_model = Some("not-the-pinned-model".into());
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                input
            )
            .await,
        Err(CommandAdmissionError::ModelMismatch)
    ));
    let mut input = submission("file");
    if let AgentCommand::Message { input, .. } = &mut input.command {
        input.content.push(AgentContent::Attachment {
            reference: AgentAttachmentReference {
                id: "file".into(),
                sha256: "a".repeat(64),
                media_type: "text/plain".into(),
                size_bytes: 10,
            },
        });
    }
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                input
            )
            .await,
        Err(CommandAdmissionError::Input(
            AgentOperationError::ContentUnsupported
        ))
    ));
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
