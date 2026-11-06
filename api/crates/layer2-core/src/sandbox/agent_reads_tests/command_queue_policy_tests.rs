use super::*;

#[tokio::test]
async fn ordinary_api_and_user_interface_messages_keep_the_five_minute_queue_default() {
    for source in [
        CommandSubmissionSource::Api,
        CommandSubmissionSource::UserInterface,
    ] {
        let (store, service, _, _) = setup(Authority::new()).await;
        let admitted = service
            .admit(
                &context("owner"),
                "selected-credential",
                source,
                submission("default-queue"),
            )
            .await
            .unwrap();
        let command = store
            .get_command(&admitted.receipt.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::seconds_from_rfc3339(&command.queue_deadline).unwrap()
                - crate::seconds_from_rfc3339(&command.receipt.admitted_at).unwrap(),
            300
        );
        assert!(command.work.dispatch.is_none());
    }
}
