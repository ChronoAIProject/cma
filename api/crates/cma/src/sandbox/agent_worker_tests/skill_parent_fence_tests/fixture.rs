use super::*;
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;

#[derive(Default, Clone)]
pub(super) struct Observation {
    pub armed: bool,
    pub commits: usize,
    pub update_failures: usize,
    pub readback_injected: bool,
}

pub(super) struct ProbeFixture {
    pub h: Harness<MongoSandboxStore>,
    pub skills: Arc<ScriptedSkills>,
    pub claimed: Agent,
    pub admin: Client,
    pub app: String,
    pub database: String,
    pub observation: Arc<Mutex<Observation>>,
}

pub(super) async fn clear(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": "off"
        })
        .await
        .unwrap();
}

pub(super) async fn create(case: Case, segmented: bool) -> ProbeFixture {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("skill-fence-publication-{suffix}");
    let database = format!("cma_skill_fence_publication_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let observation = Arc::new(Mutex::new(Observation::default()));
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let state = observation.clone();
    let event_app = app.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut observed = state.lock().unwrap();
        if !observed.armed {
            return;
        }
        let completed = match event {
            CommandEvent::Started(e) => {
                if e.command_name == "commitTransaction" {
                    observed.commits += 1;
                }
                false
            }
            CommandEvent::Failed(e) => {
                if e.command_name == "update" {
                    observed.update_failures += 1;
                }
                e.command_name == "commitTransaction"
            }
            CommandEvent::Succeeded(e) => e.command_name == "commitTransaction",
            _ => false,
        };
        if matches!(case, Case::FailedReadback)
            && completed
            && observed.commits == 5
            && !observed.readback_injected
        {
            // A synchronous event hook establishes ordering before receipt reads.
            // The independent runtime/client avoids blocking this client's IO loop.
            let (uri, app) = (uri.clone(), event_app.clone());
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        let admin = Client::with_uri_str(uri).await.unwrap();
                        admin
                            .database("admin")
                            .run_command(doc! {
                                "configureFailPoint":"failCommand", "mode":{"times":100_i32},
                                "data":{"failCommands":["find"],"appName":app,"errorCode":1_i32}
                            })
                            .await
                            .unwrap();
                    });
            })
            .join()
            .unwrap();
            observed.readback_injected = true;
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, &database)
            .await
            .unwrap()
            .with_segment_adoption_enabled(segmented),
    );
    let material = super::super::runtime_skill("7.0", 'a', "archive");
    let skills = ScriptedSkills::new([Ok(vec![]), Ok(vec![material])]);
    let mut h = Harness::with_store(store, skills.clone(), 1).await;
    h.now = SystemTime::now();
    let admitted = h.admit("lv2-publication-probe", vec![]).await;
    for _ in 0..250 {
        let current = h.stored(&admitted.id).await;
        if current.status == AgentStatus::Ready && current.work.step == AgentWorkStep::None {
            break;
        }
        h.worker
            .run_one(&admitted.id, SystemTime::now())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ready = h.stored(&admitted.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::None);
    if segmented {
        crate::store::skill_result_test_support::enable_segments(&h.store, &ready).await;
    }
    AgentOperationService::new(h.store.clone(), h.registry.clone())
        .install_skill(
            &Caller::new(
                ready.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            &ready.id,
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                version: Some("7.0".into()),
            },
        )
        .await
        .unwrap();
    crate::store::skill_result_test_support::assert_format(&h.store, &ready.id, segmented).await;
    if matches!(case, Case::Refusal) {
        h.backend.fail_next(
            WireOperation::InstallAgentSkill,
            "request_rejected",
            "synthetic skill refusal",
        );
    }
    h.now = SystemTime::now();
    let claimed = h
        .store
        .claim_agent_work(
            &ready.id,
            "agent-worker",
            rfc3339_at(h.now),
            rfc3339_at(h.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    ProbeFixture {
        h,
        skills,
        claimed,
        admin,
        app,
        database,
        observation,
    }
}
