//! Expiring process membership chooses placement; resource journals grant authority.
use super::{
    config::{LEASE, RENEW},
    drain::Control,
    store::{Store, Write},
    Error,
};
use crate::{
    http::readiness::Readiness,
    records::mongo::{LeaseCondition, ReplicaKind, ReplicaProjection},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};
use tokio::{task::JoinHandle, time::Instant};

const PROTOCOL: u32 = 1;
const MAX_MEMBERS: usize = 128;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    kind: String,
    deployment: String,
    instance: String,
    origin: String,
    protocol: u32,
    until_ms: i64,
    draining: bool,
}

struct View {
    instances: Vec<String>,
    origins: BTreeMap<String, String>,
    until: Instant,
    server_millis: i64,
    sampled_at: Instant,
}

#[derive(Clone)]
pub(crate) struct Membership {
    pub instance: String,
    pub deployment: String,
    view: Arc<RwLock<View>>,
    control: Control,
}

pub(crate) struct Worker {
    task: JoinHandle<()>,
    control: Control,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.control.start();
        self.task.abort();
    }
}

impl Membership {
    pub async fn join(
        store: Store,
        origin: &str,
        readiness: Readiness,
        address: String,
        control: Control,
    ) -> Result<(Self, Worker), Error> {
        readiness.require_replica_membership();
        let deployment = format!("{:x}", Sha256::digest(origin.as_bytes()));
        let instance = format!("{:x}", Sha256::digest(uuid::Uuid::new_v4().as_bytes()));
        let id = uuid::Uuid::new_v4().to_string();
        let clock = store.clock().await?;
        let member = Member {
            kind: "cmaeg_replica_member_v1".into(),
            deployment: deployment.clone(),
            instance: instance.clone(),
            origin: super::config::validate_address(&address).map_err(|_| Error::Corrupt)?,
            protocol: PROTOCOL,
            until_ms: clock.millis + LEASE.as_millis() as i64,
            draining: false,
        };
        let version = write_member(&store, &id, &member, None, None).await?;
        let shared = Self {
            instance,
            deployment,
            view: Arc::new(RwLock::new(View {
                instances: Vec::new(),
                origins: BTreeMap::new(),
                until: clock.started,
                server_millis: clock.millis,
                sampled_at: clock.started,
            })),
            control: control.clone(),
        };
        shared
            .refresh(&store, clock.millis, clock.started + LEASE)
            .await?;
        readiness.replica_membership(true);
        let running = shared.clone();
        let task_control = control.clone();
        let task = tokio::spawn(async move {
            let mut member = member;
            let mut version = version;
            let mut deadline = clock.started + LEASE;
            loop {
                tokio::select! {
                    biased;
                    _ = task_control.requested() => break,
                    _ = tokio::time::sleep_until(deadline) => {
                        readiness.replica_membership(false);
                        task_control.start();
                        break;
                    },
                    _ = tokio::time::sleep(RENEW) => {}
                }
                if Instant::now() >= deadline {
                    readiness.replica_membership(false);
                    task_control.start();
                    break;
                }
                let renewed = async {
                    let clock = store.clock().await?;
                    let previous = member.until_ms;
                    member.until_ms = clock.millis + LEASE.as_millis() as i64;
                    let next =
                        write_member(&store, &id, &member, Some(version), Some(previous)).await?;
                    running
                        .refresh(&store, clock.millis, clock.started + LEASE)
                        .await?;
                    Ok::<_, Error>((next, clock.started + LEASE))
                }
                .await;
                match renewed {
                    Ok((next, until)) if Instant::now() < until => {
                        version = next;
                        deadline = until;
                        readiness.replica_membership(true);
                    }
                    _ => {
                        readiness.replica_membership(false);
                        tracing::warn!("replica membership renewal failed; withdrawing authority");
                        task_control.start();
                        break;
                    }
                }
            }
            readiness.replica_membership(false);
            running.view.write().unwrap().instances.clear();
            member.draining = true;
            if write_member(&store, &id, &member, Some(version), None)
                .await
                .is_err()
            {
                tracing::warn!("replica membership drain marker unavailable; expiry retained");
            }
        });
        Ok((shared, Worker { task, control }))
    }

