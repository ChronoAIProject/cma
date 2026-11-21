use super::*;
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::{Arc, Mutex};

type Observed = Arc<Mutex<Vec<(String, Document)>>>;

pub(super) async fn begin(store: &ArchiveStore) -> (ArchiveStore, Observed) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app = format!("archive-reconcile-{}", uuid::Uuid::new_v4());
    let observed = Arc::new(Mutex::new(Vec::new()));
    let commands = Arc::clone(&observed);
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            commands
                .lock()
                .unwrap()
                .push((event.command_name, event.command));
        }
    }));
    let client = Client::with_options(options).unwrap();
    let archive = ArchiveStore {
        database: client.database(store.database.name()),
        client,
    };
    store
        .client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":5}, "data":{
                "failCommands":["commitTransaction"], "appName":app,
                "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired",
                    "errmsg":"injected archive acknowledgement loss"}
            }
        })
        .await
        .unwrap();
    (archive, observed)
}

pub(super) async fn finish(store: &ArchiveStore, observed: Observed) {
    store
        .client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let commands = observed.lock().unwrap();
    let commits: Vec<_> = commands
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| name == "commitTransaction")
        .collect();
    assert_eq!(commits.len(), 5);
    let reads: Vec<_> = commands
        .iter()
        .skip(commits.last().unwrap().0 + 1)
        .filter(|(name, _)| name == "find")
        .collect();
    assert_eq!(reads.len(), 1);
    let concern = reads[0].1.get_document("readConcern").unwrap();
    assert_eq!(concern.get_str("level").unwrap(), "snapshot");
    concern.get_timestamp("atClusterTime").unwrap();
}
