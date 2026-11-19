//! Observe the child-owned listeners without a parent-side port reservation.
use std::{
    io::{self, Write},
    net::SocketAddr,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, ChildStdout},
    sync::oneshot,
};

pub(super) struct Endpoints {
    pub http: SocketAddr,
    pub metrics: SocketAddr,
}

fn endpoints(line: &[u8]) -> io::Result<Option<Endpoints>> {
    let line = String::from_utf8_lossy(line);
    let Some(addresses) = line.strip_prefix("CMAEG_BOUND_LISTENERS ") else {
        return Ok(None);
    };
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid gateway listener report",
        )
    };
    let (http, metrics) = addresses.trim_end().split_once(' ').ok_or_else(invalid)?;
    let http: SocketAddr = http.parse().map_err(|_| invalid())?;
    let metrics: SocketAddr = metrics.parse().map_err(|_| invalid())?;
    if [http, metrics]
        .iter()
        .any(|addr| !addr.ip().is_loopback() || addr.port() == 0)
        || http == metrics
    {
        return Err(invalid());
    }
    Ok(Some(Endpoints { http, metrics }))
}

pub(super) async fn collect(
    stdout: ChildStdout,
    mut log: impl Write,
    ready: oneshot::Sender<io::Result<Endpoints>>,
) -> io::Result<()> {
    let mut stdout = BufReader::new(stdout);
    let mut ready = Some(ready);
    let mut line = Vec::new();
    while stdout.read_until(b'\n', &mut line).await? != 0 {
        log.write_all(&line)?;
        if let Some(sender) = ready.take() {
            match endpoints(&line).transpose() {
                Some(addresses) => {
                    // A cancelled test no longer needs the handshake. Finish
                    // draining the child trace even if the receiver is gone.
                    let _ = sender.send(addresses);
                }
                None => ready = Some(sender),
            }
        }
        line.clear();
    }
    if let Some(sender) = ready {
        let _ = sender.send(Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "gateway exited before reporting its bound listeners",
        )));
    }
    Ok(())
}

pub(super) async fn ready_endpoints(
    child: &mut Child,
    report: oneshot::Receiver<io::Result<Endpoints>>,
    deadline: Duration,
) -> Result<Endpoints, String> {
    let startup = async {
        let endpoints = match report.await {
            Ok(Ok(endpoints)) => endpoints,
            Ok(Err(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                // EOF can arrive before the process status. Keep the original
                // deadline while waiting for the status of this exact child.
                child.wait().await.map_err(|_| "child status unavailable")?;
                return Err("child exited before reporting listeners");
            }
            Ok(Err(_)) => return Err("invalid listener report"),
            Err(_) => return Err("listener report stream failed"),
        };
        let (ready, metrics) = tokio::try_join!(
            reqwest::get(format!("http://{}/readyz", endpoints.http)),
            reqwest::get(format!("http://{}/metrics", endpoints.metrics)),
        )
        .map_err(|_| "listener probe transport failed")?;
        if ready.status() != reqwest::StatusCode::OK || metrics.status() != reqwest::StatusCode::OK
        {
            return Err("listener probe refused readiness");
        }
        Ok(endpoints)
    };
    let failure = match tokio::time::timeout(deadline, startup).await {
        Ok(Ok(endpoints)) => return Ok(endpoints),
        Ok(Err(category)) => category,
        Err(_) => "startup deadline elapsed",
    };
    // Keep the diagnostic bounded to a fixed category and OS exit status.
    // The child trace can contain arbitrary fixture output.
    let status = match child.try_wait() {
        Ok(Some(status)) => status.to_string(),
        Ok(None) => "child still running".to_owned(),
        Err(_) => "child status unavailable".to_owned(),
    };
    Err(format!("gateway startup failed ({failure}, {status})"))
}

#[cfg(unix)]
#[tokio::test]
async fn early_child_exit_reports_status_without_echoing_its_output() {
    let mut child = tokio::process::Command::new("sh")
        .args(["-c", "printf '%s\\n' private-fixture-text; exit 17"])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (send, receive) = oneshot::channel();
    let collector = tokio::spawn(collect(child.stdout.take().unwrap(), io::sink(), send));
    let Err(diagnostic) = ready_endpoints(&mut child, receive, Duration::from_secs(5)).await else {
        panic!("an exited child reported readiness");
    };
    collector.await.unwrap().unwrap();
    assert_eq!(
        diagnostic,
        "gateway startup failed (child exited before reporting listeners, exit status: 17)"
    );
}

#[test]
fn endpoint_report_requires_two_distinct_bound_loopback_sockets() {
    let good = endpoints(b"CMAEG_BOUND_LISTENERS 127.0.0.1:123 127.0.0.1:456\n")
        .unwrap()
        .unwrap();
    assert_eq!(good.http.port(), 123);
    assert_eq!(good.metrics.port(), 456);
    assert!(endpoints(b"INFO unrelated line").unwrap().is_none());
    for record in [
        "127.0.0.1:123",
        "127.0.0.1:123 127.0.0.1:123",
        "127.0.0.1:0 127.0.0.1:456",
        "127.0.0.1:123 0.0.0.0:456",
    ] {
        assert!(endpoints(format!("CMAEG_BOUND_LISTENERS {record}").as_bytes()).is_err());
    }
}
