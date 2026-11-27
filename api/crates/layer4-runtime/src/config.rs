use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const DEFAULT_BIND: &str = "0.0.0.0:9090";
const DEFAULT_HOME: &str = "/home/cma";
const DEFAULT_STATE_ROOT: &str = "/var/lib/cma-runtime";
const DEFAULT_PATH: &str = "/home/cma/.local/bin:/usr/local/bin:/usr/bin:/bin";
const DESKTOP_STARTUP_FLAG: &str = "CHRONO_RUNTIME_DESKTOP_EXTENDED_STARTUP_ENABLED";

#[derive(Clone, Debug)]
pub(crate) struct ProxyConfig {
    pub bind: SocketAddr,
    pub upstream: String,
    pub role: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfig {
    pub bind: SocketAddr,
    pub sandbox_id: String,
    pub generation: u64,
    pub home_identity: Option<String>,
    pub home: PathBuf,
    pub state_root: PathBuf,
    pub checkpoint_root: PathBuf,
    pub install_env: BTreeMap<String, String>,
    pub process_env: BTreeMap<String, String>,
    pub model_proxy: Option<ProxyConfig>,
    pub model_credit_required: bool,
    pub platform_proxy: Option<ProxyConfig>,
    pub tool_proxy: Option<crate::tool_proxy::ToolProxyConfig>,
    pub github_proxy: Option<crate::github_proxy::GithubProxyConfig>,
    pub desktop_supervision_enabled: bool,
    pub nyxid_url: Option<String>,
    pub allow_local_git: bool,
    /// Set only on a Sandbox whose immutable profile requested a desktop.
    pub machine_included: bool,
    pub extended_desktop_startup: bool,
    pub capacity_recovery_enabled: bool,
    pub agent_process_restart_enabled: bool,
    pub legacy_journal_recovery_enabled: bool,
    pub preserve_file_mode: bool,
}

impl RuntimeConfig {
    pub(crate) fn from_environment() -> Result<Self, String> {
        Self::from_values(std::env::vars().collect())
    }

    pub(crate) fn configure_stale_turn_seal(&mut self, control_authenticated: bool) {
        // Agent Manager qualifies the running image and checks the current
        // writer before invoking the seal on the native transport.
        if control_authenticated && !self.machine_included {
            self.process_env
                .entry(crate::bridge::STALE_TURN_SEAL_FLAG.into())
                .or_insert_with(|| "1".into());
        }
    }

