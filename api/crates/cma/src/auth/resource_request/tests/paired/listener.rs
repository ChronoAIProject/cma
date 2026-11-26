use super::{Phase, Receipt};
use futures_util::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe, time::Duration};
use tokio::{sync::oneshot, task::JoinHandle};

pub(super) struct Listener {
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Listener {
    pub(super) fn start(socket: tokio::net::TcpListener, app: axum::Router) -> Self {
        let (shutdown, receive) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(socket, app)
                .with_graceful_shutdown(async {
                    let _ = receive.await;
                })
                .await
        });
        Self {
            shutdown: Some(shutdown),
            task,
        }
    }

    async fn close(&mut self) -> bool {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match tokio::time::timeout(Duration::from_secs(2), &mut self.task).await {
            Ok(Ok(Ok(()))) => true,
            Ok(_) => false,
            Err(_) => {
                self.task.abort();
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut self.task).await;
                false
            }
        }
    }

    pub(super) async fn run(&mut self, exercise: impl Future<Output = ()>) {
        let outcome = AssertUnwindSafe(exercise).catch_unwind().await;
        let receipt = Receipt::start(Phase::ListenerClosed);
        let closed = self.close().await;
        if closed {
            receipt.pass();
        } else {
            drop(receipt);
        }
        // Teardown cannot replace the matrix's original assertion failure.
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
        assert!(closed, "paired CMA listener did not close cleanly");
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn listener_is_joined_after_success_and_matrix_panic() {
    for fail in [false, true] {
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let mut listener = Listener::start(socket, axum::Router::new());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let result = AssertUnwindSafe(listener.run(async {
            let response = client
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
            drop(response);
            if fail {
                std::panic::panic_any(42_u8);
            }
        }))
        .catch_unwind()
        .await;
        if fail {
            assert_eq!(*result.unwrap_err().downcast::<u8>().unwrap(), 42);
        } else {
            result.unwrap();
        }
        assert!(listener.task.is_finished());
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
    }
}

#[tokio::test]
async fn failed_listener_keeps_the_original_matrix_panic() {
    let mut listener = Listener {
        shutdown: None,
        task: tokio::spawn(async { Err(std::io::Error::other("synthetic close failure")) }),
    };
    let result = AssertUnwindSafe(listener.run(async {
        std::panic::panic_any(42_u8);
    }))
    .catch_unwind()
    .await;
    assert_eq!(*result.unwrap_err().downcast::<u8>().unwrap(), 42);
}

#[tokio::test(start_paused = true)]
async fn stuck_listener_fails_after_bounded_abort_and_join() {
    let mut listener = Listener {
        shutdown: None,
        task: tokio::spawn(std::future::pending()),
    };
    let result = AssertUnwindSafe(listener.run(async {}))
        .catch_unwind()
        .await;
    assert!(result.is_err());
    assert!(listener.task.is_finished());
}
