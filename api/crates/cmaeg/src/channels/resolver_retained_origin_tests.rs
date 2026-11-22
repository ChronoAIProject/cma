//! Ingress resolution of channel IDs sealed under an earlier CMA origin.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::ids::RecordId;

const RECORD: &str = "6f9c1f43-5a1e-4c43-9b0e-3f8f61d1c2a7";
const EARLIER: &str = "http://cma-api.earlier.svc";

fn keys() -> Arc<MasterKeyRing> {
    Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([41; 32]))).unwrap())
}

/// A gateway whose configured CMA refuses connections: a pointer that passes
/// the identity check reaches the read and reports the store unavailable.
fn resolver(keys: &Arc<MasterKeyRing>, retained: &[&str]) -> ChannelResolver {
    let records = RecordClient::new("http://127.0.0.1:1", "fixture")
        .unwrap()
        .with_retained_origins(&retained.iter().map(|o| o.to_string()).collect::<Vec<_>>())
        .unwrap();
    ChannelResolver::new(Arc::new(records), keys.clone(), Default::default())
}

fn sealed_under(keys: &MasterKeyRing, origin: &str) -> String {
    ChannelPointer {
        v: 1,
        record_url: format!("{origin}/api/v1/gateway/channel-records/{RECORD}"),
        record_id: RecordId::new(RECORD).unwrap(),
    }
    .seal_id(keys)
    .unwrap()
    .as_str()
    .to_owned()
}

#[tokio::test]
async fn an_earlier_origin_is_tampered_until_retained() {
    let keys = keys();
    let id = sealed_under(&keys, EARLIER);
    let unretained = resolver(&keys, &[]);
    assert_eq!(
        unretained.resolve(&id).await.err(),
        Some(ResolveError::Tampered)
    );
    assert_eq!(
        unretained.resolve_current(&id).await.err(),
        Some(ResolveError::Tampered)
    );

    let retained = resolver(&keys, &[EARLIER]);
    assert_eq!(
        retained.resolve(&id).await.err(),
        Some(ResolveError::Unavailable)
    );
    assert_eq!(
        retained.resolve_current(&id).await.err(),
        Some(ResolveError::Unavailable)
    );
}

#[tokio::test]
async fn an_unlisted_origin_stays_tampered_beside_a_retained_one() {
    let keys = keys();
    let resolver = resolver(&keys, &[EARLIER]);
    for origin in ["http://cma-api.unlisted.svc", "https://cma-api.earlier.svc"] {
        let id = sealed_under(&keys, origin);
        assert_eq!(
            resolver.resolve(&id).await.err(),
            Some(ResolveError::Tampered),
            "{origin}"
        );
    }
}
