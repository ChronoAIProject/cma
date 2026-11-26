use super::*;

pub(super) const FIELD: &str = "keyed_publication_attempt_v1";
pub(super) type Trace = Arc<Mutex<Vec<CommandEvent>>>;
pub(super) type Hook = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

pub(super) fn now() -> String {
    layer2_core::now_rfc3339()
}
pub(super) fn deadline() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
}

pub(super) async fn authority(f: &Fixture) -> Sandbox {
    let mut parent = lifecycle_caller::parent(f).await;
    parent
        .work
        .schedule(SandboxWorkStep::RunLifecycleOperation, now());
    parent.revision += 1;
    f.store
        .save_sandbox(parent.revision - 1, parent.clone())
        .await
        .unwrap();
    f.store
        .create_worktree(Worktree {
            id: f.agent.worktree_id.clone(),
            workspace_id: f.agent.workspace_id.clone(),
            sandbox_id: f.agent.sandbox_id.clone(),
            owner: f.agent.owner.clone(),
            name: "wt-1".into(),
            path: "/home/cma/receipt/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: "a".repeat(40),
            status: WorktreeStatus::Ready,
            agent_id: Some(f.agent.id.clone()),
            revision: 1,
        })
        .await
        .unwrap();
    f.store
        .claim_sandbox_work(&parent.id, "precommit-worker", now(), deadline())
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn observed(f: &mut Fixture) -> (Client, String, Trace, Hook, Hook) {
    let admin = f.client.clone();
    let app = format!("m4-precommit-{}", uuid::Uuid::new_v4().simple());
    let trace: Trace = Arc::default();
    let seen = trace.clone();
    let hook: Hook = Arc::default();
    let observer_hook = hook.clone();
    let abort_hook: Hook = Arc::default();
    let observer_abort = abort_hook.clone();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let abort_acknowledged = matches!(&event, CommandEvent::Succeeded(reply)
            if reply.command_name == "abortTransaction"
                && !reply.reply.contains_key("writeConcernError")
                && !reply.reply.contains_key("errorLabels"));
        let publication_inserted = {
            let mut events = seen.lock().unwrap();
            let matched = matches!(&event, CommandEvent::Succeeded(reply)
                if events.iter().any(|started| matches!(started,
                    CommandEvent::Started(start) if start.request_id == reply.request_id
                        && start.command.get_str("insert").ok() == Some(PUBLICATIONS))));
            events.push(event);
            matched
        };
        // The synchronous driver callback runs after the actual receipt insert
        // succeeds and before the transaction can send commit. No product hook.
        if abort_acknowledged {
            if let Some(action) = observer_abort.lock().unwrap().take() {
                action();
            }
        }
        if publication_inserted {
            let close = observer_hook.lock().unwrap().take();
            if let Some(close) = close {
                close();
            }
        }
    }));
    f.client = Client::with_options(options).unwrap();
    f.store = MongoSandboxStore::open(&f.client, &f.store.agents.namespace().db)
        .await
        .unwrap()
        .with_renewable_work(true);
    (admin, app, trace, hook, abort_hook)
}

pub(super) fn succeeded_command(events: &[CommandEvent], request_id: i32) -> bool {
    events.iter().any(|event| {
        matches!(event,
        CommandEvent::Succeeded(reply) if reply.request_id == request_id)
    })
}

pub(super) fn update(command: &Document) -> Option<(&Document, &Document)> {
    if command.get_str("update").ok() != Some("agents") {
        return None;
    }
    let row = command.get_array("updates").ok()?.first()?.as_document()?;
    Some((row.get_document("q").ok()?, row.get_document("u").ok()?))
}

pub(super) fn acquired_marker(events: &[CommandEvent]) -> Document {
    events
        .iter()
        .find_map(|event| {
            let CommandEvent::Started(event) = event else {
                return None;
            };
            if event.command.contains_key("autocommit")
                || !succeeded_command(events, event.request_id)
            {
                return None;
            }
            let (_, change) = update(&event.command)?;
            if event
                .command
                .get_document("writeConcern")
                .ok()?
                .get_str("w")
                .ok()
                != Some("majority")
            {
                return None;
            }
            change
                .get_document("$set")
                .ok()?
                .get_document(FIELD)
                .ok()
                .cloned()
        })
        .expect("the exact majority durable claim must exist before publication")
}

pub(super) fn close_at_guard(
    trace: Trace,
    authority: WorkAuthority,
    calls: Arc<AtomicUsize>,
) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        assert_eq!(
            calls.fetch_add(1, Ordering::SeqCst),
            0,
            "guard must not be retried"
        );
        let events = trace.lock().unwrap();
        let marker = acquired_marker(&events);
        assert!(
            events.iter().any(|event| {
                let CommandEvent::Started(event) = event else {
                    return false;
                };
                let Some((query, change)) = update(&event.command) else {
                    return false;
                };
                event.command.get_bool("autocommit").ok() == Some(false)
                    && query.get_document(FIELD).ok() == Some(&marker)
                    && change
                        .get_document("$unset")
                        .is_ok_and(|fields| fields.contains_key(FIELD))
                    && succeeded_command(&events, event.request_id)
            }),
            "publication must have consumed this exact durable marker transactionally"
        );
        assert!(
            events.iter().any(|event| matches!(event,
            CommandEvent::Started(start)
                if start.command.get_str("insert").ok() == Some(PUBLICATIONS)
                    && succeeded_command(&events, start.request_id))),
            "actual publication receipt insertion must finish before the guard"
        );
        drop(events);
        // Closing a polled clone uses the real shared CloseOnDrop validity flag.
        // No clock advance can let the outer biased timeout cancel cleanup first.
        // The enclosing run has already completed its actual Mongo heartbeat.
        let pending = authority.run(
            std::future::pending::<Result<(), SandboxStoreError>>(),
            std::future::pending::<Result<Option<WorkLeaseGrant>, SandboxStoreError>>,
        );
        assert!(pending.now_or_never().is_none());
        assert!(
            WorkAuthority::current().is_err(),
            "guard must see closed authority"
        );
    })
}
