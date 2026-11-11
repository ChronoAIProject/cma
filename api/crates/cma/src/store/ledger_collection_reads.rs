//! Every branch has an explicit complete index; missing indexes refuse.
use std::time::Duration;

use layer2_core::sandbox::{Workspace, Worktree};
use layer3_adapter::runtime_ledger_collection::{CollectionRequest, MAX_COLLECTION_DEPENDENCIES};
use mongodb::{
    bson::{doc, Document},
    options::Hint,
    ClientSession, Database,
};
use sha2::{Digest, Sha256};

use super::{
    CollectionReadRefusal::{AuthorityUnavailable, BudgetExceeded, UnknownEligibility},
    Result,
};

const MAX_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct ReadBudget<'a> {
    db: &'a Database,
    session: &'a mut ClientSession,
    digest: Sha256,
    rows: usize,
    bytes: usize,
}

impl<'a> ReadBudget<'a> {
    pub(super) fn new(
        db: &'a Database,
        session: &'a mut ClientSession,
        request: &CollectionRequest,
    ) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"chrono.runtime.ledger.collection.authority.v1\0");
        digest.update(request.fingerprint().expect("validated request"));
        Self {
            db,
            session,
            digest,
            rows: 0,
            bytes: 0,
        }
    }

    pub(super) fn finish(self) -> (String, u64, u64) {
        (
            format!("{:x}", self.digest.finalize()),
            self.rows as u64,
            self.bytes as u64,
        )
    }

    pub(super) async fn one(&mut self, collection: &str, id: &str) -> Result<Document> {
        let mut rows = self
            .query(collection, doc! {"_id": id}, doc! {"_id": 1})
            .await?;
        if rows.len() != 1 {
            return Err(UnknownEligibility);
        }
        Ok(rows.remove(0))
    }

    pub(super) async fn named_worktree(
        &mut self,
        sandbox: &str,
        workspace: &str,
        name: &str,
    ) -> Result<(Workspace, Worktree)> {
        let mut spaces = self
            .query(
                "workspaces",
                doc! {"sandbox_id":sandbox,"name":workspace},
                doc! {"sandbox_id":1,"name":1,"_id":1},
            )
            .await?;
        spaces.extend(
            self.query(
                "workspaces",
                doc! {"record.sandbox_id":sandbox,"record.name":workspace},
                doc! {"record.sandbox_id":1,"record.name":1,"_id":1},
            )
            .await?,
        );
        let document = unique(spaces)?;
        let workspace =
            crate::store::workspaces::read_workspace(&document).map_err(|_| UnknownEligibility)?;
        let mut trees = self
            .query(
                "worktrees",
                doc! {"workspace_id":workspace.id.as_str(),"name":name},
                doc! {"workspace_id":1,"name":1},
            )
            .await?;
        trees.extend(
            self.query(
                "worktrees",
                doc! {"record.workspace_id":workspace.id.as_str(),"record.name":name},
                doc! {"record.workspace_id":1,"record.name":1,"_id":1},
            )
            .await?,
        );
        let document = unique(trees)?;
        let worktree =
            crate::store::worktrees::read_worktree(&document).map_err(|_| UnknownEligibility)?;
        Ok((workspace, worktree))
    }

    pub(super) async fn agents_for_worktree(&mut self, worktree: &str) -> Result<Vec<Document>> {
        let mut rows = self
            .query(
                "agents",
                doc! {"worktree_id":worktree},
                doc! {"worktree_id":1,"_id":1},
            )
            .await?;
        rows.extend(
            self.query(
                "agents",
                doc! {"record.worktree_id":worktree},
                doc! {"record.worktree_id":1,"_id":1},
            )
            .await?,
        );
        Ok(rows)
    }

    async fn query(
        &mut self,
        collection: &str,
        filter: Document,
        index: Document,
    ) -> Result<Vec<Document>> {
        let remaining = MAX_COLLECTION_DEPENDENCIES
            .checked_sub(self.rows)
            .ok_or(BudgetExceeded)?;
        let mut cursor = self
            .db
            .collection::<Document>(collection)
            .find(filter)
            .hint(Hint::Keys(index))
            .limit((remaining + 1) as i64)
            .max_time(Duration::from_secs(2))
            .sort(doc! {"_id":1})
            .session(&mut *self.session)
            .await
            .map_err(|_| AuthorityUnavailable)?;
        let mut rows = Vec::new();
        while cursor
            .advance(&mut *self.session)
            .await
            .map_err(|_| AuthorityUnavailable)?
        {
            // Limit+1 proves completeness; excess rows never become truncation.
            if self.rows == MAX_COLLECTION_DEPENDENCIES {
                return Err(BudgetExceeded);
            }
            let document: Document = cursor
                .deserialize_current()
                .map_err(|_| UnknownEligibility)?;
            let bytes =
                mongodb::bson::serialize_to_vec(&document).map_err(|_| UnknownEligibility)?;
            self.bytes = self.bytes.checked_add(bytes.len()).ok_or(BudgetExceeded)?;
            if self.bytes > MAX_BYTES {
                return Err(BudgetExceeded);
            }
            self.rows += 1;
            self.digest.update((collection.len() as u64).to_be_bytes());
            self.digest.update(collection.as_bytes());
            self.digest.update((bytes.len() as u64).to_be_bytes());
            self.digest.update(bytes);
            rows.push(document);
        }
        Ok(rows)
    }
}

fn unique(rows: Vec<Document>) -> Result<Document> {
    let first = rows.first().ok_or(UnknownEligibility)?;
    if rows.iter().any(|row| row != first) {
        return Err(UnknownEligibility);
    }
    Ok(first.clone())
}
