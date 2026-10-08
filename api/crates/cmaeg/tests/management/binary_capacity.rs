//! Startup validates routed limits and notices even while routing is disabled.
use super::{
    binary_support::{assert_no_secrets, MountedSecrets},
    *,
};
use std::{ffi::OsString, time::Duration};

type Environment = Vec<(&'static str, OsString)>;

#[tokio::test]
async fn capacity_and_notice_environment_is_validated_without_echoing_values() {
    let provider = FakeProvider::start(Default::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let mut cases: Vec<(Environment, Option<&str>)> = vec![
        (vec![], None),
        (
            vec![
                ("CMAEG_ROUTED_CHANNEL_MAX_TURNS", "256".into()),
                ("CMAEG_ROUTED_OWNER_MAX_TURNS", "1024".into()),
                (
                    "CMAEG_ROUTING_UNKNOWN_TARGET_TEXT",
                    "private-notice-sentinel {name}".into(),
                ),
            ],
            None,
        ),
        (
            vec![("CMAEG_ROUTED_CHANNEL_MAX_TURNS", "0".into())],
            Some("CMAEG_ROUTED_CHANNEL_MAX_TURNS"),
        ),
        (
            vec![("CMAEG_ROUTED_OWNER_MAX_TURNS", "1025".into())],
            Some("CMAEG_ROUTED_OWNER_MAX_TURNS"),
        ),
        (
            vec![("CMAEG_ROUTED_CHANNEL_MAX_TURNS", "17".into())],
            Some("CMAEG_ROUTED_OWNER_MAX_TURNS"),
        ),
        (
            vec![(
                "CMAEG_ROUTING_UNKNOWN_TARGET_TEXT",
                "private-notice-sentinel {other}".into(),
            )],
            Some("CMAEG_ROUTING_UNKNOWN_TARGET_TEXT"),
        ),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        for key in [
            "CMAEG_ROUTED_CHANNEL_MAX_TURNS",
            "CMAEG_ROUTED_OWNER_MAX_TURNS",
            "CMAEG_ROUTING_UNKNOWN_TARGET_TEXT",
        ] {
            cases.push((vec![(key, OsString::from_vec(vec![0xff]))], Some(key)));
        }
    }
    for (environment, failure_key) in cases {
        let mut command = files.command("127.0.0.1:0", &cma, &nyxid, &provider);
        command
            .env("CMAEG_MULTI_CONSUMER_ENABLED", "false")
            .arg("--audit-routing");
        for (key, value) in environment {
            command.env(key, value);
        }
        let output = tokio::time::timeout(Duration::from_secs(5), command.output())
            .await
            .unwrap()
            .unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        for text in [&stdout, &stderr] {
            assert_no_secrets(text);
            assert!(!text.contains("private-notice-sentinel"));
        }
        if let Some(key) = failure_key {
            assert!(!output.status.success());
            assert!(stderr.contains(key), "{stderr}");
            assert!(!stderr.contains("CMAEG listening"));
        } else {
            assert!(output.status.success(), "{stderr}");
            assert!(stdout.contains("\"scanned\":0"), "{stdout}");
        }
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
