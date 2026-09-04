use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const DEFAULT_BIND: &str = "0.0.0.0:9090";
const DEFAULT_HOME: &str = "/home/cma";
const DEFAULT_PATH: &str = "/home/cma/.local/bin:/usr/local/bin:/usr/bin:/bin";

#[derive(Clone, Debug)]
pub(crate) struct ProxyConfig {
    pub bind: SocketAddr,
    pub upstream: String,
    pub role: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeConfig {
    pub bind: SocketAddr,
    pub home: PathBuf,
    pub checkpoint_root: PathBuf,
    pub install_env: BTreeMap<String, String>,
    pub process_env: BTreeMap<String, String>,
    pub model_proxy: Option<ProxyConfig>,
    pub platform_proxy: Option<ProxyConfig>,
    pub nyxid_url: Option<String>,
    pub allow_local_git: bool,
}

impl RuntimeConfig {
    pub(crate) fn from_environment() -> Result<Self, String> {
        let mut process_env = std::env::vars().collect::<BTreeMap<_, _>>();
        let home = process_env
            .get("CHRONO_RUNTIME_HOME")
            .or_else(|| process_env.get("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_HOME));
        validate_home(&home)?;

        let bind = env_socket(&process_env, "CHRONO_RUNTIME_BIND", DEFAULT_BIND)?;
        process_env.insert("HOME".into(), home.to_string_lossy().into_owned());
        process_env
            .entry("PATH".into())
            .or_insert_with(|| DEFAULT_PATH.into());

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

        Ok(Self {
            bind,
            checkpoint_root: std::env::temp_dir().join("cma-checkpoints"),
            home,
            install_env,
            process_env,
            model_proxy,
            platform_proxy,
            nyxid_url,
            allow_local_git: false,
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
        let env = BTreeMap::from([
            ("HOME".into(), home_text),
            ("PATH".into(), DEFAULT_PATH.into()),
            ("CMA_TEST_ENV".into(), "visible".into()),
        ]);
        Self {
            bind: "127.0.0.1:0".parse().unwrap(),
            checkpoint_root: home.with_extension("checkpoints"),
            home,
            install_env: env.clone(),
            process_env: env,
            model_proxy: None,
            platform_proxy: None,
            nyxid_url: None,
            allow_local_git: true,
        }
    }
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
