use crate::{
    config::RuntimeConfig,
    failure::{Result, RuntimeError},
};
use layer3_adapter::{BrowserObservation, BrowserRequest};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub(crate) struct Browser {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    incoming: Vec<u8>,
    sequence: u64,
    transport_closed: bool,
    _managed: crate::writer_fence::ManagedChild,
}

pub(crate) fn directory(config: &RuntimeConfig, boot: &str, agent: &str) -> PathBuf {
    let identity = format!("{}\0{}\0{}", config.state_root.display(), boot, agent);
    let digest = format!("{:x}", Sha256::digest(identity));
    std::env::temp_dir().join(format!("cma-browser-{}", &digest[..24]))
}

impl Browser {
    pub(crate) async fn start(config: &RuntimeConfig, boot: &str, agent: &str) -> Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let root = directory(config, boot, agent);
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|_| unavailable())?;
        tokio::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| unavailable())?;
        for (name, bytes) in [
            ("mediator.cjs", include_str!("browser/mediator.cjs")),
            ("devtools.cjs", include_str!("browser/devtools.cjs")),
            ("control.cjs", include_str!("browser/control.cjs")),
            ("input.cjs", include_str!("browser/input.cjs")),
        ] {
            tokio::fs::write(root.join(name), bytes)
                .await
                .map_err(|_| unavailable())?;
        }
        let mut command = Command::new("node");
        command
            .arg(root.join("mediator.cjs"))
            .arg(&root)
            .env_clear()
            .env("HOME", &root)
            .env(
                "PATH",
                config
                    .process_env
                    .get("PATH")
                    .map(String::as_str)
                    .unwrap_or("/usr/local/bin:/usr/bin:/bin"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(test)]
        if let Some(binary) = config.process_env.get("CMA_BROWSER_BINARY") {
            command.env("CMA_BROWSER_BINARY", binary);
        }
        command.process_group(0);
        let permit = crate::writer_fence::ManagedSpawnPermit::begin();
        let mut child = command.spawn().map_err(|_| unavailable())?;
        let managed = permit.register(child.id());
        let input = child.stdin.take().ok_or_else(unavailable)?;
        let output = BufReader::new(child.stdout.take().ok_or_else(unavailable)?);
        Ok(Self {
            child,
            input,
            output,
            incoming: Vec::new(),
            sequence: 0,
            transport_closed: false,
            _managed: managed,
        })
    }

    pub(crate) fn transport_closed(&self) -> bool {
        self.transport_closed
    }

    pub(crate) async fn exchange(
        &mut self,
        request: &BrowserRequest,
    ) -> Result<BrowserObservation> {
        let value = self
            .raw(serde_json::to_value(request).map_err(|_| unavailable())?)
            .await?;
        let observation: BrowserObservation =
            serde_json::from_value(value).map_err(|_| unavailable())?;
        if !observation.valid() {
            return Err(unavailable());
        }
        Ok(observation)
    }

    pub(crate) async fn control(
        &mut self,
        request: &layer3_adapter::BrowserControlRequest,
    ) -> Result<layer3_adapter::BrowserControlReceipt> {
        let mut value = serde_json::to_value(request).map_err(|_| unavailable())?;
        value["operation"] = serde_json::json!("control");
        let value = self.raw(value).await?;
        let context_id = value["context_id"]
            .as_str()
            .ok_or_else(unavailable)?
            .to_owned();
        let control: layer3_adapter::BrowserControlStatus =
            serde_json::from_value(value).map_err(|_| unavailable())?;
        if !control.valid() {
            return Err(unavailable());
        }
        Ok(layer3_adapter::BrowserControlReceipt {
            agent_id: request.agent_id.clone(),
            boot_id: request.boot_id.clone(),
            context_id,
            control,
        })
    }

    async fn raw(&mut self, mut value: serde_json::Value) -> Result<serde_json::Value> {
        let operation = async {
            self.sequence += 1;
            value["id"] = serde_json::json!(self.sequence);
            let mut encoded = serde_json::to_vec(&value).map_err(|_| unavailable())?;
            encoded.push(b'\n');
            if self.input.write_all(&encoded).await.is_err() {
                self.transport_closed = true;
                return Err(unavailable());
            }
            let value = loop {
                let read = (&mut self.output)
                    .take(360000_u64.saturating_sub(self.incoming.len() as u64))
                    .read_until(b'\n', &mut self.incoming)
                    .await;
                if read.is_err() || self.incoming.last() != Some(&b'\n') {
                    self.transport_closed = true;
                    return Err(unavailable());
                }
                let encoded = std::mem::take(&mut self.incoming);
                let value: serde_json::Value =
                    serde_json::from_slice(&encoded).map_err(|_| unavailable())?;
                let identifier = value["id"].as_u64().ok_or_else(unavailable)?;
                if identifier < self.sequence {
                    continue;
                }
                if identifier != self.sequence {
                    return Err(unavailable());
                }
                if let Some(error) = value["error"].as_str() {
                    let code = match error {
                        "browser_control_conflict" | "browser_operation_unknown" => {
                            "browser_control_conflict"
                        }
                        "browser_identity" => "browser_identity",
                        "browser_control_invalid" | "browser_input_invalid" => {
                            "browser_control_invalid"
                        }
                        _ => "browser_disconnected",
                    };
                    return Err(RuntimeError::rejected(
                        code,
                        "Browser control request refused",
                    ));
                }
                break value;
            };
            Ok(value["value"].clone())
        };
        tokio::time::timeout(Duration::from_secs(12), operation)
            .await
            .map_err(|_| unavailable())?
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(pid) = self.child.id().and_then(|value| i32::try_from(value).ok()) {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
        }
    }
}

fn unavailable() -> RuntimeError {
    RuntimeError::rejected("browser_disconnected", "Agent browser is unavailable")
}
