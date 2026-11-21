//! Consumed cards are the encrypted outbox: no second write is needed for discoverability.
use super::super::{Error, InteractionStore};
use cmaeg_core::seal::{open_consumer_interaction, open_interaction};
#[derive(Default)]
pub(crate) struct RecoveryPage {
    pub cards: Vec<(String, String)>,
    pub next: Option<String>,
}
impl InteractionStore {
    pub(crate) async fn whatsapp_flow_recovery_page(
        &self,
        after: Option<&str>,
    ) -> Result<RecoveryPage, Error> {
        let records = self.channels.clone().whatsapp_flows();
        let rows = records
            .list(None, after, 32)
            .await
            .map_err(|_| Error::Unavailable)?;
        let mut result = RecoveryPage {
            cards: vec![],
            next: (rows.len() == 32).then(|| rows.last().unwrap().record_id.clone()),
        };
        let mut previous = after.map(str::to_owned);
        if rows.len() > 32 {
            return Err(Error::Unavailable);
        }
        for head in rows {
            if previous.as_ref().is_some_and(|p| head.record_id <= *p) {
                return Err(Error::Unavailable);
            }
            previous = Some(head.record_id.clone());
            let url = records
                .record_url(&head.record_id)
                .map_err(|_| Error::Unavailable)?;
            let row = match records.get(url.as_str()).await {
                Ok(row) => row,
                Err(crate::records::RecordError::NotFound) => continue,
                Err(_) => return Err(Error::Unavailable),
            };
            let Ok(opened) = open_interaction(&self.keys, &head.record_id, &row.blob)
                .or_else(|_| open_consumer_interaction(&self.keys, &head.record_id, &row.blob))
            else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&opened.payload) else {
                continue;
            };
            if value
                .get("native_whatsapp_flow")
                .is_none_or(serde_json::Value::is_null)
            {
                continue;
            }
            if value
                .get("expires_at")
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|expires| expires <= super::super::now())
            {
                continue;
            }
            let (card, _) = match self.load_whatsapp_flow(&head.record_id).await {
                Ok(card) => card,
                Err(Error::Denied) => continue,
                Err(e) => return Err(e),
            };
            if card.owner != head.owner {
                continue;
            }
            if !card.superseded
                && card.native_whatsapp_flow.as_ref().is_some_and(|a| {
                    matches!(a.phase, super::Phase::ConfirmedUnbound { .. })
                        || a.pending.as_ref().is_some_and(|p| !p.admitted)
                })
            {
                result.cards.push((card.id, card.channel_id.to_string()));
            }
        }
        Ok(result)
    }
}

impl InteractionStore {
    pub(crate) async fn recover_whatsapp_flow_binding(&self, id: &str) -> Result<(), Error> {
        let (mut card, version) = self.load_whatsapp_flow(id).await?;
        self.refresh_whatsapp_flow(&card).await?;
        if card
            .native_whatsapp_flow
            .as_ref()
            .is_some_and(|a| matches!(a.phase, super::Phase::ConfirmedUnbound { .. }))
        {
            self.bind_whatsapp_flow(&mut card, version).await?;
        }
        Ok(())
    }
}
