//! Exact runtime grants and production storage back native reaction safety.
use super::*;
use crate::records::mongo::{self, MongoRecords};
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client, Database,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn whatsapp_retention_on_real_mongo_preserves_replay_unknown_and_cross_channel_fences() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    for named in [false, true] {
        for uncertain in [false, true] {
            let name = format!("cma_wa_retention_{}", uuid::Uuid::new_v4().simple());
            let db = admin.database(&name);
            let result = exercise(&uri, &db, named, uncertain).await;
            db.drop().await.unwrap();
            result.unwrap();
        }
    }
}

async fn exercise(
    uri: &str,
    db: &Database,
    named: bool,
    uncertain: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    mongo::migrate(db).await?;
    db.run_command(mongo::role(db.name(), false)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"reaction-runtime","pwd":&password,
    "roles":[{"role":"cma_eg_runtime","db":db.name()}]})
        .await?;
    let mut options = ClientOptions::parse(uri).await?;
    options.credential = Some(
        Credential::builder()
            .username("reaction-runtime".to_owned())
            .password(password)
            .source(db.name().to_owned())
            .build(),
    );
    let runtime = Client::with_options(options)?.database(db.name());
    let database = MongoRecords::open(runtime.clone()).await?;
    let records =
        RecordClient::new("https://records.example", "fixture")?.with_database(database.clone());
    assert_retention_limits(records).await;
    assert!(!db
        .list_collection_names()
        .await?
        .contains(&mongo::WHATSAPP_FLOWS.to_owned()));
    assert!(!db
        .list_collection_names()
        .await?
        .contains(&mongo::THREAD_FENCES.to_owned()));
    assert!(runtime
        .run_command(doc! {"collMod":mongo::INTERACTIONS,"validationLevel":"off"})
        .await
        .is_err());

    let (mut app, graph) = fixture_for(named).await;
    let original = reference(&app).await;
    let payload = original.open(&app.state.keys).unwrap().payload;
    let authority = payload.whatsapp_reaction.as_ref().unwrap();
    let full_expiry = authority.received_at + cmaeg_core::seal::whatsapp_reaction::MAX_REACTION_AGE;
    assert!(authority.expires_at < full_expiry);
    let records = app.state.resolver.record_client().with_database(database);
    seed_channel(&app, &records).await;
    app.state.resolver = crate::channels::resolver::ChannelResolver::new(
        Arc::new(records),
        app.state.keys.clone(),
        Default::default(),
    );
    app.restart();
    graph.uncertain.store(uncertain, Ordering::SeqCst);
    let first = app
        .state
        .set_message_reaction(app.caller(), &original, "retained", request("👍"))
        .await
        .unwrap();
    assert_eq!(
        first.delivery.kind(),
        if uncertain { "unknown" } else { "confirmed" }
    );
    assert_stored_retention(db, &app, full_expiry, uncertain).await;

    // Reopen the real runtime store and remint only ciphertext: both preserve replay.
    app.state.resolver = crate::channels::resolver::ChannelResolver::new(
        Arc::new(
            app.state
                .resolver
                .record_client()
                .with_database(MongoRecords::open(runtime).await?),
        ),
        app.state.keys.clone(),
        Default::default(),
    );
    app.restart();
    let reminted = MessageRef::seal(&payload, &app.state.keys).unwrap();
    for reference in [&original, &reminted] {
        assert_eq!(
            app.state
                .set_message_reaction(app.caller(), reference, "retained", request("👍"))
                .await
                .unwrap(),
            first
        );
    }
    assert_eq!(graph.calls.lock().unwrap().len(), 1);

    // A second current channel with its own authority still shares the exact
    // phone/recipient/message fence. The strict routed fixture covers issuance.
    app.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
    app.event.record_id = app.definition.record_id.clone();
    let records = app.state.resolver.record_client();
    let pointer = cmaeg_core::seal::ChannelPointer {
        v: 1,
        record_id: app.definition.record_id.clone(),
        record_url: records
            .record_url(app.definition.record_id.as_str())
            .unwrap()
            .to_string(),
    };
    app.event.channel_id = pointer.seal_id(&app.state.keys).unwrap();
    app.event.conversation_id = cmaeg_core::ids::ConversationId::derive(
        &app.state.keys,
        &app.event.channel_id,
        app.definition.partition.version,
        &app.event.partition_key,
    );
    app.event.expires_at += 60;
    seed_channel(&app, &records).await;
    app.restart();
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let later =
        whatsapp_authority::issue(&channel, &app.event, &app.activity, &app.state.keys).unwrap();
    graph.uncertain.store(false, Ordering::SeqCst);
    let second = app
        .state
        .set_message_reaction(app.caller(), &later, "another", request("❤️"))
        .await
        .unwrap();
    assert_eq!(
        second.delivery.kind(),
        if uncertain { "unknown" } else { "confirmed" }
    );
    assert_eq!(
        graph.calls.lock().unwrap().len(),
        if uncertain { 1 } else { 2 }
    );
    assert_stored_retention(db, &app, full_expiry, uncertain).await;
    Ok(())
}

async fn seed_channel(app: &App, records: &RecordClient) {
    app.replace();
    let row = app
        .backend
        .records()
        .remove(app.definition.record_id.as_str())
        .unwrap();
    records
        .put(
            &app.definition.record_id,
            &app.definition.owner.subject,
            &row.blob,
            None,
        )
        .await
        .unwrap();
}

async fn assert_stored_retention(db: &Database, app: &App, expiry: i64, uncertain: bool) {
    let rows: Vec<Document> = db
        .collection::<Document>(mongo::INTERACTIONS)
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let mut receipt_count = 0;
    let mut fence_count = 0;
    for row in rows {
        let id = row.get_str("_id").unwrap();
        let Ok(opened) = cmaeg_core::seal::open_interaction(
            &app.state.keys,
            id,
            row.get_binary_generic("blob").unwrap(),
        ) else {
            continue;
        };
        let sealed: Value = serde_json::from_slice(&opened.payload).unwrap();
        match sealed["kind"].as_str() {
            Some("whatsapp_received_reaction_v1") => receipt_count += 1,
            Some("whatsapp_received_mutation_v1") => {
                fence_count += 1;
                assert_eq!(sealed["held"], uncertain);
            }
            _ => continue,
        }
        assert_eq!(sealed["id"], id);
        assert_eq!(sealed["expires_at"], expiry);
        assert_eq!(
            row.get_datetime("expires_at").unwrap().timestamp_millis(),
            expiry * 1000
        );
        assert_eq!(row.get_i32("schema_version").unwrap(), 1);
        assert!(row.get_i64("version").unwrap() >= 1);
    }
    assert!(receipt_count >= 1);
    assert_eq!(fence_count, 1);
}
