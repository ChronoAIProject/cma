//! A known-aborted commit must actually release its server transaction.
use mongodb::{
    bson::{doc, Document},
    error::{Error, ErrorKind, TRANSIENT_TRANSACTION_ERROR, UNKNOWN_TRANSACTION_COMMIT_RESULT},
    event::command::CommandEvent,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

const FAILPOINT: &str = "failWithErrorCodeAfterSessionCheckOut";

pub(super) struct CommitAbort {
    uri: String,
    armed: AtomicBool,
    injected: AtomicUsize,
    commits: Mutex<Vec<(i32, Document)>>,
    failures: Mutex<Vec<(i32, Error)>>,
    replies: Mutex<Vec<(i32, Document)>>,
}

impl CommitAbort {
    pub(super) fn new(uri: String) -> Self {
        Self {
            uri,
            armed: AtomicBool::new(false),
            injected: AtomicUsize::new(0),
            commits: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            replies: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    pub(super) fn observe(&self, event: &CommandEvent) {
        match event {
            CommandEvent::Started(event) if event.command_name == "commitTransaction" => {
                let inject = self.armed.swap(false, Ordering::SeqCst);
                if !inject && self.injected.load(Ordering::SeqCst) == 0 {
                    return;
                }
                let identity = doc! {
                    "lsid":event.command.get_document("lsid").unwrap().clone(),
                    "txnNumber":event.command.get_i64("txnNumber").unwrap(),
                };
                self.commits
                    .lock()
                    .unwrap()
                    .push((event.request_id, identity));
                assert!(matches!(event.command.get_bool("autocommit"), Ok(false)));
                if !inject {
                    return;
                }
                let uri = self.uri.clone();
                // This failpoint is global. Use only the campaign's exclusive
                // disposable server, with serial tests and no other transaction
                // producer. Arm at this exact commit, after all fixture setup.
                std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async {
                            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                                let admin = Client::with_uri_str(uri).await.unwrap();
                                let result = admin
                                    .database("admin")
                                    .run_command(doc! {
                                        "configureFailPoint":FAILPOINT,
                                        "mode":{"times":1},
                                        "data":{"errorCode":112_i32},
                                    })
                                    .await;
                                admin.shutdown().await;
                                result.expect("arm abort after server transaction checkout");
                            })
                            .await
                            .expect("commit abort setup must finish before command dispatch");
                        });
                })
                .join()
                .expect("joined setup must finish before the intercepted commit sends");
                self.injected.fetch_add(1, Ordering::SeqCst);
            }
            CommandEvent::Failed(event)
                if event.command_name == "commitTransaction"
                    && self.injected.load(Ordering::SeqCst) > 0 =>
            {
                self.failures
                    .lock()
                    .unwrap()
                    .push((event.request_id, event.failure.clone()));
            }
            CommandEvent::Succeeded(event)
                if event.command_name == "commitTransaction"
                    && self.injected.load(Ordering::SeqCst) > 0 =>
            {
                self.replies
                    .lock()
                    .unwrap()
                    .push((event.request_id, event.reply.clone()));
            }
            _ => {}
        }
    }

    pub(super) async fn disable(&self, admin: &Client) {
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":FAILPOINT,"mode":"off"})
            .await
            .expect("disable exact checkout failpoint before observations");
    }

    pub(super) fn assert_one_aborted_commit(&self) {
        assert_eq!(self.injected.load(Ordering::SeqCst), 1);
        let commits = self.commits.lock().unwrap();
        assert_eq!(
            commits.len(),
            2,
            "original and successor commits: {commits:?}"
        );
        assert_ne!(
            commits[0].1, commits[1].1,
            "retry needs a fresh lsid/txnNumber"
        );
        let failures = self.failures.lock().unwrap();
        assert_eq!(
            failures.len(),
            1,
            "one real aborted commit, no extra failure"
        );
        assert_eq!(
            failures[0].0, commits[0].0,
            "failure must match the first commit request"
        );
        let failure = &failures[0].1;
        assert!(
            matches!(failure.kind.as_ref(), ErrorKind::Command(command) if command.code == 112)
        );
        // The server supplies this label after its checked-out transaction is
        // aborted. No errorLabels override may stand in for that cleanup.
        assert!(failure.contains_label(TRANSIENT_TRANSACTION_ERROR));
        assert!(!failure.contains_label(UNKNOWN_TRANSACTION_COMMIT_RESULT));
        let replies = self.replies.lock().unwrap();
        assert_eq!(
            replies.len(),
            1,
            "one successful successor reply: {replies:?}"
        );
        assert_eq!(
            replies[0].0, commits[1].0,
            "reply must match the fresh successor"
        );
        assert!(!replies[0].1.contains_key("writeConcernError"));
    }
}
