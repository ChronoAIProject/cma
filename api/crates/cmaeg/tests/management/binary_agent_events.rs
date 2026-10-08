//! Exercise startup configuration through the actual gateway binary.
use super::{
    binary_support::{assert_no_secrets, MountedSecrets},
    *,
};
use std::{ffi::OsString, time::Duration};

type Environment = Vec<(&'static str, OsString)>;

#[tokio::test]
async fn agent_event_startup_validates_budgets_and_warns_when_routing_is_off() {
    let provider = FakeProvider::start(Default::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let files = MountedSecrets::new();
    let mut cases: Vec<(Environment, Option<&str>, bool)> = vec![
        (vec![], None, false),
        (
            vec![("CMAEG_AGENT_EVENTS_ENABLED", "true".into())],
            None,
            true,
        ),
        (
            vec![
                ("CMAEG_AGENT_EVENTS_RATE_PER_MINUTE", "6000".into()),
                ("CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE", "6000".into()),
                ("CMAEG_AGENT_EVENTS_ACCEPT_MS", "7000".into()),
            ],
            None,
            false,
        ),
        (
            vec![(
                "CMAEG_AGENT_EVENTS_ENABLED",
                "private-config-sentinel".into(),
            )],
            Some("CMAEG_AGENT_EVENTS_ENABLED"),
            false,
        ),
        (
            vec![("CMAEG_AGENT_EVENTS_RATE_PER_MINUTE", "9".into())],
            Some("CMAEG_AGENT_EVENTS_RATE_PER_MINUTE"),
            false,
        ),
        (
            vec![("CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE", "601".into())],
            Some("CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE"),
            false,
        ),
        (
            vec![("CMAEG_AGENT_EVENTS_ACCEPT_MS", "7001".into())],
            Some("CMAEG_AGENT_EVENTS_ACCEPT_MS"),
            false,
        ),
    ];
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        for key in [
            "CMAEG_AGENT_EVENTS_ENABLED",
            "CMAEG_AGENT_EVENTS_RATE_PER_MINUTE",
            "CMAEG_AGENT_EVENTS_OWNER_RATE_PER_MINUTE",
            "CMAEG_AGENT_EVENTS_ACCEPT_MS",
        ] {
            cases.push((
                vec![(key, OsString::from_vec(vec![0xff]))],
                Some(key),
                false,
            ));
        }
    }
    for (environment, failure_key, warning) in cases {
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
            assert!(!text.contains("private-config-sentinel"));
        }
        if let Some(key) = failure_key {
            assert!(!output.status.success());
            assert!(stderr.contains(key), "{stderr}");
            assert!(!stderr.contains("CMAEG listening"));
        } else {
            assert!(output.status.success(), "{stderr}");
            assert!(stdout.contains("\"scanned\":0"), "{stdout}");
            assert_eq!(
                stderr.contains("publishing remains unlisted and disabled"),
                warning,
                "{stderr}"
            );
        }
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