    async fn refresh(&self, store: &Store, now: i64, deadline: Instant) -> Result<(), Error> {
        let rows = store
            .active::<Member>(ReplicaKind::Member, &self.deployment, None, MAX_MEMBERS)
            .await?;
        if rows.len() == MAX_MEMBERS {
            return Err(Error::Capacity);
        }
        let mut instances = BTreeSet::new();
        let mut origins = BTreeMap::new();
        let mut unique_origins = BTreeSet::new();
        for (_, member, _) in rows {
            if member.kind != "cmaeg_replica_member_v1"
                || member.deployment != self.deployment
                || member.protocol != PROTOCOL
                || member.instance.len() != 64
            {
                return Err(Error::Corrupt);
            }
            if !member.draining && member.until_ms > now {
                let origin =
                    super::config::validate_address(&member.origin).map_err(|_| Error::Corrupt)?;
                if !unique_origins.insert(origin.clone()) {
                    return Err(Error::Corrupt);
                }
                origins.insert(member.instance.clone(), origin);
                instances.insert(member.instance);
            }
        }
        if !instances.contains(&self.instance) {
            return Err(Error::Lost);
        }
        *self.view.write().unwrap() = View {
            instances: instances.into_iter().collect(),
            origins,
            until: deadline,
            server_millis: now,
            sampled_at: deadline - LEASE,
        };
        Ok(())
    }

    /// Estimate UTC from the last server sample plus monotonic elapsed time;
    /// this never grants authority and may continue through a graceful drain.
    pub fn now_seconds(&self) -> i64 {
        self.now_millis().div_euclid(1000)
    }

    pub fn now_millis(&self) -> i64 {
        let Ok(view) = self.view.read() else {
            return i64::MAX;
        };
        view.server_millis.saturating_add(
            view.sampled_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(i64::MAX),
        )
    }

    pub fn origin(&self, instance: &str) -> Result<String, Error> {
        let view = self.view.read().map_err(|_| Error::Lost)?;
        if Instant::now() >= view.until {
            return Err(Error::Lost);
        }
        view.origins.get(instance).cloned().ok_or(Error::Lost)
    }

    pub fn assignments(
        &self,
        keys: impl IntoIterator<Item = String>,
    ) -> Result<BTreeMap<String, String>, Error> {
        if self.control.is_draining() {
            return Err(Error::Lost);
        }
        let view = self.view.read().unwrap();
        if Instant::now() >= view.until || view.instances.is_empty() {
            return Err(Error::Lost);
        }
        balanced(keys, &view.instances)
    }
}

async fn write_member(
    store: &Store,
    id: &str,
    member: &Member,
    version: Option<u64>,
    previous_until: Option<i64>,
) -> Result<u64, Error> {
    store
        .write(
            id,
            &member.deployment,
            member,
            Write {
                expected: version,
                retained_until: Some(member.until_ms.div_euclid(1000) + 60),
                condition: previous_until.map(LeaseCondition::Live),
                projection: Some(ReplicaProjection {
                    kind: ReplicaKind::Member,
                    deployment: member.deployment.clone(),
                    active: !member.draining,
                }),
                ..Default::default()
            },
        )
        .await
}

/// Exact quotas avoid a small bot/partition population concentrating on one member.
/// Scores preserve a deterministic preference; epochs and close-before-release make
/// the resulting moves safe rather than treating the assignment as authorization.
fn balanced(
    keys: impl IntoIterator<Item = String>,
    members: &[String],
) -> Result<BTreeMap<String, String>, Error> {
    let keys: BTreeSet<_> = keys.into_iter().collect();
    if keys.len() > 4096 || members.is_empty() || members.len() > MAX_MEMBERS {
        return Err(Error::Capacity);
    }
    let mut capacity: Vec<_> = members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            (
                member,
                keys.len() / members.len() + usize::from(index < keys.len() % members.len()),
            )
        })
        .collect();
    let mut result = BTreeMap::new();
    for key in keys {
        let selected = capacity
            .iter()
            .enumerate()
            .filter(|(_, (_, remaining))| *remaining > 0)
            .max_by_key(|(_, (member, _))| {
                let mut hash = Sha256::new();
                hash.update(b"cmaeg-replica-placement-v1\0");
                hash.update(key.as_bytes());
                hash.update([0]);
                hash.update(member.as_bytes());
                <[u8; 32]>::from(hash.finalize())
            })
            .map(|(index, _)| index)
            .ok_or(Error::Capacity)?;
        capacity[selected].1 -= 1;
        result.insert(key, capacity[selected].0.clone());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placement_balances_arbitrary_members_and_input_order() {
        for replicas in [1, 2, 3, 4, 5, 17] {
            let members: Vec<_> = (0..replicas).map(|i| format!("member-{i:02}")).collect();
            for count in [0, 1, 2, 20, 100, 1000] {
                let keys: Vec<_> = (0..count).map(|i| format!("partition-{i}")).collect();
                let mapping = balanced(keys.clone(), &members).unwrap();
                assert_eq!(mapping, balanced(keys.into_iter().rev(), &members).unwrap());
                let sizes: Vec<_> = members
                    .iter()
                    .map(|member| mapping.values().filter(|owner| *owner == member).count())
                    .collect();
                assert!(sizes.iter().max().unwrap() - sizes.iter().min().unwrap() <= 1);
            }
        }
        assert!(balanced(["key".into()], &[]).is_err());
        assert!(balanced((0..4097).map(|i| i.to_string()), &["member".into()]).is_err());
    }
}
