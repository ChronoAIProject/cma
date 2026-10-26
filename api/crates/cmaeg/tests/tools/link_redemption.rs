//! Single use of a media link under simultaneous requests: whoever races the
//! agent's own `curl` with a copy of its link, exactly one request is served
//! and every other one is refused as used.
use super::{
    download_support::{autonomous, download, link, relay},
    media_support::{relayed, MediaSetup},
    support::{problem, App},
};
use cmaeg_core::activity::AttachmentKind;
use futures_util::future::join_all;

const REQUESTS: usize = 8;
/// A fresh link each round, so a narrow redemption race has many chances.
const ROUNDS: usize = 16;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_uses_of_one_link_are_served_once() {
    let original = vec![7_u8; 1024];
    let setup = MediaSetup::new(vec![relayed(0, AttachmentKind::File)]).edit(autonomous);
    setup.fakes.source.serve(&relay(0), Ok(original.clone()));
    let app = App::with_media(setup).await;
    for round in 0..ROUNDS {
        let token = link(&app, 0);
        let responses = join_all((0..REQUESTS).map(|_| download(&app, &token))).await;
        let (served, refused): (Vec<_>, Vec<_>) = responses
            .into_iter()
            .partition(|response| response.status() == 200);
        assert_eq!(served.len(), 1, "round {round}");
        for response in refused {
            let body = problem(response, 410, "media_unavailable").await;
            assert_eq!(body["detail"], "media_link_used", "round {round}");
        }
        // Reading the body to its end releases the stream slots.
        for response in served {
            assert_eq!(response.bytes().await.unwrap(), original, "round {round}");
        }
    }
}
