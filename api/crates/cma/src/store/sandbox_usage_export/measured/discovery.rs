use super::*;

impl CaptureStore {
    pub(crate) async fn discover_measured(&self) -> Result<(), SandboxStoreError> {
        let now = super::lease::server_now(self).await?;
        transaction::run(self, (self, now), |&(store, now), session| {
            Box::pin(async move {
                use TransactionEventError::{Domain, Driver};
                let key = keys::prefix("authority", &store.config.namespace);
                let saved = store
                    .collection(INVENTORY)
                    .find_one(doc! {"_id":keys::id(&key)})
                    .session(&mut *session)
                    .await
                    .map_err(Driver)?;
                let mut authority = if let Some(row) = &saved {
                    decode::<Authority>(row).map_err(Domain)?
                } else {
                    Authority {
                        namespace: store.config.namespace.clone(),
                        owner_issuer: store.config.owner_issuer.clone(),
                        capture_start: now.checked_add(59).ok_or_else(|| Domain(corrupt()))? / 60
                            * 60,
                        after: None,
                    }
                };
                if authority.namespace != store.config.namespace
                    || authority.owner_issuer != store.config.owner_issuer
                    || !authority.capture_start.is_multiple_of(60)
                {
                    return Err(Domain(corrupt()));
                }
                let mut filter = doc! {"namespace":&authority.namespace};
                if let Some(after) = &authority.after {
                    filter.insert("_id", doc! {"$gt":after});
                }
                let mut cursor = store
                    .collection(STREAMS)
                    .find(filter)
                    .sort(doc! {"_id":1})
                    .limit(32)
                    .session(&mut *session)
                    .await
                    .map_err(Driver)?;
                let mut count = 0;
                while let Some(row) = cursor
                    .next(&mut *session)
                    .await
                    .transpose()
                    .map_err(Driver)?
                {
                    let source = Stream::read(&row).map_err(Domain)?;
                    if source.owner_issuer != authority.owner_issuer {
                        return Err(Domain(corrupt()));
                    }
                    let key = keys::stream(&authority.namespace, &source.id);
                    if store
                        .collection(INVENTORY)
                        .find_one(doc! {"_id":keys::id(&key)})
                        .session(&mut *session)
                        .await
                        .map_err(Driver)?
                        .is_none()
                    {
                        let start = authority.capture_start.max(
                            source
                                .capture_start
                                .checked_add(59)
                                .ok_or_else(|| Domain(corrupt()))?
                                / 60
                                * 60,
                        );
                        let stream = MeasuredStream {
                            id: uuid::Uuid::new_v4().to_string(),
                            source_stream: source.id.clone(),
                            source_epoch: source.source_epoch,
                            namespace: source.namespace,
                            owner: source.owner,
                            owner_issuer: source.owner_issuer,
                            sandbox_id: source.sandbox_id,
                            capture_start: start,
                            tracking_started_at: source.tracking_started_at,
                            next_minute: start,
                            replay_minute: start,
                            sequence: 0,
                            previous_digest: None,
                            replay_turn: false,
                        };
                        store
                            .collection(INVENTORY)
                            .insert_one(stream.document().map_err(Domain)?)
                            .session(&mut *session)
                            .await
                            .map_err(Driver)?;
                        let delivery_key = format!(
                            "{}{}",
                            keys::prefix("delivery", &stream.namespace),
                            stream.id
                        );
                        store
                            .collection(INVENTORY)
                            .insert_one(
                                doc! {"_id":keys::id(&delivery_key),"document_schema_version":1_i64,
                                "stream_id":&stream.id,"source_stream":&stream.source_stream},
                            )
                            .session(&mut *session)
                            .await
                            .map_err(Driver)?;
                    }
                    authority.after = Some(source.id);
                    count += 1;
                }
                if count < 32 {
                    authority.after = None;
                }
                store
                    .collection(INVENTORY)
                    .replace_one(
                        doc! {"_id":keys::id(&key)},
                        document(&key, &authority).map_err(Domain)?,
                    )
                    .upsert(true)
                    .session(session)
                    .await
                    .map_err(Driver)?;
                Ok(())
            })
        })
        .await
    }

    pub(crate) async fn next_measured_page(&self) -> Result<Vec<String>, SandboxStoreError> {
        transaction::run(self, self, |store, session| {
            Box::pin(async move {
                use TransactionEventError::{Domain, Driver};
                let key = keys::prefix("schedule", &store.config.namespace);
                let row = store
                    .collection(INVENTORY)
                    .find_one(doc! {"_id":keys::id(&key)})
                    .session(&mut *session)
                    .await
                    .map_err(Driver)?;
                let after = row.as_ref().and_then(|r| r.get_str("after").ok());
                let prefix = keys::prefix("stream", &store.config.namespace);
                let mut cursor = store
                    .collection(INVENTORY)
                    .find(keys::page(&prefix, after).map_err(Domain)?)
                    .sort(doc! {"_id":1})
                    .limit(16)
                    .session(&mut *session)
                    .await
                    .map_err(Driver)?;
                let mut result = Vec::new();
                while let Some(row) = cursor
                    .next(&mut *session)
                    .await
                    .transpose()
                    .map_err(Driver)?
                {
                    result.push(MeasuredStream::read(&row).map_err(Domain)?.key());
                }
                store
                    .collection(INVENTORY)
                    .replace_one(
                        doc! {"_id":keys::id(&key)},
                        doc! {"_id":keys::id(&key),"document_schema_version":1_i64,"after":result.last()},
                    )
                    .upsert(true)
                    .session(session)
                    .await
                    .map_err(Driver)?;
                Ok(result)
            })
        })
        .await
    }
}
