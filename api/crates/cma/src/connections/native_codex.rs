//! The pinned native Codex process owns device-code OAuth and token renewal.
//! No local operator login, shell invocation or sandbox assignment is involved.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
use zeroize::Zeroizing;

use crate::auth::persistence::SecretValue;

use super::{subscription_tokens::SubscriptionTokens, SubscriptionError};

#[path = "native_codex_ingress_cache.rs"]
mod ingress_cache;

#[cfg(all(
    any(
        all(target_os = "linux", target_env = "gnu"),
        all(
            target_os = "linux",
            target_env = "musl",
            target_vendor = "unknown",
            target_pointer_width = "64",
            target_endian = "little",
        ),
        target_os = "macos",
    ),
    any(target_arch = "x86_64", target_arch = "aarch64"),
))]
#[path = "native_codex_ingress_home.rs"]
mod ingress_home;

#[path = "native_codex_ingress_protocol.rs"]
mod ingress_protocol;

#[path = "native_codex_ingress_request.rs"]
mod ingress_request;

#[path = "native_codex_ingress_response.rs"]
mod ingress_response;

const CALL_TIMEOUT: Duration = Duration::from_secs(8);
pub(super) const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_FRAME_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DeviceLogin {
    pub login_id: String,
    pub verification_url: String,
    pub user_code: String,
}

pub(crate) struct NativeCodex {
    child: Option<Child>,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    home: Option<PathBuf>,
    next_id: u64,
    completed: Option<(String, bool)>,
}

impl NativeCodex {
    pub(crate) async fn open(
        binary: &Path,
        cache: Option<&SecretValue>,
    ) -> Result<Self, SubscriptionError> {
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
        let home = std::env::temp_dir().join(format!("cma-codex-oauth-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&home)
            .map_err(|_| SubscriptionError::Unavailable)?;
        let spawn = || -> Result<Child, SubscriptionError> {
            if let Some(cache) = cache {
                use std::io::Write as _;
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(home.join("auth.json"))
                    .map_err(|_| SubscriptionError::Unavailable)?;
                file.write_all(cache.expose().as_bytes())
                    .map_err(|_| SubscriptionError::Unavailable)?;
            }
            Command::new(binary)
                .arg("app-server")
                .arg("-c")
                .arg("cli_auth_credentials_store=\"file\"")
                .env_clear()
                .env("PATH", "/usr/local/bin:/usr/bin:/bin")
                .env("CODEX_HOME", &home)
                .current_dir(&home)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| SubscriptionError::Unavailable)
        };
        let mut child = match spawn() {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&home);
                return Err(error);
            }
        };
        let input = child.stdin.take().ok_or(SubscriptionError::Unavailable)?;
        let output = BufReader::new(child.stdout.take().ok_or(SubscriptionError::Unavailable)?);
        let mut native = Self {
            child: Some(child),
            input,
            output,
            home: Some(home),
            next_id: 0,
            completed: None,
        };
        native.call("initialize", json!({
            "clientInfo": {"name": "cma_subscription_connection", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": false}
        })).await?;
        native.send(json!({"method": "initialized"})).await?;
        Ok(native)
    }

    pub(crate) async fn login(&mut self) -> Result<DeviceLogin, SubscriptionError> {
        let response = self
            .call("account/login/start", json!({"type":"chatgptDeviceCode"}))
            .await?;
        let field = |key| {
            response
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(SubscriptionError::InvalidAccount)
        };
        let login = DeviceLogin {
            login_id: field("loginId")?,
            verification_url: field("verificationUrl")?,
            user_code: field("userCode")?,
        };
        let url = reqwest::Url::parse(&login.verification_url)
            .map_err(|_| SubscriptionError::InvalidAccount)?;
        if url.scheme() != "https"
            || url.host_str() != Some("auth.openai.com")
            || url.path() != "/codex/device"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || login.user_code.is_empty()
            || login.user_code.len() > 64
            || !login
                .user_code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            || uuid::Uuid::parse_str(&login.login_id).is_err()
        {
            return Err(SubscriptionError::InvalidAccount);
        }
        Ok(login)
    }

