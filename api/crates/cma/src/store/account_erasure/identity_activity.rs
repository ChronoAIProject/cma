//! Concrete audit targets support ownership protection before identity rewriting.
use mongodb::bson::Document;

pub(super) fn sandbox_target(row: &Document) -> Option<&str> {
    let target = row
        .get_document("record")
        .ok()?
        .get_document("target")
        .ok()?;
    if target.get_str("kind").ok()? != "resource" {
        return None;
    }
    let resource = target.get_str("id").ok()?.strip_prefix("sandbox:")?;
    let id = resource.strip_suffix(":events").unwrap_or(resource);
    (!id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then_some(id)
}
