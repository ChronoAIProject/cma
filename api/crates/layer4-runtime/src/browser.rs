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
            _managed: managed,
        })
    }

    pub(crate) async fn exchange(
        &mut self,
        request: &BrowserRequest,
    ) -> Result<BrowserObservation> {
        let operation = async {
            self.sequence += 1;
            let mut value = serde_json::to_value(request).map_err(|_| unavailable())?;
            value["id"] = serde_json::json!(self.sequence);
            let mut encoded = serde_json::to_vec(&value).map_err(|_| unavailable())?;
            encoded.push(b'\n');
            self.input
                .write_all(&encoded)
                .await
                .map_err(|_| unavailable())?;
            let value = loop {
                (&mut self.output)
                    .take(360000_u64.saturating_sub(self.incoming.len() as u64))
                    .read_until(b'\n', &mut self.incoming)
                    .await
                    .map_err(|_| unavailable())?;
                if self.incoming.last() != Some(&b'\n') {
                    return Err(unavailable());
                }
                let encoded = std::mem::take(&mut self.incoming);
                let value: serde_json::Value =
                    serde_json::from_slice(&encoded).map_err(|_| unavailable())?;
                let identifier = value["id"].as_u64().ok_or_else(unavailable)?;
                if identifier < self.sequence {
                    continue;
                }
                if identifier != self.sequence || value.get("error").is_some() {
                    return Err(unavailable());
                }
                break value;
            };
            let observation: BrowserObservation =
                serde_json::from_value(value["value"].clone()).map_err(|_| unavailable())?;
            if !observation.valid() {
                return Err(unavailable());
            }
            Ok(observation)
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
