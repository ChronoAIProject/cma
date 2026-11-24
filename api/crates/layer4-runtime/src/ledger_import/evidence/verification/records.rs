use serde::de::{Error, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::schema::{decimal, digest};
use crate::ledger_import::inventory::Kind;
use crate::ledger_import::Limits;
use crate::mutation_authority::Disposition;
use crate::mutation_journal::Status;

pub(super) struct Records(pub(super) usize);

impl<'de> Deserialize<'de> for Records {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Rows;
        impl<'de> Visitor<'de> for Rows {
            type Value = Records;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("bounded V1 observations")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut rows: A,
            ) -> std::result::Result<Records, A::Error> {
                let mut count = 0;
                while let Some(record) = rows.next_element::<Record>()? {
                    if count >= Limits::default().entries || !record.valid(count) {
                        return Err(A::Error::custom("invalid V1 observation"));
                    }
                    count += 1;
                }
                Ok(Records(count))
            }
        }
        d.deserialize_seq(Rows)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    source_index: usize,
    source_sha256: String,
    source_bytes: u64,
    scope: Scope,
    kind: Kind,
    unpublished: bool,
    #[serde(deserialize_with = "required_fact")]
    fact: Option<Fact>,
}

fn required_fact<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<Fact>, D::Error> {
    Option::<Fact>::deserialize(d)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scope {
    kind: ScopeKind,
    #[serde(default, deserialize_with = "present")]
    generation: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScopeKind {
    LegacyGlobal,
    ProcessGeneration,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    #[serde(rename = "type")]
    kind: FactKind,
    operation: Operation,
    #[serde(default, deserialize_with = "present")]
    status: Option<Status>,
    #[serde(default, deserialize_with = "present")]
    resource_sha256: Option<String>,
    #[serde(default, deserialize_with = "present")]
    fence: Option<String>,
    #[serde(default, deserialize_with = "present")]
    disposition: Option<Disposition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FactKind {
    Mutation,
    Authority,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl Fact {
    fn valid(&self, kind: Kind, unpublished: bool) -> bool {
        self.operation.valid()
            && match (&self.kind, kind, unpublished) {
                (FactKind::Mutation, Kind::Journal, _) => {
                    self.status.is_some()
                        && self.resource_sha256.is_none()
                        && self.fence.is_none()
                        && self.disposition.is_none()
                }
                (FactKind::Authority, Kind::Authority, false) => {
                    self.status.is_none()
                        && self.resource_sha256.as_deref().is_some_and(digest)
                        && self.fence.as_deref().is_some_and(decimal)
                        && self.disposition.is_some()
                }
                _ => false,
            }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
    subject_sha256: String,
}

impl Operation {
    fn valid(&self) -> bool {
        !self.operation_id.is_empty()
            && self.operation_id.len() <= 256
            && digest(&self.request_sha256)
            && digest(&self.payload_sha256)
            && digest(&self.subject_sha256)
    }
}

impl Record {
    fn valid(&self, index: usize) -> bool {
        self.source_index == index
            && digest(&self.source_sha256)
            && self.source_bytes <= Limits::default().bytes
            && match (&self.scope.kind, self.scope.generation.as_deref()) {
                (ScopeKind::LegacyGlobal, None) => true,
                (ScopeKind::ProcessGeneration, Some(generation)) => decimal(generation),
                _ => false,
            }
            && match (self.kind, &self.fact, self.unpublished) {
                (kind, Some(fact), unpublished) => fact.valid(kind, unpublished),
                (Kind::Authority, None, true) => true,
                (Kind::Workspace | Kind::Installation | Kind::Ownership, None, _) => true,
                _ => false,
            }
    }
}
