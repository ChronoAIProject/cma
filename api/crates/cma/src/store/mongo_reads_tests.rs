use super::*;
use mongodb::bson::{doc, rawdoc, RawDocumentBuf};

fn totals(observer: &Observer, operation: &str, collection: &str) -> [u64; 6] {
    let t = &observer.0[observer.classify(operation, Some(collection)) as usize];
    [
        &t.responses,
        &t.failures,
        &t.documents,
        &t.bytes,
        &t.document_bytes,
        &t.incomplete,
    ]
    .map(|v| v.load(Ordering::Relaxed))
}

#[test]
fn raw_batches_and_single_values_have_exact_encoded_lengths() {
    let record = rawdoc! {"payload": "private contents", "nested": {"value": [1, 2, 3]}};
    for batch in ["firstBatch", "nextBatch"] {
        let reply: RawDocumentBuf =
            doc! {"ok":1, "cursor": {batch: [mongodb::bson::Document::try_from(&record).unwrap()]}}
                .try_into()
                .unwrap();
        let summary = ResponseSummary::from_reply(&reply);
        assert!(summary.succeeded && summary.complete);
        assert_eq!(summary.documents, 1);
        assert_eq!(summary.document_bytes, record.as_bytes().len() as u64);
        assert_eq!(summary.reply_bytes, reply.as_bytes().len() as u64);
    }
    let reply = rawdoc! {"ok":1.0, "value": record.clone()};
    let summary = ResponseSummary::from_reply(&reply);
    assert_eq!(summary.documents, 1);
    assert_eq!(summary.document_bytes, record.as_bytes().len() as u64);
    assert_eq!(summary.reply_bytes, reply.as_bytes().len() as u64);
    let empty = ResponseSummary::from_reply(&rawdoc! {"ok":1, "cursor":{"firstBatch":[]}});
    assert!(empty.complete && empty.succeeded);
    assert_eq!(empty.documents, 0);
}

#[test]
fn errors_retain_received_bytes_and_incomplete_summaries_are_explicit() {
    let observer = Observer::default();
    let class = observer.classify("find", Some("agent_log"));
    let error = rawdoc! {"ok":0, "errmsg":"private error details"};
    observer.observe(class, ResponseSummary::from_reply(&error));
    observer.observe(class, ResponseSummary::default()); // transport failure, no reply
    assert_eq!(
        totals(&observer, "find", "agent_log"),
        [2, 2, 0, error.as_bytes().len() as u64, 0, 0]
    );
    let invalid = rawdoc! {"ok":1, "cursor":{"firstBatch":[1]}};
    observer.observe(class, ResponseSummary::from_reply(&invalid));
    assert_eq!(totals(&observer, "find", "agent_log")[5], 1);
    let t = &observer.0[class as usize];
    t.bytes.store(u64::MAX, Ordering::Relaxed);
    observer.observe(class, ResponseSummary::from_reply(&error));
    assert_eq!(t.bytes.load(Ordering::Relaxed), u64::MAX);
}

#[test]
#[ignore = "local raw-summary overhead measurement; no database required"]
fn raw_summary_overhead_for_five_thousand_documents() {
    use std::{hint::black_box, time::Instant};
    let records =
        vec![doc! {"seq":1_i64,"record":{"kind":"agent_message","content":"x".repeat(768)}}; 5_000];
    let reply: RawDocumentBuf = doc! {"ok":1.0,"cursor":{"firstBatch":records}}
        .try_into()
        .unwrap();
    let observer = Observer::default();
    let class = observer.classify("getMore", Some("agent_log"));
    let mut summaries = Vec::new();
    let mut decodes = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..100 {
            let summary = ResponseSummary::from_reply(black_box(&reply));
            assert_eq!(summary.documents, 5_000);
            observer.observe(class, black_box(summary));
        }
        summaries.push(start.elapsed().as_nanos() / 100);
        let start = Instant::now();
        for _ in 0..20 {
            black_box(mongodb::bson::Document::try_from(black_box(&reply)).unwrap());
        }
        decodes.push(start.elapsed().as_nanos() / 20);
    }
    summaries.sort_unstable();
    decodes.sort_unstable();
    println!(
        "raw_summary documents=5000 reply_bytes={} summary_median_ns={} decode_median_ns={}",
        reply.as_bytes().len(),
        summaries[2],
        decodes[2]
    );
}

