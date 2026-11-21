//! Every mutation is preceded by an acknowledged CAS marker. Recovery only reads
//! after an uncertain attempt, and this type has no customer-message sender.
use super::{
    management::Manager,
    registry::{Asset, Phase, Store},
    Error,
};
use crate::{channels::resolver::ChannelResolver, interactions::InteractionStore};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Provisioner {
    pub store: Store,
    pub resolver: ChannelResolver,
    pub cards: InteractionStore,
    pub manager: Arc<dyn Manager>,
    pub writes_enabled: bool,
}
impl Provisioner {
    pub(crate) async fn advance(&self, id: &str) -> Result<(), Error> {
        let (mut asset, version) = self.store.load::<Asset>(id).await?.ok_or(Error::Invalid)?;
        if !asset.valid() {
            return Err(Error::Invalid);
        }
        if matches!(
            asset.phase,
            Phase::Ready { .. } | Phase::Declined { .. } | Phase::Revoked
        ) {
            return self.store.release(&asset).await;
        }
        let channel = self
            .resolver
            .resolve_current(asset.channel.as_str())
            .await
            .map_err(|_| Error::Unavailable)?;
        if !asset.current(&channel)
            || (matches!(asset.phase, Phase::Reserved | Phase::Validated { .. })
                && asset.admission_until <= time::OffsetDateTime::now_utc().unix_timestamp())
        {
            asset.phase = Phase::Revoked;
            self.store
                .put(&asset.id, &asset.owner, &asset, Some(version))
                .await?;
            return self.store.release(&asset).await;
        }
        if !self.store.active(&asset).await? {
            return Ok(());
        }
        let credentials = channel
            .definition
            .secrets
            .whatsapp
            .as_ref()
            .ok_or(Error::Revoked)?;
        let next = match &asset.phase {
            Phase::Reserved | Phase::Validated { .. } => {
                if !self.writes_enabled {
                    return Ok(());
                }
                // Only the original exact preparation can authorize management mutations.
                // A reservation whose card write was interrupted waits for same-key recovery.
                let (card, _) = match self.cards.load_whatsapp_flow(&asset.origin_card).await {
                    Ok(v) => v,
                    Err(crate::interactions::Error::Denied) => return Ok(()),
                    Err(_) => return Err(Error::Unavailable),
                };
                self.cards
                    .refresh_whatsapp_flow(&card)
                    .await
                    .map_err(|_| Error::Revoked)?;
                if card
                    .native_whatsapp_flow
                    .as_ref()
                    .is_none_or(|a| a.asset_registry != asset.id || a.asset_digest != asset.digest)
                {
                    asset.phase = Phase::Revoked;
                    self.store
                        .put(&asset.id, &asset.owner, &asset, Some(version))
                        .await?;
                    return self.store.release(&asset).await;
                }
                self.manager
                    .verify(&asset.source, credentials)
                    .await
                    .map_err(|_| Error::Revoked)?;
                // Recheck after potentially slow remote proof, immediately before the attempt CAS.
                self.cards
                    .refresh_whatsapp_flow(&card)
                    .await
                    .map_err(|_| Error::Revoked)?;
                if asset.admission_until <= time::OffsetDateTime::now_utc().unix_timestamp() {
                    return Err(Error::Revoked);
                }
                let operation = asset.phase.clone();
                asset.phase = match &operation {
                    Phase::Reserved => Phase::CreateAttempted {
                        attempt: uuid::Uuid::new_v4().to_string(),
                    },
                    Phase::Validated { id } => Phase::PublishAttempted {
                        id: id.clone(),
                        attempt: uuid::Uuid::new_v4().to_string(),
                    },
                    _ => unreachable!(),
                };
                let attempted = self
                    .store
                    .put(&asset.id, &asset.owner, &asset, Some(version))
                    .await?;
                if asset.admission_until <= time::OffsetDateTime::now_utc().unix_timestamp() {
                    return Err(Error::Revoked);
                }
                let result = match operation {
                    Phase::Reserved => match self
                        .manager
                        .create(&asset.source, credentials, &asset.name, &asset.schema)
                        .await
                    {
                        Ok(id) => Phase::Created { id },
                        Err(e) => write_failure("create", None, e),
                    },
                    Phase::Validated { id } => match self.manager.publish(credentials, &id).await {
                        Ok(()) => asset.phase.clone(),
                        Err(e) => write_failure("publish", Some(id), e),
                    },
                    _ => unreachable!(),
                };
                asset.phase = result;
                self.store
                    .put(&asset.id, &asset.owner, &asset, Some(attempted))
                    .await?;
                return Ok(());
            }
            Phase::Created { id } => {
                match self
                    .manager
                    .inspect(&asset.source, credentials, id, &asset.name)
                    .await
                {
                    Ok(proof) if proof.digest == asset.digest && proof.id == *id => {
                        if proof.published {
                            Phase::Ready { id: id.clone() }
                        } else {
                            Phase::Validated { id: id.clone() }
                        }
                    }
                    Ok(_) => Phase::Declined {
                        reason: "whatsapp_flow_schema_diverged".into(),
                    },
                    Err(_) => return Ok(()),
                }
            }
            Phase::CreateAttempted { .. } | Phase::Uncertain { stage: _, id: None } => {
                let candidates = match self
                    .manager
                    .candidates(&asset.source, credentials, &asset.name)
                    .await
                {
                    Ok(ids) => ids,
                    Err(_) => return Ok(()),
                };
                if candidates.len() != 1 {
                    return Ok(());
                }
                match self
                    .manager
                    .inspect(&asset.source, credentials, &candidates[0], &asset.name)
                    .await
                {
                    Ok(proof) if proof.digest == asset.digest && proof.id == candidates[0] => {
                        if proof.published {
                            Phase::Ready { id: proof.id }
                        } else {
                            Phase::Created { id: proof.id }
                        }
                    }
                    _ => return Ok(()),
                }
            }
            Phase::PublishAttempted { id, .. } | Phase::Uncertain { id: Some(id), .. } => {
                match self
                    .manager
                    .inspect(&asset.source, credentials, id, &asset.name)
                    .await
                {
                    Ok(proof)
                        if proof.published && proof.digest == asset.digest && proof.id == *id =>
                    {
                        Phase::Ready { id: id.clone() }
                    }
                    _ => return Ok(()),
                }
            }
            Phase::Ready { .. } | Phase::Declined { .. } | Phase::Revoked => return Ok(()),
        };
        asset.phase = next;
        self.store
            .put(&asset.id, &asset.owner, &asset, Some(version))
            .await?;
        if matches!(
            asset.phase,
            Phase::Ready { .. } | Phase::Declined { .. } | Phase::Revoked
        ) {
            self.store.release(&asset).await?;
        }
        Ok(())
    }
}
fn write_failure(stage: &str, id: Option<String>, error: crate::whatsapp::Error) -> Phase {
    match error {
        crate::whatsapp::Error::Rejected
        | crate::whatsapp::Error::Invalid
        | crate::whatsapp::Error::CustodyUnproved => Phase::Declined {
            reason: format!("whatsapp_flow_{stage}_declined"),
        },
        // An HTTP rate-limit response is also retained; a new key cannot reset a write.
        _ => Phase::Uncertain {
            stage: stage.into(),
            id,
        },
    }
}
