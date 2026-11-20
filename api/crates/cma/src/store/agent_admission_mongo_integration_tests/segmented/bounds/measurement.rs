use super::*;

#[derive(Clone, Debug, Default)]
pub(super) struct Trace {
    pub commands: usize,
    pub index_inventories: usize,
    pub source_rows: usize,
    pub source_bytes: usize,
    pub max_source_batch: usize,
    pub max_reply_bytes: usize,
}

pub(super) async fn observed(database: &str) -> (MongoSandboxStore, Arc<Mutex<Trace>>) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let observed = trace.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Succeeded(event) = event {
            let mut trace = observed.lock().unwrap();
            trace.commands += 1;
            trace.max_reply_bytes = trace
                .max_reply_bytes
                .max(mongodb::bson::serialize_to_vec(&event.reply).unwrap().len());
            if event.command_name == "listIndexes" {
                trace.index_inventories += 1;
            }
            if !matches!(event.command_name.as_str(), "find" | "getMore") {
                return;
            }
            if let Ok(cursor) = event.reply.get_document("cursor") {
                if cursor
                    .get_str("ns")
                    .is_ok_and(|ns| ns.ends_with(".agent_log"))
                {
                    let rows = cursor
                        .get_array("firstBatch")
                        .or_else(|_| cursor.get_array("nextBatch"))
                        .unwrap();
                    trace.source_rows += rows.len();
                    trace.max_source_batch = trace.max_source_batch.max(rows.len());
                    trace.source_bytes += rows
                        .iter()
                        .map(|row| {
                            mongodb::bson::serialize_to_vec(row.as_document().unwrap())
                                .unwrap()
                                .len()
                        })
                        .sum::<usize>();
                }
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, database).await.unwrap();
    *trace.lock().unwrap() = Trace::default();
    (store, trace)
}
