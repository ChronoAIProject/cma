//! Observe the child-owned listeners without a parent-side port reservation.
use std::{
    io::{self, Write},
    net::SocketAddr,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStdout,
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
    mut log: std::fs::File,
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
