use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::{FileTypeExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use crate::config::ProxyConfig;
use crate::failure::{Result, RuntimeError};
use crate::upstream::CredentialHolder;
use crate::upstream_target::TargetBase;

pub(crate) const ACCESS_PATH_ENV: &str = "CMA_GITHUB_ACCESS_PATH";

const PLACEHOLDER: &str = "local-proxy-no-credential-needed";

#[derive(Clone, Debug)]
pub(crate) struct GithubProxyConfig {
    directory: PathBuf,
    upstream: String,
}

impl GithubProxyConfig {
    pub(crate) fn socket(&self) -> std::path::PathBuf {
        self.directory.join("api.sock")
    }
}

pub(crate) fn configuration(
    state_root: &Path,
    environment: &mut BTreeMap<String, String>,
    platform: Option<&ProxyConfig>,
) -> std::result::Result<Option<GithubProxyConfig>, String> {
    let granted = environment
        .get("CHRONO_SANDBOX_REACHES")
        .is_some_and(|reaches| reaches.split(',').any(|service| service == "api-github"));
    let Some(platform) = platform else {
        return if granted {
            Err("GitHub service requires the platform credential proxy".into())
        } else {
            Ok(None)
        };
    };
    // Prepare transport for every managed platform runtime. NyxID authorizes
    // each request with the current scoped key, so a live scope grant/removal
    // takes effect without changing an existing Agent process's environment.
    let directory = state_root.join("gh");
    let socket = directory.join("api.sock");
    if socket.as_os_str().len() > 100 {
        return Err("GitHub proxy socket path exceeds the portable Unix socket limit".into());
    }
    let target = TargetBase::parse(&platform.upstream)
        .and_then(|base| base.join(&"/api/v1/proxy/s/api-github".parse().expect("static path")))
        .map_err(|_| "GitHub platform upstream is invalid")?;
    environment.extend([
        (
            "GH_CONFIG_DIR".into(),
            directory.to_string_lossy().into_owned(),
        ),
        ("GH_HOST".into(), "github.com".into()),
        ("GH_TOKEN".into(), PLACEHOLDER.into()),
        ("GH_PROMPT_DISABLED".into(), "1".into()),
    ]);
    Ok(Some(GithubProxyConfig {
        directory,
        upstream: target.into(),
    }))
}

pub(crate) fn reserved_environment(name: &str) -> bool {
    matches!(
        name,
        "GH_CONFIG_DIR" | "GH_HOST" | "GH_TOKEN" | "GH_PROMPT_DISABLED" | ACCESS_PATH_ENV
    )
}

pub(crate) struct PreparedGithubProxy {
    listener: tokio::net::UnixListener,
    target: TargetBase,
    lease: std::fs::File,
}

impl PreparedGithubProxy {
    pub(crate) async fn bind(config: &GithubProxyConfig) -> Result<Self> {
        let target = TargetBase::parse(&config.upstream)?;
        prepare_directory(&config.directory).map_err(configuration_error)?;
        let lease = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(config.directory.join("proxy.lock"))
            .map_err(configuration_error)?;
        if !lease.metadata().map_err(configuration_error)?.is_file() {
            return Err(configuration_error(std::io::Error::other(
                "invalid proxy lease",
            )));
        }
        lease.try_lock().map_err(|_| {
            RuntimeError::unavailable(
                "github_proxy_in_use",
                "GitHub proxy is already active or its lease is unavailable",
            )
        })?;
        let socket = config.directory.join("api.sock");
        match std::fs::symlink_metadata(&socket) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(RuntimeError::rejected(
                        "github_proxy_path_conflict",
                        "GitHub proxy path is occupied by a non-socket entry",
                    ));
                }
                match tokio::net::UnixStream::connect(&socket).await {
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        std::fs::remove_file(&socket).map_err(configuration_error)?;
                    }
                    _ => {
                        return Err(RuntimeError::unavailable(
                            "github_proxy_in_use",
                            "GitHub proxy socket is already active or unavailable",
                        ));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(configuration_error(error)),
        }
        let listener = tokio::net::UnixListener::bind(&socket).map_err(configuration_error)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .map_err(configuration_error)?;
        // JSON is valid YAML; serialize the path instead of interpolating YAML syntax.
        let content = serde_json::to_vec(&serde_json::json!({
            "http_unix_socket": socket,
            "git_protocol": "https",
        }))
        .expect("static GitHub configuration serializes");
        write_configuration(&config.directory, &content).map_err(configuration_error)?;
        Ok(Self {
            listener,
            target,
            lease,
        })
    }

    pub(crate) fn start(self, credential: CredentialHolder) {
        let app = crate::upstream::router(self.target, credential, Some("api.github.com"));
        tokio::spawn(async move {
            let _lease = self.lease;
            tracing::info!("GitHub credential proxy listening on local socket");
            if let Err(error) = axum::serve(self.listener, app).await {
                tracing::error!(%error, "GitHub credential proxy stopped");
            }
        });
    }
}

fn write_configuration(directory: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = directory.join(format!(
        "config-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(content)?;
        file.sync_all()?;
        crate::fs_durability::rename(&temporary, &directory.join("config.yml"))
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

fn prepare_directory(path: &Path) -> std::io::Result<()> {
    crate::fs_durability::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "GitHub configuration requires a real directory",
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn configuration_error(_error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "github_proxy_unavailable",
        "GitHub local proxy configuration could not be prepared",
    )
}

#[cfg(test)]
#[path = "github_proxy_tests.rs"]
mod tests;
