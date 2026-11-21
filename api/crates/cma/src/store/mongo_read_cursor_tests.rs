use super::*;
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client,
};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn raw_cursor_lengths_cover_sessions_getmore_and_consumed_rows_only() {
    isolated_test(async {
        let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let database = client.database(&format!(
            "cma_read_cursor_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let collection = database.collection::<Document>("agents");
        let documents: Vec<_> = (0..7)
            .map(|i| doc! {"_id":i,"payload":"private fixture data"})
            .collect();
        let bytes = documents[0].to_vec().unwrap().len() as u64;
        collection.insert_many(documents.clone()).await.unwrap();
        {
            let mut read = Read::find(collection.name());
            let mut cursor = collection
                .find(doc! {})
                .sort(doc! {"_id":1})
                .batch_size(2)
                .await
                .unwrap();
            assert_eq!(
                read.next(&mut cursor).await.unwrap(),
                Some(documents[0].clone())
            );
            read.complete(); // caller intentionally needs just one row
        }
        assert_eq!(snapshot("agents"), [1, 0, 1, bytes]); // received but unconsumed row is excluded
        {
            let mut read = Read::find(collection.name());
            let mut session = client.start_session().await.unwrap();
            let mut cursor = collection
                .find(doc! {})
                .sort(doc! {"_id":1})
                .batch_size(2)
                .session(&mut session)
                .await
                .unwrap();
            let mut actual = Vec::new();
            while let Some(row) = read
                .next_in_session(&mut cursor, &mut session)
                .await
                .unwrap()
            {
                actual.push(row);
            }
            assert_eq!(actual, documents);
            read.complete();
        }
        assert_eq!(snapshot("agents"), [2, 0, 8, 8 * bytes]);
        {
            let mut read = Read::find(collection.name());
            let mut cursor = collection.find(doc! {"_id":-1}).await.unwrap();
            assert!(read.next(&mut cursor).await.unwrap().is_none());
            read.complete();
        }
        assert_eq!(snapshot("agents"), [3, 0, 8, 8 * bytes]);
        database.drop().await.unwrap();
    })
    .await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner with failpoints"]
async fn cursor_decode_and_getmore_failures_keep_partial_counts_without_extra_decoding() {
    isolated_test(async {
        let app = format!("cma-read-errors-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(app.clone());
        options.retry_reads = Some(false);
        let client = Client::with_options(options).unwrap();
        let database = client.database(&format!(
            "cma_read_errors_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let collection = database.collection::<Document>("sandbox_events");
        let documents: Vec<_> = (0..4)
            .map(|i| doc! {"_id":i,"payload":"private fixture data"})
            .collect();
        let bytes = documents[0].to_vec().unwrap().len() as u64;
        collection.insert_many(documents).await.unwrap();
        {
            let _read = Read::find(collection.name());
            assert!(collection.find(doc! {"$invalid":1}).await.is_err());
        }
        assert_eq!(snapshot("sandbox_events"), [1, 1, 0, 0]);
        {
            // Fails type decoding, with no extra serialization needed to count the row.
            let mut read = Read::find(collection.name());
            let mut cursor = collection
                .clone_with_type::<u64>()
                .find(doc! {})
                .await
                .unwrap();
            assert!(read.next(&mut cursor).await.is_err());
        }
        assert_eq!(snapshot("sandbox_events"), [2, 2, 1, bytes]);
        {
            let mut read = Read::find(collection.name());
            let mut cursor = collection.find(doc! {}).batch_size(2).await.unwrap();
            assert!(read.next(&mut cursor).await.unwrap().is_some());
            assert!(read.next(&mut cursor).await.unwrap().is_some());
            client
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":{"times":1},
                    "data":{"failCommands":["getMore"],"appName":app,"errorCode":43}
                })
                .await
                .unwrap();
            assert!(read.next(&mut cursor).await.is_err());
        }
        assert_eq!(snapshot("sandbox_events"), [3, 3, 3, 3 * bytes]);
        database.drop().await.unwrap();
    })
    .await;
}
