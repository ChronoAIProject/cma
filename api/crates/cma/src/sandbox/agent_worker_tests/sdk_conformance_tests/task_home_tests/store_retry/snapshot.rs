use super::*;

pub(super) async fn agent(harness: &Harness<MongoSandboxStore>, id: &AgentId) -> String {
    read(async {
        Ok(harness.store.get_agent(id).await?.map(|agent| {
            let lease = agent
                .work
                .lease
                .as_ref()
                .map(|lease| lease.expires_at.as_str());
            format!(
                "status={:?} step={:?} attempts={} {}",
                agent.status,
                agent.work.step,
                agent.work.attempts,
                timing(&agent.work.ready_at, lease, SystemTime::now())
            )
        }))
    })
    .await
}

pub(super) async fn sandbox(harness: &Harness<MongoSandboxStore>, id: &SandboxId) -> String {
    read(async {
        Ok(harness.store.get_sandbox(id).await?.map(|sandbox| {
            let lease = sandbox
                .work
                .lease
                .as_ref()
                .map(|lease| lease.expires_at.as_str());
            format!(
                "status={:?} step={:?} attempts={} {}",
                sandbox.status,
                sandbox.work.step,
                sandbox.work.attempts,
                timing(&sandbox.work.ready_at, lease, SystemTime::now())
            )
        }))
    })
    .await
}

pub(super) async fn read(
    future: impl std::future::Future<Output = Result<Option<String>, SandboxStoreError>>,
) -> String {
    match tokio::time::timeout(Duration::from_millis(250), future).await {
        Ok(Ok(Some(state))) => state,
        Ok(Ok(None)) => "record_missing".into(),
        Ok(Err(error)) => format!("read_error={}", super::error_class(&error)),
        Err(_) => "read_timeout".into(),
    }
}

fn timing(ready: &str, lease: Option<&str>, now: SystemTime) -> String {
    let relative = |stamp: &str| {
        let seconds = layer2_core::seconds_from_rfc3339(stamp)?;
        let now_ms = now.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as i128;
        Some(i128::from(seconds) * 1000 - now_ms)
    };
    format!(
        "due_in_ms={:?} lease_present={} lease_in_ms={:?}",
        relative(ready),
        lease.is_some(),
        lease.and_then(relative)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_reports_signed_offsets_without_raw_timestamps() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_millis(10_500);
        assert_eq!(
            timing("1970-01-01T00:00:09Z", Some("1970-01-01T00:00:12Z"), now),
            "due_in_ms=Some(-1500) lease_present=true lease_in_ms=Some(1500)"
        );
        assert_eq!(
            timing("invalid", None, now),
            "due_in_ms=None lease_present=false lease_in_ms=None"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failed_or_stalled_diagnostic_reads_do_not_mask_the_worker_error() {
        assert_eq!(read(std::future::pending()).await, "read_timeout");
        assert_eq!(
            read(async { Err(SandboxStoreError::Unavailable("private detail".into())) }).await,
            "read_error=Unavailable"
        );
        assert_eq!(read(async { Ok(None) }).await, "record_missing");
    }
}
