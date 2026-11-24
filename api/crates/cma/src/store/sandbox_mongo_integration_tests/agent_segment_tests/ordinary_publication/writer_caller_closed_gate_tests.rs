//! Compare both real roots with their original transactional command sequences.
use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::Mutex;

type Commands = Arc<Mutex<Vec<(String, Option<String>)>>>;

async fn observed(name: &str) -> (MongoSandboxStore, Commands) {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let commands = Commands::default();
    let captured = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let target = event
                .command
                .get_str(&event.command_name)
                .ok()
                .map(str::to_owned);
            let name = match event.command_name.as_str() {
                "abortTransaction" | "commitTransaction" => "finishTransaction".into(),
                _ => event.command_name,
            };
            captured.lock().unwrap().push((name, target));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, name).await.unwrap();
    commands.lock().unwrap().clear();
    (store, commands)
}

fn take(commands: &Commands) -> Vec<(String, Option<String>)> {
    std::mem::take(&mut *commands.lock().unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn disabled_adoption_matches_original_mongo_commands_for_both_roots() {
    for mode in 0..3 {
        let (client, name, _, current) = legacy("caller_no_io").await;
        if mode != 0 {
            gate(&client, &name, &current, mode == 2, true).await;
        }
        let (store, commands) = observed(&name).await;
        let mutation = output(&current, "same original publication");
        // The real old root begins directly with apply. Its rollback preserves
        // the fixture so the public caller can execute the identical mutation.
        let mut session = store.client.start_session().await.unwrap();
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        scoped::apply(
            scoped::PublicationSource::Conversation,
            &store,
            &mutation,
            None,
            None,
            None,
            &mut scope,
        )
        .await
        .unwrap();
        scope.release().abort_transaction().await.unwrap();
        let expected = take(&commands);
        store
            .commit_agent_conversation(mutation.clone())
            .await
            .unwrap();
        assert_eq!(take(&commands), expected, "conversation gate mode {mode}");
        assert_eq!(version(&store, &current).await, 1);
        let current = mutation.agent;
        let mutation = output(&current, "same checkpoint publication");
        let next = boundary(&mutation);
        take(&commands);
        let mut scope = TransactionScope::start(&mut session).await.unwrap();
        scoped::checkpoint::apply(
            &store,
            (&current, None),
            None,
            &next,
            Some(&mutation),
            &conversation::checkpoint::Authority::Live,
            &mut scope,
        )
        .await
        .unwrap();
        scope.release().abort_transaction().await.unwrap();
        let expected = take(&commands);
        store
            .commit_agent_message_update(current, None, next, Some(mutation))
            .await
            .unwrap();
        assert_eq!(take(&commands), expected, "checkpoint gate mode {mode}");
        client.database(&name).drop().await.unwrap();
    }
}
