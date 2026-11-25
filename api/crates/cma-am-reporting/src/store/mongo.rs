//! MongoDB handles and operations used by the reporting persistence.
use crate::{transaction_scope as tx, unavailable};
use layer2_core::sandbox_metrics::{cursor::CursorSigner, MetricsError, Result};
use mongodb::{
    bson::{doc, Document},
    Client, ClientSession, Collection, Database,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct ReportingStore {
    pub client: Client,
    pub db: Database,
    pub signer: Arc<CursorSigner>,
    pub(crate) transaction_policy: tx::Policy,
}

impl ReportingStore {
    pub fn new(client: Client, db: Database, key: &[u8], transaction_policy: tx::Policy) -> Self {
        Self {
            client,
            db,
            signer: Arc::new(CursorSigner::new(key)),
            transaction_policy,
        }
    }

    pub fn with_client(&self, client: Client) -> Self {
        let db = client.database(self.db.name());
        Self {
            client,
            db,
            signer: Arc::clone(&self.signer),
            transaction_policy: self.transaction_policy,
        }
    }

    pub fn collection(&self, name: &str) -> Collection<Document> {
        self.db.collection(name)
    }

    pub async fn snapshot(&self) -> Result<ClientSession> {
        self.client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)
    }

    pub async fn bulk_updates_with_deadline(
        &self,
        collection: &str,
        updates: Vec<Document>,
        session: Option<&mut mongodb::ClientSession>,
        deadline: Duration,
    ) -> tx::Result<()> {
        if updates.len() > 200 {
            return Err(MetricsError::Busy.into());
        }
        let command = self.db.run_command(
            doc! {"update":collection,"updates":updates,"ordered":false,"maxTimeMS":deadline.as_millis() as i64},
        );
        let result = match session {
            Some(session) => command.session(session).await,
            None => command.await,
        }
        .map_err(tx::mongo)?;
        if result
            .get_array("writeErrors")
            .is_ok_and(|errors| !errors.is_empty())
            || result.contains_key("writeConcernError")
        {
            return Err(tx::ErrorKind::Command(result));
        }
        Ok(())
    }
}
