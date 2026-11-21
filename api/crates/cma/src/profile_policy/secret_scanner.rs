use std::{path::PathBuf, process::Stdio, time::Duration};

use async_trait::async_trait;
use layer2_core::profiles::{EnvironmentVariable, PublicValueScanError, PublicValueScanner};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{OnceCell, Semaphore},
};
use zeroize::Zeroizing;

const VERSION: &str = "8.30.1";
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_REPORT_BYTES: usize = 64 * 1024;
const SCAN_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_RULES: &str = "[extend]\nuseDefault = true\n";

#[derive(Debug)]
pub(super) struct GitleaksScanner {
    executable: PathBuf,
    verified: OnceCell<()>,
    slots: Semaphore,
    timeout: Duration,
}

impl GitleaksScanner {
    pub(super) fn production() -> Self {
        Self::new(PathBuf::from("/usr/local/bin/gitleaks"))
    }

    fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            verified: OnceCell::new(),
            slots: Semaphore::new(2),
            timeout: SCAN_TIMEOUT,
        }
    }

    async fn run(
        &self,
        arguments: &[&str],
        input: &[u8],
        output_limit: usize,
    ) -> Result<(i32, Vec<u8>), PublicValueScanError> {
        if !self.executable.is_absolute() {
            return Err(PublicValueScanError::Unavailable);
        }
        let mut child = Command::new(&self.executable)
            .args(arguments)
            .env_clear()
            // Explicitly extend only the embedded defaults; never discover caller configuration.
            .env("GITLEAKS_CONFIG_TOML", DEFAULT_RULES)
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| PublicValueScanError::Unavailable)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or(PublicValueScanError::Unavailable)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(PublicValueScanError::Unavailable)?;
        let write = async move {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<(), std::io::Error>(())
        };
        let read = async {
            let mut output = Vec::new();
            stdout
                .take((output_limit + 1) as u64)
                .read_to_end(&mut output)
                .await?;
            if output.len() > output_limit {
                return Err(std::io::Error::other("scanner output exceeded its bound"));
            }
            Ok(output)
        };
        match tokio::try_join!(write, read, child.wait()) {
            Ok(((), output, status)) => Ok((
                status.code().ok_or(PublicValueScanError::Unavailable)?,
                output,
            )),
            Err(_) => {
                let _ = child.kill().await;
                Err(PublicValueScanError::Unavailable)
            }
        }
    }

    async fn inspect(&self, input: &ScanInput) -> Result<(), PublicValueScanError> {
        self.verified
            .get_or_try_init(|| async {
                let (status, output) = self.run(&["version"], &[], 128).await?;
                if status != 0 || std::str::from_utf8(&output).ok().map(str::trim) != Some(VERSION)
                {
                    return Err(PublicValueScanError::Unavailable);
                }
                Ok(())
            })
            .await?;
        let (status, report) = self
            .run(
                &[
                    "stdin",
                    "--no-banner",
                    "--no-color",
                    "--log-level",
                    "fatal",
                    "--redact=100",
                    "--report-format",
                    "json",
                    "--report-path",
                    "-",
                    "--exit-code",
                    "42",
                    "--ignore-gitleaks-allow",
                    "--gitleaks-ignore-path",
                    "/dev/null",
                ],
                input.bytes.as_bytes(),
                MAX_REPORT_BYTES,
            )
            .await?;
        input.result(status, &report)
    }
}

#[async_trait]
impl PublicValueScanner for GitleaksScanner {
    async fn scan(&self, variables: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        if variables.is_empty() {
            return Ok(());
        }
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| PublicValueScanError::Unavailable)?;
        let input = ScanInput::new(variables)?;
        tokio::time::timeout(self.timeout, self.inspect(&input))
            .await
            .map_err(|_| PublicValueScanError::Unavailable)?
    }
}

struct ScanInput {
    bytes: Zeroizing<String>,
    variables: Vec<(String, usize, usize)>,
}

#[derive(Deserialize)]
struct Finding {
    #[serde(rename = "StartLine")]
    start: usize,
    #[serde(rename = "EndLine")]
    end: usize,
}

impl ScanInput {
    fn new(variables: &[EnvironmentVariable]) -> Result<Self, PublicValueScanError> {
        if variables.len() > 64 {
            return Err(PublicValueScanError::Unavailable);
        }
        let mut result = Self {
            bytes: Zeroizing::new(String::new()),
            variables: Vec::new(),
        };
        let mut start = 1;
        for variable in variables {
            let required = variable
                .name
                .len()
                .checked_add(variable.value.len())
                .and_then(|length| length.checked_add(2))
                .ok_or(PublicValueScanError::Unavailable)?;
            if required > MAX_INPUT_BYTES.saturating_sub(result.bytes.len()) {
                return Err(PublicValueScanError::Unavailable);
            }
            let end = start + variable.value.bytes().filter(|byte| *byte == b'\n').count();
            result.variables.push((variable.name.clone(), start, end));
            result.bytes.push_str(&variable.name);
            result.bytes.push('=');
            result.bytes.push_str(&variable.value);
            result.bytes.push('\n');
            start = end + 1;
        }
        Ok(result)
    }

    fn result(&self, status: i32, report: &[u8]) -> Result<(), PublicValueScanError> {
        let findings: Vec<Finding> =
            serde_json::from_slice(report).map_err(|_| PublicValueScanError::Unavailable)?;
        if status == 0 && findings.is_empty() {
            return Ok(());
        }
        if status != 42 || findings.is_empty() {
            return Err(PublicValueScanError::Unavailable);
        }
        let mut found = None;
        for finding in findings {
            if finding.start == 0
                || finding.end < finding.start
                || finding.end > self.variables.last().map_or(0, |(_, _, end)| *end)
            {
                return Err(PublicValueScanError::Unavailable);
            }
            let (name, _, _) = self
                .variables
                .iter()
                .find(|(_, start, end)| finding.start <= *end && finding.end >= *start)
                .ok_or(PublicValueScanError::Unavailable)?;
            found.get_or_insert_with(|| name.clone());
        }
        Err(PublicValueScanError::Secret {
            variable: found.ok_or(PublicValueScanError::Unavailable)?,
        })
    }
}

#[cfg(test)]
#[path = "secret_scanner_tests.rs"]
mod tests;
