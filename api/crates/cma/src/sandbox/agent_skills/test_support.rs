use super::*;
use crate::dependency_test_support::Peer;

pub(in crate::sandbox) fn scoped_peer(
    peer: &Peer,
    credentials: Arc<dyn SandboxCredentialController>,
) -> Arc<OrnnAgentSkillResolver> {
    // Exercise production authentication, limits and redirects against only the loopback peer.
    let mut resolver = OrnnAgentSkillResolver::scoped("https://platform.example", credentials)
        .expect("synthetic platform origin is valid");
    Arc::get_mut(&mut resolver).unwrap().request_origin =
        reqwest::Url::parse(&peer.origin).unwrap();
    resolver
}