#[test]
fn arbitrary_collection_and_operation_names_cannot_expand_the_label_set() {
    let observer = Observer::default();
    let mut classes = std::collections::HashSet::new();
    for collection in COLLECTIONS.iter().copied().chain(std::iter::once("other")) {
        for operation in OPERATIONS {
            classes.insert(observer.classify(operation, Some(collection)));
        }
    }
    assert_eq!(classes.len(), CLASSES);
    assert!(CLASSES <= 256 * OPERATIONS.len());
    for n in 0..10_000 {
        let unknown = format!("private_{n}");
        assert_eq!(
            observer.classify(&unknown, Some(&unknown)),
            observer.classify("other", Some("other"))
        );
        assert_eq!(
            observer.classify("getMore", Some(&unknown)),
            observer.classify("getMore", Some("other"))
        );
    }
    assert_eq!(
        observer.classify("find", None),
        observer.classify("find", Some("other"))
    );
}

#[derive(Clone, Default)]
struct Output(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[test]
fn periodic_metrics_exclude_values_unknown_names_and_inherited_spans() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let private = "private request and document contents";
    let span = tracing::info_span!("request", secret = private);
    let _entered = span.enter();
    let observer = Observer::default();
    let reply = rawdoc! {"ok":1, "cursor":{"firstBatch":[{"private":private}]}};
    observer.observe(
        observer.classify(private, Some(private)),
        ResponseSummary::from_reply(&reply),
    );
    assert!(output.0.lock().unwrap().is_empty());
    observer.emit();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains(private));
    assert!(logs.contains("collection=\"other\"") && logs.contains("operation=\"other\""));
    assert!(
        logs.contains("cma_mongo_read_bytes_total")
            && logs.contains("cma_mongo_read_documents_total")
    );
    assert_eq!(logs.lines().count(), 6);
    observer.emit();
    assert_eq!(output.0.lock().unwrap().len(), logs.len());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn real_driver_counts_received_batches_abandonment_getmore_and_failed_reads() {
    use futures_util::TryStreamExt;
    use mongodb::{bson::Document, Client};
    let observer = Arc::new(Observer::default());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.response_observer = Some(observer.clone());
    options.retry_reads = Some(false);
    assert!(options.command_event_handler.is_none());
    let client = Client::with_options(options).unwrap();
    let database = client.database(&format!("cma_reads_{}", uuid::Uuid::new_v4().simple()));
    let collection = database.collection::<Document>("agent_log");
    let docs: Vec<_> = (0..7)
        .map(|i| doc! {"_id":i,"content":"private fixture contents"})
        .collect();
    let bytes = docs[0].to_vec().unwrap().len() as u64;
    collection.insert_many(docs).await.unwrap();
    // Count all received firstBatch documents even without consuming the cursor.
    let abandoned = collection.find(doc! {}).batch_size(2).await.unwrap();
    assert_eq!(totals(&observer, "find", "agent_log")[2], 2);
    drop(abandoned);
    let mut session = client.start_session().await.unwrap();
    let mut cursor = collection
        .find(doc! {})
        .batch_size(2)
        .session(&mut session)
        .await
        .unwrap();
    let received: Vec<_> = cursor.stream(&mut session).try_collect().await.unwrap();
    assert_eq!(received.len(), 7);
    let find = totals(&observer, "find", "agent_log");
    assert_eq!([find[0], find[1], find[2], find[4]], [2, 0, 4, 4 * bytes]);
    let more = totals(&observer, "getMore", "agent_log");
    assert_eq!([more[0], more[1], more[2], more[4]], [3, 0, 5, 5 * bytes]);
    assert!(find[3] > find[4] && more[3] > more[4]);
    assert!(collection.find_one(doc! {"_id":0}).await.unwrap().is_some());
    assert_eq!(totals(&observer, "find", "agent_log")[2], 5);
    let rows: Vec<_> = collection
        .aggregate([doc! {"$limit":1}])
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(totals(&observer, "aggregate", "agent_log")[2], 1);
    collection
        .find_one_and_update(doc! {"_id":0}, doc! {"$set":{"content":"changed"}})
        .await
        .unwrap();
    assert_eq!(totals(&observer, "findAndModify", "agent_log")[2], 1);
    assert!(collection.find(doc! {"$invalid":1}).await.is_err());
    assert_eq!(totals(&observer, "find", "agent_log")[1], 1);
    // The fixture enables failCommand. A single connection close exercises the
    // driver's transport-error path; it must not invent received BSON bytes.
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["find"],"closeConnection":true}
        })
        .await
        .unwrap();
    let before = totals(&observer, "find", "agent_log");
    assert!(collection.find(doc! {}).await.is_err());
    let after = totals(&observer, "find", "agent_log");
    assert_eq!(after[0], before[0] + 1);
    assert_eq!(after[1], before[1] + 1);
    assert_eq!(&after[2..], &before[2..]);
    let unknown = database.collection::<Document>("unlisted_private_collection");
    unknown.insert_one(doc! {"_id":1}).await.unwrap();
    assert!(unknown.find_one(doc! {}).await.unwrap().is_some());
    assert_eq!(totals(&observer, "find", "other")[2], 1);
    database.drop().await.unwrap();
}
