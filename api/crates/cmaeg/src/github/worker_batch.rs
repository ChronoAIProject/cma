use super::*;

impl Gateway {
    pub(super) async fn build_batch_request(
        &self,
        owner: &str,
        group_id: &str,
        group: &group::ExecutionGroup,
        batch: &group::Batch,
    ) -> Result<crate::ingress::launch::LaunchRequest, Error> {
        let Some(ingress) = self.ingress.as_ref() else {
            return Err(Error::Unavailable);
        };
        let channel = ingress
            .resolver
            .resolve(&group.channel_id)
            .await
            .map_err(|_| Error::Unavailable)?;
        if channel.definition.owner.subject != owner {
            return Err(Error::Forbidden);
        }
        let first = batch.members.first().ok_or(Error::Invalid)?;
        let app = self.config.app(&group.app_ref).ok_or(Error::Forbidden)?;
        let mut bindings = BTreeMap::new();
        for member in &batch.members {
            let binding = match bindings.entry(member.installation_id) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let binding_id =
                        super::super::identity("binding", &(app.app_id, member.installation_id));
                    let (stored_owner, binding) = self
                        .store
                        .read::<super::super::Binding>(&binding_id, "binding")
                        .await?;
                    if stored_owner != owner
                        || binding.status != "active"
                        || binding.verified_until <= super::super::now()
                    {
                        return Err(Error::Forbidden);
                    }
                    entry.insert(binding)
                }
            };
            if !binding_allows(binding, member, super::super::now()) {
                return Err(Error::Forbidden);
            }
            let source = SourceKind::Github {
                app_ref: group.app_ref.clone(),
                installation_id: member.installation_id,
                repository_id: member.repository_id,
            };
            let Some(configured) = channel.definition.source_of(&source) else {
                return Err(Error::Forbidden);
            };
            if !configured.github_accepts(&member.event, member.action.as_deref()) {
                return Err(Error::Forbidden);
            }
        }
        let source = SourceKind::Github {
            app_ref: group.app_ref.clone(),
            installation_id: first.installation_id,
            repository_id: first.repository_id,
        };
        let data = json!({
            "batch_id": batch.batch_id,
            "group_id": group_id,
            "events": batch.members.iter().map(|member| json!({
                "delivery_id": member.delivery_id,
                "event": member.event,
                "action": member.action,
                "installation_id": member.installation_id,
                "repository_id": member.repository_id,
                "payload": member.payload,
            })).collect::<Vec<_>>(),
        });
        let occurred_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Error::Unavailable)?;
        let subject = format!("github-execution-group:{group_id}");
        let ce = CloudEvent {
            specversion: "1.0".into(),
            id: batch.batch_id.clone(),
            source: format!("github://app/{}", group.app_ref),
            r#type: "com.github.batch".into(),
            time: Some(occurred_at.clone()),
            subject: Some(subject.clone()),
            datacontenttype: Some("application/json".into()),
            dataschema: None,
            data: Some(data),
            extensions: std::collections::BTreeMap::new(),
        };
        let activity = Activity {
            thread: None,
            schema_version: 1,
            event_id: EventId::new(batch.batch_id.clone()).map_err(|_| Error::Invalid)?,
            kind: ActivityKind::Event {
                ce,
                attachments: Vec::new(),
            },
            actor: Actor {
                id: format!("github:execution-group:{group_id}"),
                display_name: None,
                kind: ActorKind::System,
            },
            conversation: ActivityConversation {
                id: subject,
                kind: ConversationKind::Custom,
                thread_id: None,
            },
            occurred_at,
            source,
            routing: None,
        };
        activity.validate().map_err(|_| Error::Invalid)?;
        let partition = activity.partition_key(&channel.definition.partition);
        let conversation_id = ConversationId::derive(
            &ingress.master_keys,
            &channel.id,
            channel.definition.partition.version,
            &partition,
        );
        let event_id = activity.event_id.clone();
        let current = super::super::now();
        let event_ref = EventRef::seal(
            &EventRefPayload {
                telegram_admission: None,
                v: 1,
                activity_sha256: Some(activity_digest(&activity)),
                channel_id: channel.id.clone(),
                record_id: channel.pointer.record_id.clone(),
                event_id: event_id.clone(),
                partition_key: cmaeg_core::ids::compact_partition_key(&partition),
                conversation_id: conversation_id.clone(),
                expires_at: current.saturating_add(RETENTION_SECONDS),
                reply: ReplyAddress::None,
                consumer: None,
                lineage: None,
            },
            &ingress.master_keys,
        )
        .map_err(|_| Error::Unavailable)?;
        Ok(crate::ingress::launch::LaunchRequest {
            channel,
            activity,
            conversation_id,
            event_ref,
            idempotency_key: IdempotencyKey::from_event(&event_id),
            received_at: Instant::now(),
            trace: tracing::Span::current(),
            timing: None,
            consumer: None,
        })
    }
}
