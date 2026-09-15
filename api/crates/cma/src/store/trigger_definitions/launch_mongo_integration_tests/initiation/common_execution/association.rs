use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_owned_association_reopens_and_never_exposes_internal_proof() {
    let a = Accepted::new().await;
    let expected = LaunchFirstMessageCommand {
        agent_id: a.agent.id.clone(),
        command_id: a.command.receipt.envelope.command_id.clone(),
        response_id: a.command.receipt.response_id.clone().unwrap(),
    };
    let owned =
        a.f.service
            .read(&a.f.identity.caller, &a.launch.id)
            .await
            .unwrap();
    assert_eq!(owned.first_message_command.as_ref(), Some(&expected));
    let raw = serde_json::to_value(&owned).unwrap();
    for private in [
        "first_message_command",
        "input_binding_sha256",
        "input_protocol",
        "initiation",
    ] {
        assert!(raw.get(private).is_none());
    }
    let reopened = MongoTriggerDefinitionStore::open(
        &a.f.seed.client,
        &a.f.seed.database,
        a.f.seed.profiles.clone(),
    )
    .await
    .unwrap();
    let read = reopened
        .launch(&a.launch.id, &a.launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(read.first_message_command.as_ref(), Some(&expected));
    let page = reopened
        .list_launches(&a.launch.owner, &LaunchPageRequest::default())
        .await
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .find(|l| l.id == a.launch.id)
            .unwrap()
            .first_message_command
            .as_ref(),
        Some(&expected)
    );
    assert!(reopened
        .launch(&a.launch.id, &SubjectId::new("unrelated-owner"))
        .await
        .unwrap()
        .is_none());
    let outsider = Caller::new(
        SubjectId::new("unrelated-owner"),
        a.f.identity.caller.authorization.clone(),
    );
    assert!(a.f.service.read(&outsider, &a.launch.id).await.is_err());
    assert!(a.current_agent().await.conversation.active_turn.is_none());
    assert_eq!(a.current_command().await.receipt, a.command.receipt);
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_association_refuses_missing_or_substituted_permanent_receipt() {
    for substitution in [false, true] {
        let a = Accepted::new().await;
        let collection =
            a.f.seed
                .client
                .database(&a.f.seed.database)
                .collection::<Document>("trigger_first_message_bindings");
        if substitution {
            collection
                .update_one(
                    doc! {"_id":a.launch.id.as_str()},
                    doc! {"$set":{"record.receipt.response_id":"rsp_substituted"}},
                )
                .await
                .unwrap();
        } else {
            collection
                .delete_one(doc! {"_id":a.launch.id.as_str()})
                .await
                .unwrap();
        }
        assert!(matches!(
            a.f.seed.store.launch(&a.launch.id, &a.launch.owner).await,
            Err(LaunchStoreError::Corrupt(_))
        ));
        assert!(a
            .f
            .service
            .list(&a.f.identity.caller, &LaunchPageRequest::default())
            .await
            .is_err());
        assert_eq!(
            a.f.resources
                .get_command(&a.command.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap()
                .receipt,
            a.command.receipt
        );
    }
}
