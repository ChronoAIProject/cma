//! Exact singleton context for a source inventory, not restore authority.
use super::{field, invalid, Connection, Result, RuntimeConfig, Sha256};
use sha2::Digest;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::mutation_ledger) struct Context {
    pub(super) store: Vec<u8>,
    pub(super) generation: Vec<u8>,
    sandbox: String,
    home_identity: String,
    roots: [String; 3],
    admission: String,
    // revision, reserved, operations, authority, terminal, in schema order.
    pub(super) accounting: [i64; 5],
    // page_size, page_count, freelist_count, max_page_count.
    geometry: [i64; 4],
}

impl Context {
    pub(super) fn read(db: &Connection, config: &RuntimeConfig) -> Result<Self> {
        let bytes: i64 = db
            .query_row(
                "SELECT length(store_id)+length(generation)+length(CAST(sandbox AS BLOB))+
             length(CAST(home AS BLOB))+length(CAST(state_root AS BLOB))+
             length(CAST(home_root AS BLOB))+length(CAST(transfer_root AS BLOB))+
             length(CAST(admission AS BLOB)) FROM metadata WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| invalid())?;
        if !(0..=200_000).contains(&bytes) {
            return Err(invalid());
        }
        let mut context = db
            .query_row(
                "SELECT store_id,generation,sandbox,home,state_root,home_root,transfer_root,
             admission,revision,reserved,operations,authority,terminal
             FROM metadata,accounting WHERE metadata.singleton=1 AND accounting.singleton=1",
                [],
                |r| {
                    Ok(Self {
                        store: r.get(0)?,
                        generation: r.get(1)?,
                        sandbox: r.get(2)?,
                        home_identity: r.get(3)?,
                        roots: [r.get(4)?, r.get(5)?, r.get(6)?],
                        admission: r.get(7)?,
                        accounting: [r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?],
                        geometry: [0; 4],
                    })
                },
            )
            .map_err(|_| invalid())?;
        for (index, name) in [
            "page_size",
            "page_count",
            "freelist_count",
            "max_page_count",
        ]
        .into_iter()
        .enumerate()
        {
            context.geometry[index] = db
                .pragma_query_value(None, name, |r| r.get(0))
                .map_err(|_| invalid())?;
        }
        let version: i64 = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|_| invalid())?;
        let application: i64 = db
            .pragma_query_value(None, "application_id", |r| r.get(0))
            .map_err(|_| invalid())?;
        if version != 2
            || application != 0x434d4132
            || context.store.len() != 16
            || super::fence::decode(&context.generation)? != config.generation
            || context.sandbox != config.sandbox_id
            || context.home_identity != config.home_identity.clone().unwrap_or_default()
            || context.admission != "fresh"
            || context
                .roots
                .iter()
                .zip([&config.state_root, &config.home, &config.checkpoint_root])
                .any(|(raw, expected)| raw.len() > 65536 || std::path::Path::new(raw) != expected)
            || context.accounting.iter().any(|n| *n < 0)
            || context.accounting[4] > context.accounting[2]
            || context.geometry[0] != 4096
            || context.geometry[1] < 1
            || context.geometry[2] < 0
            || context.geometry[2] > context.geometry[1]
            || context.geometry[3] < context.geometry[1]
        {
            return Err(invalid());
        }
        Ok(context)
    }

    pub(super) fn bytes(&self) -> usize {
        self.store.len()
            + self.generation.len()
            + self.sandbox.len()
            + self.home_identity.len()
            + self.roots.iter().map(String::len).sum::<usize>()
            + self.admission.len()
            + 9 * 8
    }

    pub(in crate::mutation_ledger) fn hash_into(&self, digest: &mut Sha256) {
        digest.update(b"cma-v2-retirement-inventory-context-v1\0");
        for value in [
            self.store.as_slice(),
            &self.generation,
            self.sandbox.as_bytes(),
            self.home_identity.as_bytes(),
            self.roots[0].as_bytes(),
            self.roots[1].as_bytes(),
            self.roots[2].as_bytes(),
            self.admission.as_bytes(),
        ] {
            field(digest, Some(value));
        }
        for value in self.accounting.iter().chain(&self.geometry) {
            digest.update(value.to_be_bytes());
        }
    }
}
