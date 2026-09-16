use super::*;
use crate::store::sandbox_mongo::read_record;
use serde::{Deserialize, Serialize};

pub(in crate::store) fn read_admitted(
    document: &Document,
) -> Result<AdmittedChannelControl, Error> {
    if document.contains_key("channel_admission") {
        return Err(Error::Corrupt);
    }
    let result = AdmittedChannelControl {
        command: read_command(document).map_err(|_| Error::Corrupt)?,
        sidecar: mongodb::bson::deserialize_from_bson(
            document
                .get("channel_control_admission")
                .cloned()
                .ok_or(Error::Corrupt)?,
        )
        .map_err(|_| Error::Corrupt)?,
    };
    result.validate()?;
    Ok(result)
}

pub(super) fn intent_document(intent: &ChannelControlIntent) -> Result<Document, Error> {
    Ok(
        doc! {"_id":&intent.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "intent_sha256":intent.fingerprint()?,"record":record_bson(intent).map_err(|_| Error::Corrupt)?},
    )
}

pub(in crate::store) fn read_intent(document: &Document) -> Result<ChannelControlIntent, Error> {
    let intent: ChannelControlIntent = read_record(document).map_err(|_| Error::Corrupt)?;
    let expected = intent_document(&intent)?;
    if ["_id", "intent_sha256"]
        .iter()
        .any(|key| document.get(*key) != expected.get(*key))
    {
        return Err(Error::Corrupt);
    }
    Ok(intent)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Alias {
    schema_version: u8,
    callback: ChannelControlCallback,
    intent_id: String,
    intent_sha256: String,
}

pub(super) fn alias_document(
    callback: &ChannelControlCallback,
    intent: &ChannelControlIntent,
) -> Result<Document, Error> {
    intent.callback.require_alias(callback)?;
    let alias = Alias {
        schema_version: 1,
        callback: callback.clone(),
        intent_id: intent.id.clone(),
        intent_sha256: intent.fingerprint()?,
    };
    Ok(
        doc! {"_id":&callback.callback_id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "record":record_bson(&alias).map_err(|_| Error::Corrupt)?},
    )
}

pub(super) fn require_alias(
    document: &Document,
    callback: &ChannelControlCallback,
    intent: &ChannelControlIntent,
) -> Result<(), Error> {
    let alias: Alias = read_record(document).map_err(|_| Error::Corrupt)?;
    alias.callback.require_callback(callback)?;
    let expected = alias_document(&alias.callback, intent)?;
    if ["_id", "record"]
        .iter()
        .any(|key| document.get(*key) != expected.get(*key))
    {
        return Err(Error::Corrupt);
    }
    Ok(())
}

/// Validate a retained alias without exposing its private storage model.
pub(in crate::store) fn history_alias(
    document: &Document,
    intent: &ChannelControlIntent,
) -> Result<ChannelControlCallback, Error> {
    let alias: Alias = read_record(document).map_err(|_| Error::Corrupt)?;
    require_alias(document, &alias.callback, intent)?;
    Ok(alias.callback)
}
