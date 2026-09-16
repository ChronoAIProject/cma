use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_http_disabled_fallback_recovers_enabled_winner_in_one_complete_transaction(
) {
    let c = ControlFixture::with_callback_route().await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
    let application = format!("control-recovery-race-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(application.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    let disabled = disabled_app_with_client(&c, &client).await;
    let enabled = app(&c, true, Some(Runtime::local(&c))).await;
    let alias = body(&c, "disabled-raced-alias", 744, false);
    let original = body(&c, "enabled-raced-original", 744, false);
    let admin = c.f.client.database("admin");
    // Pause the disabled request's first, read-only recovery commit. Its
    // snapshot found no acceptance; the enabled request commits before that
    // observation returns and before the disabled path enters fresh fallback.
    let configured = admin
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1_i32},
            "data":{"failCommands":["commitTransaction"], "appName":&application,
            "blockConnection":true, "blockTimeMS":3000_i32},
        })
        .await
        .unwrap();
    let entered = match configured.get("count").unwrap() {
        mongodb::bson::Bson::Int32(count) => i64::from(*count) + 1,
        mongodb::bson::Bson::Int64(count) => *count + 1,
        _ => panic!("failpoint entry count must be an integer"),
    };
    let competing = async {
        admin
            .run_command(
                doc! {"waitForFailPoint":"failCommand", "timesEntered":entered,
                "maxTimeMS":1500_i32},
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), post(enabled, &c, &original)).await
    };
    let (disabled_result, enabled_result) = tokio::join!(post(disabled, &c, &alias), competing);
    admin
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let (enabled_status, accepted) =
        enabled_result.expect("enabled admission wins while recovery is paused");
    assert_eq!(enabled_status, StatusCode::ACCEPTED, "{accepted}");
    assert_eq!(accepted["status"], "control_accepted");
    assert_eq!(
        disabled_result.0,
        StatusCode::ACCEPTED,
        "{}",
        disabled_result.1
    );
    assert_eq!(disabled_result.1, accepted);
    assert_eq!(
        rows(&c, "bridge_channel_control_aliases").await,
        2,
        "disabled fallback must commit the raced v2 alias with its complete intent graph"
    );
    no_message_effects(&c, 1).await;
    c.f.close().await;
}
