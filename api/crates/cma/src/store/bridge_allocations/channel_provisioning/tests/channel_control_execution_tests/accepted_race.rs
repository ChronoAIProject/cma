//! A real stale Mongo snapshot distinguishes receipt reads from write fences.
use super::*;
use crate::store::{
    bridge_definitions::inbox::MutationError,
    durability::majority_collection,
    sandbox_mongo::{is_transient, start_transaction},
    MongoCallbackInbox,
};
use mongodb::error::{ErrorKind, WriteFailure};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_execution_canonical_write_fence_conflicts_with_deleted_snapshot_record() {
    for provider in [false, true] {
        let c = ControlFixture::new().await;
        let held = worker::admit(&c, false, "canonical-snapshot-race").await;
        let intent = &held.sidecar.intent;
        let database = c.f.client.database(&c.f.database);
        let rows = majority_collection::<Document>(&database, "bridge_callback_dispositions");
        let commands = majority_collection::<Document>(&database, "agent_commands");
        let ids = [
            format!("callback:{}", intent.callback.callback_id),
            format!("provider:{}", intent.callback.provider_key_sha256),
        ];
        let originals = [
            rows.find_one(doc! {"_id":&ids[0]}).await.unwrap().unwrap(),
            rows.find_one(doc! {"_id":&ids[1]}).await.unwrap().unwrap(),
        ];
        let command_query = doc! {"_id":held.sidecar.command_id.as_str()};
        let command_before = commands
            .find_one(command_query.clone())
            .await
            .unwrap()
            .unwrap();
        let mut session = c.f.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        MongoCallbackInbox::require_accepted_control(&database, intent, &mut session)
            .await
            .unwrap();
        // This staged marker proves that the receipt conflict aborts the whole
        // transaction. It never becomes part of the retained common command.
        assert_eq!(
            commands
                .update_one(
                    command_query.clone(),
                    doc! {"$inc":{"owned_accepted_fence_probe":1_i64}},
                )
                .session(&mut session)
                .await
                .unwrap()
                .matched_count,
            1
        );
        let deleted = usize::from(provider);
        assert_eq!(
            rows.delete_one(doc! {"_id":&ids[deleted]})
                .await
                .unwrap()
                .deleted_count,
            1
        );
        assert!(rows
            .find_one(doc! {"_id":&ids[deleted]})
            .await
            .unwrap()
            .is_none());
        // The independent deletion committed after this transaction's first
        // read. A read-only recheck still accepts its genuine older snapshot.
        MongoCallbackInbox::require_accepted_control(&database, intent, &mut session)
            .await
            .unwrap();
        let result =
            MongoCallbackInbox::fence_accepted_control(&database, intent, &mut session).await;
        let _ = session.abort_transaction().await;
        let error = match result {
            Err(MutationError::Mongo(error)) => error,
            other => panic!("canonical {deleted} must cause a Mongo write conflict: {other:?}"),
        };
        assert!(is_transient(&error), "{error:?}");
        let code = match error.kind.as_ref() {
            ErrorKind::Command(command) => command.code,
            ErrorKind::Write(WriteFailure::WriteError(write)) => write.code,
            other => panic!("expected the actual Mongo WriteConflict: {other:?}"),
        };
        assert_eq!(code, 112, "{error:?}");
        assert_eq!(
            commands.find_one(command_query).await.unwrap(),
            Some(command_before)
        );
        // When the provider row was deleted, the callback counter was written
        // first in the transaction. That write must also have rolled back.
        assert_eq!(
            rows.find_one(doc! {"_id":&ids[1 - deleted]}).await.unwrap(),
            Some(originals[1 - deleted].clone())
        );
        assert!(rows
            .find_one(doc! {"_id":&ids[deleted]})
            .await
            .unwrap()
            .is_none());
        rows.insert_one(originals[deleted].clone()).await.unwrap();
        // Restoring the exact original evidence permits a fresh transaction;
        // each canonical row must receive its own actual committed fence.
        start_transaction(&mut session).await.unwrap();
        MongoCallbackInbox::fence_accepted_control(&database, intent, &mut session)
            .await
            .unwrap();
        session.commit_transaction().await.unwrap();
        for (id, mut expected) in ids.into_iter().zip(originals) {
            let count = expected.get_i64("control_execution_fence").unwrap_or(0);
            expected.insert("control_execution_fence", count + 1);
            assert_eq!(
                rows.find_one(doc! {"_id":id}).await.unwrap(),
                Some(expected)
            );
        }
        assert_eq!(
            c.f.resources
                .get_command(&held.sidecar.command_id)
                .await
                .unwrap(),
            Some(held.command)
        );
        c.f.close().await;
    }
}
