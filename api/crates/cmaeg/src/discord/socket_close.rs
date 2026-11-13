//! Bounded close evidence, separate from requesting shutdown or dropping a socket.
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::{
    tungstenite::{Error, Message},
    WebSocketStream,
};

pub(super) async fn confirmed<S: AsyncRead + AsyncWrite + Unpin>(
    socket: &mut WebSocketStream<S>,
) -> bool {
    tokio::time::timeout(Duration::from_secs(2), async {
        match socket.close(None).await {
            Ok(()) => {}
            Err(Error::ConnectionClosed | Error::AlreadyClosed) => return true,
            Err(_) => return false,
        }
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(_))) => {
                    // Flush an automatic close reply when the peer initiated it.
                    return matches!(socket.flush().await, Ok(()) | Err(Error::ConnectionClosed));
                }
                None | Some(Err(Error::ConnectionClosed | Error::AlreadyClosed)) => return true,
                Some(Err(_)) => return false,
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test]
    async fn close_requires_peer_handshake_and_stops_before_handoff() {
        let (client, peer) = tokio::io::duplex(4096);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let close = tokio::spawn(async move { confirmed(&mut client).await });
        assert!(matches!(peer.next().await, Some(Ok(Message::Close(_)))));
        assert!(!close.is_finished());
        peer.flush().await.unwrap();
        assert!(close.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn peer_that_never_acknowledges_does_not_prove_release_safe() {
        let (client, peer) = tokio::io::duplex(4096);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let _peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        assert!(!confirmed(&mut client).await);
    }
}
