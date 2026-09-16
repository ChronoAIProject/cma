//! Only network IO is replaced: source grant, custody, current invoke authority,
//! validation, immutable capture and Agent admission use production code.
use super::*;
use crate::{
    bridge::channel_scheduler::ChannelWorkError,
    store::{MongoBridgeInvokeStore, MongoCallbackInbox},
};
use layer2_core::bridge::{
    ingress::StoredInbox,
    invoke::{BridgeInvokeProof, BridgeInvokeService},
};

pub(crate) async fn capture(
    inbox: &StoredInbox,
    proof: &BridgeInvokeProof,
    invokes: &BridgeInvokeService,
    custody: &MongoCallbackInbox,
    store: &MongoBridgeInvokeStore,
    bytes: &[u8],
) -> Result<StoredInbox, ChannelWorkError> {
    worker::capture_using(
        inbox,
        proof,
        invokes,
        custody,
        store,
        (&Dns, &Http(bytes.to_vec())),
    )
    .await
}

struct Dns;
#[async_trait::async_trait]
impl Resolver for Dns {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, Unavailable> {
        assert_eq!(host, "cdn.discordapp.com");
        Ok(vec!["8.8.8.8".parse().unwrap()])
    }
}
struct Http(Vec<u8>);
#[async_trait::async_trait]
impl Transport for Http {
    async fn get(&self, target: &PinnedTarget) -> Result<network::DownloadResponse, Unavailable> {
        assert_eq!(target.host, "cdn.discordapp.com");
        assert_eq!(
            target.url.path(),
            "/attachments/123456789012345678/523456789012345678/note.txt"
        );
        Ok(network::DownloadResponse {
            status: 200,
            peer: target.addresses[0],
            mime: Some("text/plain".into()),
            encoding: None,
            length: None,
            body: Box::new(Body(Some(self.0.clone()))),
        })
    }
}
struct Body(Option<Vec<u8>>);
#[async_trait::async_trait]
impl network::DownloadBody for Body {
    async fn next(&mut self, remaining: usize) -> Result<Option<Vec<u8>>, Unavailable> {
        let bytes = self.0.take();
        if bytes.as_ref().is_some_and(|bytes| bytes.len() > remaining) {
            return Err(Unavailable::Bounds);
        }
        Ok(bytes)
    }
}
