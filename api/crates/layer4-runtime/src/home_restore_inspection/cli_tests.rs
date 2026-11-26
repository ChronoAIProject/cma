use super::*;

#[tokio::test]
async fn exact_command_and_malformed_invocations_never_reach_serving_startup() {
    struct Trap;
    impl Drop for Trap {
        fn drop(&mut self) {
            crate::TRAP_SERVING_STARTUP.set(false);
        }
    }
    crate::TRAP_SERVING_STARTUP.set(true);
    let _trap = Trap;
    assert_eq!(
        crate::run_arguments(vec![]).await.unwrap_err(),
        "serving_startup_trapped"
    );
    for arguments in [
        vec!["--inspect-restored-home"],
        vec!["--inspect-restored-home", "/absent/request.json"],
        vec!["--inspect-restored-home", "/absent/request.json", "extra"],
    ] {
        let error = crate::run_arguments(arguments.into_iter().map(OsString::from).collect())
            .await
            .unwrap_err();
        assert!(error.starts_with("restored_home_"), "{error}");
    }
    assert_eq!(
        crate::run_arguments(vec!["--unknown".into()])
            .await
            .unwrap_err(),
        "unknown runtime command"
    );
    assert!(crate::run_arguments(vec!["--cutover-ledger-v1".into()])
        .await
        .unwrap_err()
        .starts_with("mutation_ledger_import_held"));
    assert_ne!(
        crate::run_arguments(vec!["--export-home-snapshot".into()])
            .await
            .unwrap_err(),
        "serving_startup_trapped"
    );
}