    pub(crate) async fn wait_login(
        &mut self,
        login: &DeviceLogin,
    ) -> Result<SecretValue, SubscriptionError> {
        tokio::time::timeout(LOGIN_TIMEOUT, async {
            loop {
                if let Some((id, success)) = self.completed.take() {
                    if id != login.login_id {
                        return Err(SubscriptionError::InvalidAccount);
                    }
                    if !success {
                        return Err(SubscriptionError::SignInRequired);
                    }
                    return self.cache().await;
                }
                let frame = self.frame().await?;
                self.notification(&frame);
            }
        })
        .await
        .map_err(|_| SubscriptionError::Expired)?
    }

    pub(crate) async fn refresh(&mut self) -> Result<SecretValue, SubscriptionError> {
        let result = self
            .call("account/read", json!({"refreshToken":true}))
            .await?;
        if result.pointer("/account/type").and_then(Value::as_str) != Some("chatgpt") {
            return Err(SubscriptionError::SignInRequired);
        }
        self.cache().await
    }

    pub(crate) async fn cancel(&mut self, id: &str) -> Result<(), SubscriptionError> {
        self.call("account/login/cancel", json!({"loginId":id}))
            .await?;
        Ok(())
    }

    async fn cache(&self) -> Result<SecretValue, SubscriptionError> {
        let path = self
            .home
            .as_ref()
            .ok_or(SubscriptionError::Unavailable)?
            .join("auth.json");
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|_| SubscriptionError::InvalidAccount)?;
        if !metadata.is_file() || metadata.len() > 128 * 1024 {
            return Err(SubscriptionError::InvalidAccount);
        }
        let cache = SecretValue::new(
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|_| SubscriptionError::Unavailable)?,
        );
        SubscriptionTokens::from_native_cache(&cache)?;
        Ok(cache)
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, SubscriptionError> {
        self.next_id += 1;
        let id = self.next_id;
        tokio::time::timeout(CALL_TIMEOUT, async {
            self.send(json!({"id":id, "method":method, "params":params}))
                .await?;
            loop {
                let frame = self.frame().await?;
                if frame.get("id").and_then(Value::as_u64) == Some(id) {
                    // Provider messages may contain credentials. They are deliberately
                    // never included in errors, diagnostics or browser responses.
                    return frame
                        .get("result")
                        .cloned()
                        .ok_or(SubscriptionError::Unavailable);
                }
                self.notification(&frame);
            }
        })
        .await
        .map_err(|_| SubscriptionError::Unavailable)?
    }

    async fn send(&mut self, value: Value) -> Result<(), SubscriptionError> {
        let mut bytes =
            Zeroizing::new(serde_json::to_vec(&value).map_err(|_| SubscriptionError::Unavailable)?);
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|_| SubscriptionError::Unavailable)?;
        self.input
            .flush()
            .await
            .map_err(|_| SubscriptionError::Unavailable)
    }

    async fn frame(&mut self) -> Result<Value, SubscriptionError> {
        let mut bytes = Zeroizing::new(Vec::new());
        let size = (&mut self.output)
            .take(MAX_FRAME_BYTES + 1)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| SubscriptionError::Unavailable)?;
        if size == 0 || size as u64 > MAX_FRAME_BYTES || bytes.last() != Some(&b'\n') {
            return Err(SubscriptionError::Unavailable);
        }
        serde_json::from_slice(&bytes).map_err(|_| SubscriptionError::Unavailable)
    }

    fn notification(&mut self, frame: &Value) {
        if frame.get("method").and_then(Value::as_str) == Some("account/login/completed") {
            if let (Some(id), Some(success)) = (
                frame.pointer("/params/loginId").and_then(Value::as_str),
                frame.pointer("/params/success").and_then(Value::as_bool),
            ) {
                self.completed = Some((id.to_owned(), success));
            }
        }
    }
}

impl Drop for NativeCodex {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();
        let home = self.home.take();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = child.wait().await;
                if let Some(home) = home {
                    let _ = tokio::fs::remove_dir_all(home).await;
                }
            });
        }
    }
}
