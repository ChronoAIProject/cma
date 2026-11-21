//! Persistent asset generations retain write fences independently of card expiry.
use super::{compile::Compiled, hash, Error};
use crate::{
    channels::resolver::ResolvedChannel,
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    channel::Source,
    ids::{ChannelId, RecordId},
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Store {
    pub records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Index {
    kind: String,
    pub id: String,
    pub owner: String,
    pub channel: ChannelId,
    pub assets: Vec<String>,
    pub active: Option<String>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Asset {
    pub kind: String,
    pub id: String,
    pub registry: String,
    pub owner: String,
    pub channel: ChannelId,
    pub source: Source,
    pub source_version: u64,
    pub credentials: String,
    pub origin_card: String,
    pub admission_until: i64,
    pub digest: String,
    pub name: String,
    pub schema: Value,
    pub phase: Phase,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Phase {
    Reserved,
    CreateAttempted { attempt: String },
    Created { id: String },
    Validated { id: String },
    PublishAttempted { id: String, attempt: String },
    Ready { id: String },
    Declined { reason: String },
    Uncertain { stage: String, id: Option<String> },
    Revoked,
}
pub(crate) fn identifier(value: &impl Serialize) -> String {
    let digest = hash(value);
    let bytes: Vec<_> = (0..32)
        .step_by(2)
        .map(|i| u8::from_str_radix(&digest[i..i + 2], 16).expect("digest hex"))
        .collect();
    let mut id: [u8; 16] = bytes.try_into().expect("sixteen digest bytes");
    id[6] = (id[6] & 15) | 64;
    id[8] = (id[8] & 63) | 128;
    uuid::Uuid::from_bytes(id).to_string()
}
impl Store {
    pub(crate) fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.whatsapp_flows(),
            keys,
        }
    }
    pub(crate) fn registry_id(channel: &ResolvedChannel, source: &Source) -> String {
        let Source::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            ..
        } = source
        else {
            return String::new();
        };
        identifier(&(
            "whatsapp_flow_registry_v1",
            &channel.id,
            &channel.pointer.record_id,
            app_id,
            waba_id,
            phone_number_id,
        ))
    }
    pub(crate) fn asset_id(registry: &str, version: u64, digest: &str) -> String {
        identifier(&("whatsapp_flow_asset_v1", registry, version, digest))
    }
    pub(crate) async fn load<T: DeserializeOwned>(
        &self,
        id: &str,
    ) -> Result<Option<(T, u64)>, Error> {
        let url = self.records.record_url(id).map_err(|_| Error::Invalid)?;
        let row = match self.records.get(url.as_str()).await {
            Ok(v) => v,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(Error::Unavailable),
        };
        let opened = open_interaction(&self.keys, id, &row.blob).map_err(|_| Error::Invalid)?;
        let value = serde_json::from_slice(&opened.payload).map_err(|_| Error::Invalid)?;
        Ok(Some((value, row.version)))
    }
    pub(crate) async fn put<T: Serialize + DeserializeOwned + PartialEq>(
        &self,
        id: &str,
        owner: &str,
        value: &T,
        version: Option<u64>,
    ) -> Result<u64, Error> {
        let bytes = serde_json::to_vec(value).map_err(|_| Error::Invalid)?;
        let blob = seal_interaction(&self.keys, id, &bytes).map_err(|_| Error::Invalid)?;
        match self
            .records
            .put(
                &RecordId::new(id).map_err(|_| Error::Invalid)?,
                owner,
                &blob,
                version,
            )
            .await
        {
            Ok(v) => Ok(v),
            Err(RecordError::Conflict { .. } | RecordError::Unavailable(_)) => {
                match self.load::<T>(id).await? {
                    Some((actual, v)) if actual == *value && v == version.unwrap_or(0) + 1 => Ok(v),
                    Some(_) => Err(Error::Conflict),
                    None => Err(Error::Unavailable),
                }
            }
            Err(_) => Err(Error::Unavailable),
        }
    }
    pub(crate) async fn reserve(
        &self,
        channel: &ResolvedChannel,
        source: &Source,
        compiled: &Compiled,
        card: &str,
        admission_until: i64,
    ) -> Result<String, Error> {
        let registry = Self::registry_id(channel, source);
        let id = Self::asset_id(&registry, channel.version, &compiled.digest);
        let credentials = channel
            .definition
            .secrets
            .whatsapp
            .as_ref()
            .ok_or(Error::Invalid)?;
        let asset = Asset {
            kind: "whatsapp_flow_asset_v1".into(),
            id: id.clone(),
            registry: registry.clone(),
            owner: channel.definition.owner.subject.clone(),
            channel: channel.id.clone(),
            source: source.clone(),
            source_version: channel.version,
            credentials: crate::whatsapp::group_revision(source, credentials),
            origin_card: card.into(),
            admission_until,
            digest: compiled.digest.clone(),
            name: format!(
                "cmaeg_static_v1_{}",
                hash(&(&registry, channel.version, &compiled.digest))
            ),
            schema: compiled.json.clone(),
            phase: Phase::Reserved,
        };
        // Later phases add only bounded IDs/nonces/reasons. Refuse before reserving
        // the generation, so oversized schemas cannot exhaust the durable registry.
        if serde_json::to_vec(&asset).map_or(true, |bytes| {
            bytes.len() + 1024 > cmaeg_core::seal::MAX_BLOB_BYTES
        }) {
            return Err(Error::Invalid);
        }
        for _ in 0..3 {
            let (mut index, version) = match self.load::<Index>(&registry).await? {
                Some((i, v)) => (i, Some(v)),
                None => (
                    Index {
                        kind: "whatsapp_flow_registry_v1".into(),
                        id: registry.clone(),
                        owner: channel.definition.owner.subject.clone(),
                        channel: channel.id.clone(),
                        assets: vec![],
                        active: None,
                    },
                    None,
                ),
            };
            if index.id != registry
                || index.owner != channel.definition.owner.subject
                || index.channel != channel.id
                || index.kind != "whatsapp_flow_registry_v1"
                || index.assets.len() > 64
            {
                return Err(Error::Invalid);
            }
            if !index.assets.contains(&id) {
                if index.assets.len() == 64 {
                    return Err(Error::Conflict);
                }
                index.assets.push(id.clone());
                match self.put(&registry, &index.owner, &index, version).await {
                    Ok(_) => {}
                    Err(Error::Conflict) => continue,
                    Err(e) => return Err(e),
                }
            }
            if let Some((asset, _)) = self.load::<Asset>(&id).await? {
                if !asset.current(channel)
                    || asset.digest != compiled.digest
                    || asset.registry != registry
                {
                    return Err(Error::Revoked);
                }
                return Ok(id);
            }
            match self.put(&id, &asset.owner, &asset, None).await {
                Ok(_) => return Ok(id),
                Err(Error::Conflict) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(Error::Conflict)
    }
    pub(crate) async fn active(&self, asset: &Asset) -> Result<bool, Error> {
        let (mut index, version) = self
            .load::<Index>(&asset.registry)
            .await?
            .ok_or(Error::Invalid)?;
        if index.kind != "whatsapp_flow_registry_v1"
            || index.owner != asset.owner
            || !index.assets.contains(&asset.id)
        {
            return Err(Error::Invalid);
        }
        if let Some(id) = &index.active {
            return Ok(id == &asset.id);
        }
        index.active = Some(asset.id.clone());
        self.put(&index.id, &index.owner, &index, Some(version))
            .await?;
        Ok(true)
    }
    pub(crate) async fn release(&self, asset: &Asset) -> Result<(), Error> {
        let (mut index, version) = self
            .load::<Index>(&asset.registry)
            .await?
            .ok_or(Error::Invalid)?;
        if index.active.as_deref() == Some(&asset.id) {
            index.active = None;
            self.put(&index.id, &index.owner, &index, Some(version))
                .await?;
        }
        Ok(())
    }
    pub(crate) async fn page(
        &self,
        after: Option<&str>,
    ) -> Result<(Vec<Asset>, Option<String>), Error> {
        let heads = self
            .records
            .list(None, after, 32)
            .await
            .map_err(|_| Error::Unavailable)?;
        if heads.len() > 32 {
            return Err(Error::Invalid);
        }
        let next = (heads.len() == 32).then(|| heads.last().unwrap().record_id.clone());
        let mut previous = after.map(str::to_owned);
        let mut assets = vec![];
        for head in heads {
            if previous.as_ref().is_some_and(|p| head.record_id <= *p) {
                return Err(Error::Invalid);
            }
            previous = Some(head.record_id.clone());
            match self.load::<Asset>(&head.record_id).await {
                Ok(Some((asset, _)))
                    if asset.id == head.record_id
                        && asset.owner == head.owner
                        && asset.kind == "whatsapp_flow_asset_v1"
                        && asset.valid() =>
                {
                    assets.push(asset)
                }
                Err(Error::Unavailable) => return Err(Error::Unavailable),
                _ => {}
            }
        }
        Ok((assets, next))
    }
}
impl Asset {
    pub(crate) fn valid(&self) -> bool {
        self.kind == "whatsapp_flow_asset_v1"
            && uuid::Uuid::parse_str(&self.id).is_ok()
            && uuid::Uuid::parse_str(&self.registry).is_ok()
            && uuid::Uuid::parse_str(&self.origin_card).is_ok()
            && self.source_version > 0
            && self.admission_until > 0
            && self.digest == hash(&self.schema)
            && self.schema["version"] == super::JSON_VERSION
            && self.name
                == format!(
                    "cmaeg_static_v1_{}",
                    hash(&(&self.registry, self.source_version, &self.digest))
                )
            && self.id == Store::asset_id(&self.registry, self.source_version, &self.digest)
            && serde_json::to_vec(&self.schema).is_ok_and(|v| v.len() <= super::MAX_JSON_BYTES)
    }
    pub(crate) fn current(&self, channel: &ResolvedChannel) -> bool {
        self.valid()
            && self.channel == channel.id
            && self.source_version == channel.version
            && self.owner == channel.definition.owner.subject
            && channel.definition.sources.contains(&self.source)
            && matches!(
                self.source,
                Source::Whatsapp {
                    forms_enabled: true,
                    ..
                }
            )
            && channel
                .definition
                .secrets
                .whatsapp
                .as_ref()
                .is_some_and(|c| {
                    crate::whatsapp::group_revision(&self.source, c) == self.credentials
                })
    }
}
