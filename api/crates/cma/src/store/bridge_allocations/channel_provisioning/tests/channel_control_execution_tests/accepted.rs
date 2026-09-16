use super::*;
use futures_util::FutureExt;

fn canonical_id(held: &AdmittedChannelControl, provider: bool) -> String {
    let callback = &held.sidecar.intent.callback;
    if provider {
        format!("provider:{}", callback.provider_key_sha256)
    } else {
        format!("callback:{}", callback.callback_id)
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_requires_both_acceptance_records_before_releasing_hold() {
    for provider in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, false, "accepted-control-record").await;
        let rows =
            c.f.client
                .database(&c.f.database)
                .collection::<Document>("bridge_callback_dispositions");
        let id = canonical_id(&held, provider);
        let original = rows.find_one(doc! {"_id":&id}).await.unwrap().unwrap();
        assert_eq!(
            rows.delete_one(doc! {"_id":&id})
                .await
                .unwrap()
                .deleted_count,
            1
        );
        assert!(worker::service(&c).authorize(&held.command).await.is_err());
        let (execution, writes) = worker::execution(&c, None, false);
        worker::run(&c, &held, execution.clone()).await;
        assert!(writes.lock().unwrap().is_empty());
        let unchanged =
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(unchanged, held.command);
        rows.insert_one(original).await.unwrap();
        worker::run(&c, &held, execution).await;
        let recovered =
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(recovered.receipt, held.command.receipt);
        assert_eq!(writes.lock().unwrap().len(), 1);
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}

fn remove_before_effect(
    c: &ControlFixture,
    held: &AdmittedChannelControl,
    provider: bool,
    after_binding: bool,
) -> worker::Hook {
    let rows =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_callback_dispositions");
    let id = canonical_id(held, provider);
    let resources = c.f.resources.clone();
    let command_id = held.sidecar.command_id.clone();
    Box::new(move || {
        async move {
            let command = resources.get_command(&command_id).await.unwrap().unwrap();
            assert_eq!(command.work.dispatch.is_some(), after_binding);
            assert_eq!(
                rows.delete_one(doc! {"_id":id})
                    .await
                    .unwrap()
                    .deleted_count,
                1
            );
        }
        .boxed()
    })
}

async fn removed(c: &ControlFixture, held: &AdmittedChannelControl, provider: bool) {
    assert!(c
        .f
        .client
        .database(&c.f.database)
        .collection::<Document>("bridge_callback_dispositions")
        .find_one(doc! {"_id":canonical_id(held, provider)})
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_rechecks_acceptance_in_conversation_transaction() {
    for provider in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, true, "transaction-acceptance-control").await;
        let agent_before =
            c.f.resources
                .get_agent(&held.command.receipt.envelope.agent_id)
                .await
                .unwrap()
                .unwrap();
        let logs_before =
            c.f.resources
                .agent_log(&agent_before.id, 0, 100)
                .await
                .unwrap();
        let hook = remove_before_effect(&c, &held, provider, false);
        let (execution, writes) = worker::execution_at(&c, Some(hook), 0, false);
        worker::run(&c, &held, execution).await;
        removed(&c, &held, provider).await;
        assert!(writes.lock().unwrap().is_empty());
        let command =
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap()
                .unwrap();
        assert!(command.work.dispatch.is_none());
        assert_eq!(command.receipt, held.command.receipt);
        assert_eq!(
            c.f.resources
                .get_agent(&agent_before.id)
                .await
                .unwrap()
                .unwrap(),
            agent_before
        );
        assert_eq!(
            c.f.resources
                .agent_log(&agent_before.id, 0, 100)
                .await
                .unwrap(),
            logs_before
        );
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_ordered_writer_requires_original_acceptance_after_binding() {
    for provider in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, false, "writer-acceptance-control").await;
        let hook = remove_before_effect(&c, &held, provider, true);
        let (execution, writes) = worker::execution(&c, Some(hook), false);
        worker::run(&c, &held, execution).await;
        removed(&c, &held, provider).await;
        let command =
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap()
                .unwrap();
        assert!(command.work.dispatch.is_some());
        assert_eq!(
            command.work.observation.as_ref().unwrap().delivery,
            AgentOperationDelivery::PreWriteFailed
        );
        assert_eq!(command.receipt, held.command.receipt);
        assert!(writes.lock().unwrap().is_empty());
        assert_eq!(c.f.items().await.len(), 1);
        c.f.close().await;
    }
}
