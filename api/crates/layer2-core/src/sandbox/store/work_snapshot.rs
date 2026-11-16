//! Content equality for records whose authority expiry can advance independently.

/// Read-only reconciliation accepts the exact committed content after renewal
/// or expiry. It never authorizes another mutation or another effect dispatch.
pub fn sandbox_content_matches(
    actual: Option<&crate::sandbox::Sandbox>,
    expected: &crate::sandbox::Sandbox,
) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    if actual
        .work
        .lease
        .iter()
        .chain(expected.work.lease.iter())
        .any(|lease| crate::seconds_from_rfc3339(&lease.expires_at).is_none())
    {
        return false;
    }
    let mut expected = expected.clone();
    if let (Some(old), Some(now)) = (&mut expected.work.lease, &actual.work.lease) {
        if old.holder == now.holder && old.fence == now.fence && old.expires_at <= now.expires_at {
            old.expires_at.clone_from(&now.expires_at);
        }
    }
    actual == &expected
}

pub fn workspace_content_matches(
    actual: Option<&crate::sandbox::Workspace>,
    expected: &crate::sandbox::Workspace,
) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    if actual
        .work
        .lease
        .iter()
        .chain(expected.work.lease.iter())
        .any(|lease| crate::seconds_from_rfc3339(&lease.expires_at).is_none())
    {
        return false;
    }
    let mut expected = expected.clone();
    if let (Some(old), Some(now)) = (&mut expected.work.lease, &actual.work.lease) {
        if old.holder == now.holder && old.fence == now.fence && old.expires_at <= now.expires_at {
            old.expires_at.clone_from(&now.expires_at);
        }
    }
    actual == &expected
}
