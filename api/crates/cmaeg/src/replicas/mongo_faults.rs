//! Deliberate record inconsistencies for the real-Mongo discovery tests.
use super::Node;
use crate::replicas::{
    partition_model::{item_id, Head, Item},
    store::Write,
};

impl Node {
    pub async fn relocate_head(&self, source: &str, target: &str, retention: Option<i64>) {
        let store = &self.partitions.store;
        let (mut head, version) = store
            .read::<Head>(source, retention.is_some())
            .await
            .unwrap()
            .unwrap();
        let key = item_id(head.queue.front().unwrap());
        let (mut item, item_version) = store.read::<Item>(&key, true).await.unwrap().unwrap();
        // Keep item.partition consistent with the forged head, so only the exact
        // plan-derived ID check can reject this evidence.
        item.partition = target.into();
        store
            .write(
                &key,
                &head.owner,
                &item,
                Write {
                    expected: Some(item_version),
                    retained_until: Some(item.retained_until),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        store
            .write(
                source,
                &head.owner,
                &head,
                Write {
                    expected: Some(version),
                    retained_until: retention,
                    projection: Some(self.partitions.projection(false)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        head.id = target.into();
        store
            .write(
                target,
                &head.owner,
                &head,
                Write {
                    retained_until: retention,
                    projection: Some(self.partitions.projection(true)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    pub async fn eager_empty_lane_placement(&self) {
        // The previous page called this even when the lane key set was empty.
        self.partitions
            .membership
            .lane_assignments(&self.partitions.store, [])
            .await
            .unwrap();
    }
}

impl Node {
    pub async fn permanent_head_binding(&self, id: &str) -> crate::replicas::unresolved::Binding {
        let (head, version) = self
            .partitions
            .store
            .read::<Head>(id, false)
            .await
            .unwrap()
            .unwrap();
        crate::replicas::unresolved::Binding {
            partition: id.into(),
            head_work: head.queue.front().cloned().unwrap(),
            head_version: version,
        }
    }

    pub async fn corrupt_lane_evidence(&self, work: &str, fault: &str) {
        let store = &self.partitions.store;
        let key = item_id(work);
        let (mut item, version) = store.read::<Item>(&key, true).await.unwrap().unwrap();
        let remove = match fault {
            "missing_item" => Some(key.clone()),
            "missing_plan" => Some(item.identity.admission_id()),
            "missing_time" => Some(crate::replicas::delayed_lane::time_id(work)),
            "store" => None,
            _ => None,
        };
        if let Some(key) = remove {
            let (_, version) = store
                .read::<serde_json::Value>(&key, true)
                .await
                .unwrap()
                .unwrap();
            store
                .records
                .clone()
                .interactions()
                .delete(&key.parse().unwrap(), version)
                .await
                .unwrap();
        } else {
            match fault {
                "store" => {
                    let (head, _) = store
                        .read::<Head>(&item.partition, true)
                        .await
                        .unwrap()
                        .unwrap();
                    store
                        .write(
                            &item.partition,
                            &item.identity.owner,
                            &head,
                            Write {
                                projection: Some(self.partitions.projection(false)),
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap();
                }
                "kind" => item.kind.push('x'),
                "work" => item.work_id = uuid::Uuid::new_v4().to_string(),
                "effect" => item.effect = false,
                "retention" => item.retained_until -= 1,
                "hash" => item.request_hash.push('x'),
                _ => panic!("unknown fixture fault"),
            }
            store
                .write(
                    &key,
                    &item.identity.owner,
                    &item,
                    Write {
                        expected: Some(version),
                        retained_until: Some(item.retained_until),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
    }
}
