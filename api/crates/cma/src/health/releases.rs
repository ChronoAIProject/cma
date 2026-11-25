use serde::Serialize;

#[derive(Serialize)]
pub(super) struct Lane<'a> {
    digest: &'a str,
    revision: u64,
}

pub(super) fn lane(image: &str, revision: u64, machine: bool) -> Option<Lane<'_>> {
    let expected = if machine {
        "ghcr.io/chronoaiproject/cma-runtime-omarchy"
    } else {
        "ghcr.io/chronoaiproject/cma-runtime"
    };
    let (repository, digest) = image.split_once('@')?;
    (repository == expected && crate::runtime_pointer::valid_digest(digest))
        .then_some(Lane { digest, revision })
}
