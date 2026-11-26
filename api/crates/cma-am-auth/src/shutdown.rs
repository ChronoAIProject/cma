//! Cooperative shutdown shared by authentication and the host.

use tokio::sync::watch;

pub async fn signalled(mut signal: watch::Receiver<bool>) {
    while !*signal.borrow() {
        if signal.changed().await.is_err() {
            break;
        }
    }
}
