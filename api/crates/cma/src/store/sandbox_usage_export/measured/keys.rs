use super::*;

const FIELD: &str = "measured_usage_v1";

pub(super) fn prefix(kind: &str, namespace: &str) -> String {
    let namespace: String = namespace
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{kind}/{namespace}/")
}

pub(super) fn id(key: &str) -> Document {
    doc! {FIELD:key}
}

pub(super) fn key(row: &Document) -> Result<&str, SandboxStoreError> {
    let id = row.get_document("_id").map_err(unavailable)?;
    if id.len() != 1 {
        return Err(corrupt());
    }
    id.get_str(FIELD).map_err(unavailable)
}

pub(super) fn page(prefix: &str, after: Option<&str>) -> Result<Document, SandboxStoreError> {
    let stem = prefix.strip_suffix('/').ok_or_else(corrupt)?;
    if !prefix.is_ascii() || after.is_some_and(|after| !after.starts_with(prefix)) {
        return Err(corrupt());
    }
    let mut range = doc! {"$lt":id(&format!("{stem}0"))};
    if let Some(after) = after {
        range.insert("$gt", id(after));
    } else {
        range.insert("$gte", id(prefix));
    }
    Ok(doc! {"_id":range})
}

pub(super) fn stream(namespace: &str, source: &str) -> String {
    format!("{}{source}", prefix("stream", namespace))
}

pub(super) fn head(stream: &str, minute: u64) -> String {
    format!("head/{stream}/{minute:016x}")
}

pub(super) fn bridge(binding: &str) -> String {
    format!("binding/{binding}/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_encoding_and_typed_ids_cannot_alias_source_or_transport_keys() {
        assert_ne!(prefix("stream", "a/b"), prefix("stream/a", "b"));
        assert_ne!(
            id("stream/fixture"),
            doc! {"usage_transport_namespace":"stream/fixture"}
        );
        let prefix = prefix("stream", "fixture");
        assert!(page(&prefix, Some("stream/foreign/id")).is_err());
        assert!(page(&prefix, Some(&format!("{prefix}source"))).is_ok());
    }
}
