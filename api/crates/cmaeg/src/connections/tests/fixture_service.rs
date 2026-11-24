//! Shared management setup for HTTP and authenticated Mongo fixtures.
use super::*;
impl Fixture {
    pub(in crate::connections) fn service(&self) -> ConnectionService {
        self.service_with_console_origin(None)
    }
    pub(in crate::connections) fn service_with_console_origin(
        &self,
        console_public_url: Option<String>,
    ) -> ConnectionService {
        self.service_with_records(console_public_url, self.records())
    }
    pub(in crate::connections) async fn independent_service(
        &self,
        native: &crate::replicas::mongo_fixture::Fixture,
    ) -> ConnectionService {
        self.service_with_records(None, native.records(&self.base).await)
    }
    fn service_with_records(
        &self,
        console_public_url: Option<String>,
        records: RecordClient,
    ) -> ConnectionService {
        let providers = Arc::new(
            ProviderClient::new(
                ProviderRegistry::parse("[]").unwrap(),
                Duration::from_secs(1),
                Duration::from_secs(1),
                2,
            )
            .unwrap(),
        );
        let mut management = ManagementState::new(
            providers.clone(),
            Arc::new(records),
            Arc::new(CreatorAuthenticator::new(&self.base).unwrap()),
            ManagementConfig {
                console_public_url,
                public_url: "https://gateway.example".into(),
                public_api_url: "https://gateway.example".into(),
                public_api_aliases: vec![],
                advertise_public_api_url: true,
                master_keys: self.keys.clone(),
                cma_base_url: self.base.clone(),
                cma_service_token: Secret::new(SERVICE_TOKEN),
                nyxid_base_url: self.base.clone(),
                tools_nyxid_slug: "cmaeg".into(),
            },
            Default::default(),
        );
        management.managed_telegram_setup_enabled = true;
        let (delivery, _receiver) = tokio::sync::mpsc::channel(1);
        let tools = ToolsState {
            discord_thread_history: None,
            telegram_topics: None,
            discord_threads: None,
            native_lark_conversation_history: None,
            discord_conversation_history: None,
            complete_replies_enabled: false,
            commonmark_relay: Default::default(),
            resolver: management.resolver.clone(),
            keys: self.keys.clone(),
            providers,
            delivery: Arc::new(crate::delivery::router::ShardedScheduler::new(delivery)),
            routed_replies: None,
            tracker: TurnTracker::new(Readiness::initialized()),
            public_origin: management.public_url.clone(),
            public_api_url: management.public_url.clone(),
            public_api_aliases: vec![],
            advertise_public_api_url: true,
            whatsapp_templates_enabled: false,
            discord_interactions_enabled: false,
            native_discord_media_controls_enabled: false,
            native_discord_media_card_edits_enabled: false,
            native_discord_media_card_replacements_enabled: false,
            native_discord_media_card_sets_enabled: false,
            whatsapp_interactions_enabled: false,
            native_whatsapp_interactions_enabled: false,
            native_whatsapp_media_controls_enabled: false,
            consumer_interactions_enabled: false,
            telegram_effects_issuance_enabled: false,
            native_telegram_media_controls_enabled: false,
            native_telegram_media_control_edits_enabled: false,
            native_media_edits: None,
            native_whatsapp_forms: None,
            native_typing: None,
            native_lark_effects: None,
            lark_effects_issuance_enabled: false,
            lark_effects: None,
            telegram_effects: None,
            native_media_enabled: false,
            native_lark_outbound_media_enabled: false,
            native_lark_sticker_replies_enabled: false,
            native_lark_media_controls_enabled: false,
            native_lark_multi_image_controls_enabled: false,
            native_lark_media_card_replacements_enabled: false,
            native_media_presentations_enabled: false,
            native_discord_voice_enabled: false,
            native_telegram_structured_replies_enabled: false,
            native_whatsapp_structured_replies_enabled: false,
            native_telegram_albums_enabled: false,
            native_lark_thread_start_enabled: false,
            native_whatsapp_outbound_media_enabled: false,
            whatsapp_reactions: None,
            native_whatsapp_templates_enabled: false,
            native_whatsapp_groups_enabled: false,
            native_discord_edits_enabled: false,
            discord_effects_issuance_enabled: false,
            discord_effects: None,
            relay_text_edits_enabled: false,
            lark_cards_enabled: false,
            media: None,
            downloads: Default::default(),
        };
        ConnectionService::new(management, tools)
    }
    pub(in crate::connections) async fn retain_channel(
        &self,
        definition: &cmaeg_core::channel::ChannelDefinition,
    ) -> cmaeg_core::ids::ChannelId {
        let pointer = cmaeg_core::seal::ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: self
                .records()
                .record_url(definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        if self.records_override.is_some() {
            self.records()
                .put(
                    &definition.record_id,
                    &definition.owner.subject,
                    &seal_blob(&self.keys, definition.to_blob_json().to_string().as_bytes())
                        .unwrap(),
                    None,
                )
                .await
                .unwrap();
            return pointer.seal_id(&self.keys).unwrap();
        }
        self.peer.channels.lock().unwrap().insert(
            definition.record_id.to_string(),
            Retained {
                owner: definition.owner.subject.clone(),
                version: 1,
                blob: STANDARD.encode(
                    seal_blob(&self.keys, definition.to_blob_json().to_string().as_bytes())
                        .unwrap(),
                ),
            },
        );
        pointer.seal_id(&self.keys).unwrap()
    }
    pub(in crate::connections) async fn forget_channel(&self, record_id: &str) {
        if self.records_override.is_some() {
            let records = self.records();
            let record = records
                .get(records.record_url(record_id).unwrap().as_str())
                .await
                .unwrap();
            records
                .delete(&record_id.parse().unwrap(), record.version)
                .await
                .unwrap();
            return;
        }
        self.peer.channels.lock().unwrap().remove(record_id);
    }
}
