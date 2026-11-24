use super::*;
use crate::records::mongo::MongoRecords;

pub(super) fn record_client(
    backend: &fake_cma::FakeCma,
    database: Option<MongoRecords>,
) -> RecordClient {
    let records = RecordClient::new(&backend.base_url, "cma-service").unwrap();
    match database {
        Some(database) => records.with_database(database),
        None => records.with_thread_fences_fixture(),
    }
}

impl App {
    pub(in crate::tools::threads) async fn with_mongo(database: MongoRecords) -> Self {
        Self::with_records(11, false, Some(database)).await
    }

    pub(super) async fn seed_initial_record(&self) {
        let records = self.state.resolver.record_client();
        if records.database().is_none() {
            self.save();
            return;
        }
        records
            .put(
                &self.definition.record_id,
                &self.definition.owner.subject,
                &seal_blob(
                    &self.state.keys,
                    &serde_json::to_vec(&self.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                None,
            )
            .await
            .unwrap();
    }

    pub(in crate::tools::threads) async fn restart_on_mongo(&mut self, database: MongoRecords) {
        let gateway = self.servers.pop().unwrap();
        gateway.abort();
        assert!(gateway.await.unwrap_err().is_cancelled());
        let records = record_client(&self.backend, Some(database));
        self.state.resolver = ChannelResolver::new(
            Arc::new(records),
            self.state.keys.clone(),
            Default::default(),
        );
        self.state.discord_threads = Some(Arc::new(DiscordThreads::new(DiscordRest::fixture(
            &format!("{}/api/v10/", self.platform_base),
        ))));
        self.mount().await;
    }
}
