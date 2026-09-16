use super::*;
use tokio::time::Instant;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_unknown_commit_recovers_original_terminal_receipt() {
    let mut c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "unknown-held-denial").await;
    denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
    let app_name = temporal::scope_execution_commits(&mut c).await;
    let admin = c.f.client.database("admin");
    let configured = admin.run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":app_name,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"owned held denial commit acknowledgement lost"}}
    }).await.unwrap();
    let result = denial::refusal_store(&c)
        .refuse_control_hold(&held.command, Instant::now() + Duration::from_secs(15))
        .await;
    let stopped = admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(ChannelControlError::Unavailable)));
    assert_eq!(
        temporal::failpoint_count(&stopped) - temporal::failpoint_count(&configured),
        5
    );
    denial::assert_refused(&c, &held, CommandFailureCode::AuthorityRevoked).await;
    // Recovery uses the existing command; a stale transition cannot overwrite it.
    assert!(denial::refusal_store(&c)
        .refuse_control_hold(&held.command, Instant::now() + Duration::from_secs(15))
        .await
        .is_err());
    denial::assert_refused(&c, &held, CommandFailureCode::AuthorityRevoked).await;
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_transient_transaction_retries_only_under_original_deadline() {
    let mut c = ControlFixture::new().await;
    let held = worker::admit(&c, true, "transient-held-denial").await;
    denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
    let app_name = temporal::scope_execution_commits(&mut c).await;
    let admin = c.f.client.database("admin");
    let configured = admin
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":1_i32},
            "data":{"failCommands":["update"],"appName":app_name,"errorCode":112_i32,
                "errorLabels":["TransientTransactionError"]}
        })
        .await
        .unwrap();
    let result = denial::refusal_store(&c)
        .refuse_control_hold(&held.command, Instant::now() + Duration::from_secs(15))
        .await;
    let stopped = admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(
        temporal::failpoint_count(&stopped) - temporal::failpoint_count(&configured),
        1
    );
    result.unwrap();
    denial::assert_refused(&c, &held, CommandFailureCode::AuthorityRevoked).await;
    c.f.close().await;
}

struct DelayedDenial {
    deadline: std::sync::Mutex<Option<Instant>>,
    inner: Option<Arc<dyn ChannelControlExecutionStore>>,
}
#[async_trait::async_trait]
impl ChannelControlExecutionStore for DelayedDenial {
    async fn read_control_execution(
        &self,
        _: &AdmittedAgentCommand,
        deadline: Instant,
    ) -> Result<ChannelControlExecutionSnapshot, ChannelControlError> {
        *self.deadline.lock().unwrap() = Some(deadline);
        Err(ChannelControlError::AuthorityUnavailable)
    }
    async fn release_control_hold(
        &self,
        _: Arc<ChannelControlExecutionFence>,
        _: &AdmittedAgentCommand,
    ) -> Result<(), ChannelControlError> {
        panic!("unproved authority cannot release work")
    }
    async fn expire_control_hold(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), ChannelControlError> {
        panic!("fixture retains its real queue deadline")
    }
    async fn refuse_control_hold(
        &self,
        command: &AdmittedAgentCommand,
        deadline: Instant,
    ) -> Result<(), ChannelControlError> {
        assert_eq!(*self.deadline.lock().unwrap(), Some(deadline));
        if let Some(inner) = &self.inner {
            return inner.refuse_control_hold(command, deadline).await;
        }
        tokio::time::sleep_until(deadline + Duration::from_secs(1)).await;
        panic!("the original deadline must cancel uncertain denial work")
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_timeout_retains_hold_and_never_renews_release_attempt() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "bounded-held-denial").await;
    let delayed = Arc::new(DelayedDenial {
        deadline: std::sync::Mutex::new(None),
        inner: None,
    });
    let service = ChannelControlExecutionService::new(delayed.clone(), c.f.invokes.clone());
    assert!(matches!(
        service
            .release_held(&held.command, &worker::operations(&c))
            .await,
        Err(ChannelControlError::Unavailable)
    ));
    assert!(Instant::now() >= delayed.deadline.lock().unwrap().unwrap());
    assert!(denial::stored(&c, &held).await == held);
    // Even direct calls with an already expired attempt do no transaction work.
    denial::consent(&c, 1, SourceControlAction::RevokeSenderControls).await;
    assert!(matches!(
        denial::refusal_store(&c)
            .refuse_control_hold(&held.command, Instant::now())
            .await,
        Err(ChannelControlError::Unavailable)
    ));
    assert!(denial::stored(&c, &held).await == held);
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_held_denial_uncertain_authority_with_unchanged_local_facts_stays_held() {
    let c = ControlFixture::new().await;
    let held = worker::admit(&c, false, "uncertain-authority-held-control").await;
    let uncertain = Arc::new(DelayedDenial {
        deadline: std::sync::Mutex::new(None),
        inner: Some(denial::refusal_store(&c)),
    });
    let service = ChannelControlExecutionService::new(uncertain, c.f.invokes.clone());
    assert!(service
        .release_held(&held.command, &worker::operations(&c))
        .await
        .is_err());
    assert!(denial::stored(&c, &held).await == held);
    assert_eq!(c.f.items().await.len(), 1);
    c.f.close().await;
}