    pub(crate) fn from_values(mut process_env: BTreeMap<String, String>) -> Result<Self, String> {
        let home = process_env
            .get("CHRONO_RUNTIME_HOME")
            .or_else(|| process_env.get("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_HOME));
        validate_home(&home)?;
        let sandbox_id = process_env
            .get("CHRONO_SANDBOX_ID")
            .filter(|value| !value.trim().is_empty() && value.len() <= 256)
            .cloned()
            .ok_or_else(|| "CHRONO_SANDBOX_ID must identify this runtime".to_string())?;
        let state_root = process_env
            .get("CHRONO_RUNTIME_STATE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_ROOT).join(&sandbox_id));
        validate_state_root(&state_root, &home)?;
        let generation = process_env
            .get("CHRONO_RUNTIME_GENERATION")
            .map(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|number| *number != 0 && number.to_string() == *value)
                    .ok_or_else(|| {
                        "CHRONO_RUNTIME_GENERATION must be a nonzero canonical integer".to_string()
                    })
            })
            .transpose()?
            .unwrap_or(1);
        let home_identity = process_env
            .get("CHRONO_RUNTIME_HOME_IDENTITY")
            .filter(|value| !value.is_empty())
            .cloned();
        if home_identity
            .as_ref()
            .is_some_and(|value| !layer3_adapter::valid_home_identity(value))
        {
            return Err("CHRONO_RUNTIME_HOME_IDENTITY must be a canonical home fingerprint".into());
        }
        let machine_included = env_flag(&process_env, "CMA_MACHINE_INCLUDED")?;
        let extended_desktop_startup = env_flag(&process_env, DESKTOP_STARTUP_FLAG)?;
        let capacity_recovery_enabled =
            env_flag(&process_env, "CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED")?;
        let agent_process_restart_enabled =
            env_flag(&process_env, "CHRONO_RUNTIME_AGENT_PROCESS_RESTART_ENABLED")?;
        let legacy_journal_recovery_enabled = env_flag(
            &process_env,
            "CHRONO_RUNTIME_LEGACY_JOURNAL_RECOVERY_ENABLED",
        )?;

        let preserve_file_mode = crate::agent_filesystem::preserve_mode_enabled(&process_env)?;

        let bind = env_socket(&process_env, "CHRONO_RUNTIME_BIND", DEFAULT_BIND)?;
        process_env.insert("HOME".into(), home.to_string_lossy().into_owned());
        process_env
            .entry("PATH".into())
            .or_insert_with(|| DEFAULT_PATH.into());
        if machine_included {
            // Codex and Chrome must share the display started by the
            // supervisor-owned desktop session. Do not inherit a caller's
            // arbitrary display socket.
            process_env.insert("DISPLAY".into(), ":99".into());
        }

        let mut install_env = process_env
            .iter()
            .filter(|(name, _)| !reserved(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        install_env.insert("HOME".into(), home.to_string_lossy().into_owned());
        install_env
            .entry("PATH".into())
            .or_insert_with(|| DEFAULT_PATH.into());

        let model_proxy = proxy_from_env(
            &process_env,
            "CHRONO_UPSTREAM_BIND",
            "127.0.0.1:9091",
            "CHRONO_UPSTREAM_BASE_URL",
            "model",
        )?;
        let model_credit_required = match process_env
            .get("CHRONO_AGENT_MODEL_CREDIT_REQUIRED")
            .map(String::as_str)
        {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => {
                return Err("CHRONO_AGENT_MODEL_CREDIT_REQUIRED must be true or false".into())
            }
        };
        let platform_proxy = proxy_from_env(
            &process_env,
            "CHRONO_PLATFORM_PROXY_BIND",
            "127.0.0.2:9092",
            "CHRONO_PLATFORM_BASE_URL",
            "platform",
        )?;
        let nyxid_url = process_env
            .get("NYXID_URL")
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());
        let tool_proxy = crate::tool_proxy::ToolProxyConfig::from_env(&process_env)?;
        let github_proxy = crate::github_proxy::configuration(
            &state_root,
            &mut process_env,
            platform_proxy.as_ref(),
        )?;
        let checkpoint_root = checkpoint_root(&process_env, &home, home_identity.is_some())?;
        let desktop_supervision_enabled =
            crate::desktop_process::supervision_enabled(&process_env)?;

        Ok(Self {
            bind,
            sandbox_id,
            generation,
            home_identity,
            checkpoint_root,
            home,
            state_root,
            install_env,
            process_env,
            model_proxy,
            model_credit_required,
            platform_proxy,
            tool_proxy,
            github_proxy,
            desktop_supervision_enabled,
            nyxid_url,
            allow_local_git: false,
            machine_included,
            extended_desktop_startup,
            capacity_recovery_enabled,
            agent_process_restart_enabled,
            legacy_journal_recovery_enabled,
            preserve_file_mode,
        })
    }

    pub(crate) fn agent_home(&self, agent_id: &str) -> PathBuf {
        self.home
            .join(".cma")
            .join("agents")
            .join(agent_id)
            .join("home")
    }

    #[cfg(test)]
    pub(crate) fn for_test(home: PathBuf) -> Self {
        let home_text = home.to_string_lossy().into_owned();
        let checkpoint_root = home.with_extension("checkpoints");
        let state_root = home.with_extension("runtime-state");
        let env = BTreeMap::from([
            ("HOME".into(), home_text),
            ("PATH".into(), DEFAULT_PATH.into()),
            ("CMA_TEST_ENV".into(), "visible".into()),
        ]);
        Self {
            bind: "127.0.0.1:0".parse().unwrap(),
            sandbox_id: "sandbox-test".into(),
            generation: 1,
            home_identity: None,
            checkpoint_root,
            home,
            state_root,
            install_env: env.clone(),
            process_env: env,
            model_proxy: None,
            model_credit_required: false,
            platform_proxy: None,
            tool_proxy: None,
            github_proxy: None,
            desktop_supervision_enabled: false,
            nyxid_url: None,
            allow_local_git: true,
            machine_included: false,
            extended_desktop_startup: false,
            capacity_recovery_enabled: false,
            agent_process_restart_enabled: false,
            legacy_journal_recovery_enabled: false,
            preserve_file_mode: false,
        }
    }
}

fn checkpoint_root(
    environment: &BTreeMap<String, String>,
    home: &Path,
    persistent_home: bool,
) -> Result<PathBuf, String> {
    let Some(temporary) = environment.get("TMPDIR") else {
        // Legacy runtimes without an assigned TMPDIR retain their original
        // platform-specific temporary directory and installation recovery path.
        return Ok(std::env::temp_dir().join("cma-checkpoints"));
    };
    let temporary_path = Path::new(temporary);
    if persistent_home
        && (!temporary_path.is_absolute()
            || temporary_path == Path::new("/")
            || temporary_path.starts_with(home)
            || temporary.split('/').any(|part| matches!(part, "." | "..")))
    {
        return Err(
            "TMPDIR must be a normalized absolute directory outside the sandbox home".into(),
        );
    }
    // A waiting pool process has not installed assignment variables into its
    // ambient environment. Use the assigned transfer mount for checkpoints and
    // installation journals, preserving their existing on-disk layout.
    Ok(temporary_path.join("cma-checkpoints"))
}

fn validate_state_root(state_root: &Path, home: &Path) -> Result<(), String> {
    if !state_root.is_absolute()
        || state_root == Path::new("/")
        || state_root.starts_with(home)
        || state_root.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(
            "CHRONO_RUNTIME_STATE_ROOT must be a normalized directory outside the sandbox home"
                .into(),
        );
    }
    Ok(())
}

fn validate_home(home: &Path) -> Result<(), String> {
    if !home.is_absolute()
        || home == Path::new("/")
        || home.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("CHRONO_RUNTIME_HOME must be a normalized absolute directory".into());
    }
    Ok(())
}

fn env_flag(env: &BTreeMap<String, String>, name: &str) -> Result<bool, String> {
    match env.get(name).map(String::as_str) {
        None | Some("0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(_) => Err(format!("{name} must be 0, 1, false or true")),
    }
}

fn env_socket(
    env: &BTreeMap<String, String>,
    name: &str,
    fallback: &str,
) -> Result<SocketAddr, String> {
    env.get(name)
        .map(String::as_str)
        .unwrap_or(fallback)
        .parse()
        .map_err(|error| format!("{name} must be a socket address: {error}"))
}

fn proxy_from_env(
    env: &BTreeMap<String, String>,
    bind_name: &str,
    fallback_bind: &str,
    upstream_name: &str,
    role: &'static str,
) -> Result<Option<ProxyConfig>, String> {
    let upstream = env
        .get(upstream_name)
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if upstream.is_empty() {
        return Ok(None);
    }
    let bind = env_socket(env, bind_name, fallback_bind)?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err(format!(
            "{bind_name} must be a nonzero loopback socket address"
        ));
    }
    Ok(Some(ProxyConfig {
        bind,
        upstream,
        role,
    }))
}

fn reserved(name: &str) -> bool {
    name.starts_with("CHRONO_") || name.starts_with("NYXID_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_environment() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("CHRONO_SANDBOX_ID".into(), "sandbox-test".into()),
            ("CHRONO_RUNTIME_HOME".into(), "/home/cma".into()),
            ("CHRONO_RUNTIME_HOME_IDENTITY".into(), "a".repeat(43)),
        ])
    }

    #[test]
    fn authenticated_standard_runtime_defaults_to_stale_turn_seals() {
        let flag = crate::bridge::STALE_TURN_SEAL_FLAG;
        for (authenticated, machine, override_value, expected) in [
            (true, false, None, Some("1")),
            (true, false, Some("0"), Some("0")),
            (true, true, None, None),
            (false, false, None, None),
        ] {
            let mut environment = runtime_environment();
            if machine {
                environment.insert("CMA_MACHINE_INCLUDED".into(), "1".into());
            }
            if let Some(value) = override_value {
                environment.insert(flag.into(), value.into());
            }
            let mut config = RuntimeConfig::from_values(environment).unwrap();
            config.configure_stale_turn_seal(authenticated);
            assert_eq!(config.process_env.get(flag).map(String::as_str), expected);
            assert!(!config.install_env.contains_key(flag));
        }
    }

    #[test]
    fn assigned_tmpdir_controls_checkpoint_and_installation_storage() {
        let mut environment = runtime_environment();
        environment.insert("TMPDIR".into(), "/var/lib/cma-transfer".into());
        let config = RuntimeConfig::from_values(environment).unwrap();
        assert_eq!(
            config.checkpoint_root,
            Path::new("/var/lib/cma-transfer/cma-checkpoints")
        );
        assert_eq!(config.process_env["TMPDIR"], "/var/lib/cma-transfer");
        assert_eq!(config.install_env["TMPDIR"], "/var/lib/cma-transfer");
    }

    #[test]
    fn machine_marker_is_explicit_and_fail_closed() {
        let mut environment = runtime_environment();
        assert!(
            !RuntimeConfig::from_values(environment.clone())
                .unwrap()
                .machine_included
        );
        environment.insert("CMA_MACHINE_INCLUDED".into(), "1".into());
        let machine = RuntimeConfig::from_values(environment.clone()).unwrap();
        assert!(machine.machine_included);
        assert_eq!(
            machine.process_env.get("DISPLAY").map(String::as_str),
            Some(":99")
        );
        environment.insert("CMA_MACHINE_INCLUDED".into(), "yes".into());
        assert!(RuntimeConfig::from_values(environment).is_err());
    }

    #[test]
    fn absent_tmpdir_preserves_legacy_temporary_directory() {
        let mut environment = runtime_environment();
        for persistent in [true, false] {
            if !persistent {
                environment.remove("CHRONO_RUNTIME_HOME_IDENTITY");
            }
            let config = RuntimeConfig::from_values(environment.clone()).unwrap();
            assert_eq!(
                config.checkpoint_root,
                std::env::temp_dir().join("cma-checkpoints")
            );
        }
    }

    #[test]
    fn nonpersistent_runtime_preserves_custom_home_local_tmpdir() {
        let mut environment = runtime_environment();
        environment.remove("CHRONO_RUNTIME_HOME_IDENTITY");
        environment.insert("TMPDIR".into(), "/home/cma/tmp".into());
        let config = RuntimeConfig::from_values(environment).unwrap();
        assert_eq!(
            config.checkpoint_root,
            Path::new("/home/cma/tmp/cma-checkpoints")
        );
        assert_eq!(config.install_env["TMPDIR"], "/home/cma/tmp");
    }

    #[test]
    fn invalid_assigned_tmpdir_is_rejected_without_ambient_fallback() {
        for temporary in [
            "",
            "relative",
            "/",
            "/tmp/../transfer",
            "/tmp/./transfer",
            "/home/cma",
            "/home/cma/tmp",
        ] {
            let mut environment = runtime_environment();
            environment.insert("TMPDIR".into(), temporary.into());
            assert!(RuntimeConfig::from_values(environment)
                .unwrap_err()
                .contains("TMPDIR"));
        }
    }

    #[test]
    fn assigned_storage_keeps_foreign_installation_journals_rejected() {
        let temporary = std::env::temp_dir().join(format!(
            "cma-assigned-install-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut environment = runtime_environment();
        environment.insert("TMPDIR".into(), temporary.to_string_lossy().into_owned());
        let config = RuntimeConfig::from_values(environment).unwrap();
        let foreign = RuntimeConfig {
            sandbox_id: "another-sandbox".into(),
            ..config.clone()
        };
        let request = layer3_adapter::InstallRequest::new("foreign-install", vec![]);
        let record = crate::install_state::InstallRecord::started(&foreign, &request);
        assert!(crate::install_state::write_initial(&foreign, &record).unwrap());
        let path = temporary.join("cma-checkpoints/runtime-state/install-v2.json");
        let original = std::fs::read(&path).unwrap();
        assert_eq!(
            crate::install_state::recover(&config).unwrap_err().code,
            "install_state_invalid"
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn reserved_variables_are_not_install_environment() {
        assert!(reserved("CHRONO_SANDBOX_REACHES"));
        assert!(reserved("NYXID_URL"));
        assert!(!reserved("RUST_LOG"));
    }

    #[test]
    fn credential_proxies_require_nonzero_loopback_bindings() {
        for bind in ["0.0.0.0:9091", "192.0.2.1:9091", "127.0.0.1:0"] {
            let env = BTreeMap::from([
                ("UPSTREAM".into(), "https://example.test".into()),
                ("BIND".into(), bind.into()),
            ]);
            assert!(proxy_from_env(&env, "BIND", "127.0.0.1:9091", "UPSTREAM", "test").is_err());
        }

        let env = BTreeMap::from([
            ("UPSTREAM".into(), "https://example.test".into()),
            ("BIND".into(), "127.0.0.2:9091".into()),
        ]);
        assert_eq!(
            proxy_from_env(&env, "BIND", "127.0.0.1:9091", "UPSTREAM", "test")
                .unwrap()
                .unwrap()
                .bind,
            "127.0.0.2:9091".parse().unwrap()
        );
    }
}

#[cfg(test)]
#[path = "config_recovery_tests.rs"]
mod recovery_tests;
