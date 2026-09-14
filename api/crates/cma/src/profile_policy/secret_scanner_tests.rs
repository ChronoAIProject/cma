use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use super::*;

struct Executable(PathBuf);

impl Executable {
    fn new(body: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("cma-scanner-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("scanner");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = version ]; then printf '8.30.1\\n'; exit 0; fi\n{body}\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn scanner(&self) -> GitleaksScanner {
        GitleaksScanner::new(self.0.clone())
    }
}

impl Drop for Executable {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn variable(name: &str, value: &str) -> EnvironmentVariable {
    EnvironmentVariable {
        name: name.into(),
        value: value.into(),
    }
}

#[tokio::test]
async fn scans_stdin_with_only_embedded_default_policy_and_no_inherited_environment() {
    let executable = Executable::new("test -z \"$HOME\" || exit 7\ntest \"$GITLEAKS_CONFIG_TOML\" = '[extend]\nuseDefault = true\n' || exit 8\n/bin/cat >/dev/null\nprintf '[]'");
    executable
        .scanner()
        .scan(&[variable("MODE", "development")])
        .await
        .unwrap();
}

#[tokio::test]
async fn names_multiline_variables_without_returning_values_or_report_fields() {
    let executable = Executable::new("/bin/cat >/dev/null\nprintf '[{\"StartLine\":3,\"EndLine\":4,\"Match\":\"never returned\"}]'\nexit 42");
    let result = executable
        .scanner()
        .scan(&[
            variable("MODE", "development"),
            variable("MULTILINE", "first\nsecond\nthird"),
        ])
        .await;
    assert_eq!(
        result,
        Err(PublicValueScanError::Secret {
            variable: "MULTILINE".into()
        })
    );
    assert!(!format!("{result:?}").contains("second"));
    assert!(!format!("{result:?}").contains("never returned"));
}

#[tokio::test]
async fn missing_wrong_version_and_invalid_reports_fail_closed() {
    let missing = GitleaksScanner::new(PathBuf::from("/nonexistent/cma-secret-scanner"));
    assert_eq!(
        missing.scan(&[variable("MODE", "development")]).await,
        Err(PublicValueScanError::Unavailable)
    );
    for body in [
        "/bin/cat >/dev/null; printf '{}'; exit 0",
        "/bin/cat >/dev/null; printf '[]'; exit 42",
        "/bin/cat >/dev/null; printf '[]'; exit 7",
        "/bin/cat >/dev/null; printf '[{\"StartLine\":0,\"EndLine\":1}]'; exit 42",
        "/bin/cat >/dev/null; printf '[{\"StartLine\":1,\"EndLine\":2}]'; exit 42",
    ] {
        let executable = Executable::new(body);
        assert_eq!(
            executable
                .scanner()
                .scan(&[variable("MODE", "development")])
                .await,
            Err(PublicValueScanError::Unavailable)
        );
    }
    let executable = Executable::new("exit 0");
    fs::write(&executable.0, "#!/bin/sh\nprintf '8.30.0\\n'\n").unwrap();
    assert_eq!(
        executable
            .scanner()
            .scan(&[variable("MODE", "development")])
            .await,
        Err(PublicValueScanError::Unavailable)
    );
}

#[tokio::test]
async fn byte_concurrency_timeout_and_output_limits_fail_closed() {
    let executable = Executable::new("/bin/cat >/dev/null\nprintf '[]'");
    let scanner = executable.scanner();
    let slots = scanner.slots.acquire_many(2).await.unwrap();
    assert_eq!(
        scanner.scan(&[variable("MODE", "development")]).await,
        Err(PublicValueScanError::Unavailable)
    );
    drop(slots);
    assert_eq!(
        scanner
            .scan(&[variable("MODE", &"a".repeat(MAX_INPUT_BYTES))])
            .await,
        Err(PublicValueScanError::Unavailable)
    );
    let slow = Executable::new("/bin/cat >/dev/null\nexec /bin/sleep 30");
    let mut scanner = slow.scanner();
    scanner.timeout = Duration::from_millis(50);
    assert_eq!(
        scanner.scan(&[variable("MODE", "development")]).await,
        Err(PublicValueScanError::Unavailable)
    );
    let oversized = Executable::new("/bin/cat >/dev/null\n/usr/bin/head -c 70000 /dev/zero");
    assert_eq!(
        oversized
            .scanner()
            .scan(&[variable("MODE", "development")])
            .await,
        Err(PublicValueScanError::Unavailable)
    );
}

#[tokio::test]
#[ignore = "requires checksum-verified gitleaks 8.30.1 at CMA_TEST_GITLEAKS"]
async fn pinned_engine_detects_complete_rules_beyond_prefix_markers() {
    let executable = PathBuf::from(
        std::env::var_os("CMA_TEST_GITLEAKS").expect("CMA_TEST_GITLEAKS is required"),
    );
    let scanner = GitleaksScanner::new(executable);
    scanner
        .scan(&[
            variable("PROJECT_MODE", "development"),
            variable("PUBLIC_ORIGIN", "https://example.test"),
        ])
        .await
        .unwrap();
    let alphabet = "aB3dE6gH9jK2mN5pQ8sT1vW4yZ7cF0iL";
    let npm = format!(
        "npm_{}",
        alphabet.chars().cycle().take(36).collect::<String>()
    );
    let generic = alphabet.chars().cycle().take(40).collect::<String>();
    for value in [npm, generic] {
        let result = scanner.scan(&[variable("SERVICE_API_KEY", &value)]).await;
        assert_eq!(
            result,
            Err(PublicValueScanError::Secret {
                variable: "SERVICE_API_KEY".into()
            })
        );
        assert!(!format!("{result:?}").contains(&value));
    }
    let bypass = ["# gitleaks:", "allow"].concat();
    let value = format!(
        "{} {bypass}",
        alphabet.chars().cycle().take(40).collect::<String>()
    );
    assert_eq!(
        scanner.scan(&[variable("SERVICE_API_KEY", &value)]).await,
        Err(PublicValueScanError::Secret {
            variable: "SERVICE_API_KEY".into()
        })
    );
}
