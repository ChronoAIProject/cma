use super::*;

macro_rules! retry_at {
    ($name:ident, $command:literal, $collection:literal) => {
        #[tokio::test]
        #[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
        async fn $name() { recover_retry_case($command, Some($collection)).await; }
    };
}
retry_at!(
    credential_read_abort_retains_original_intent,
    "find",
    "sandbox_credentials"
);
retry_at!(
    sandbox_read_abort_retains_original_intent,
    "find",
    "sandboxes"
);
retry_at!(
    workspace_read_abort_retains_original_intent,
    "find",
    "workspaces"
);
retry_at!(
    sandbox_write_abort_retains_original_intent,
    "update",
    "sandboxes"
);
retry_at!(
    workspace_write_abort_retains_original_intent,
    "update",
    "workspaces"
);
retry_at!(child_read_abort_retains_original_intent, "find", "agents");
retry_at!(
    child_write_abort_retains_original_intent,
    "update",
    "agents"
);
