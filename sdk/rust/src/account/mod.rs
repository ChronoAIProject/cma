//! Explicit irreversible owner account deletion and safe progress reads.

use std::collections::BTreeMap;

use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

use crate::{Client, IdempotencyKey, Result};

string_enum! {
    /// Durable state of the account deletion request.
    pub enum AccountDeletionStatus {
        Running => "running", Blocked => "blocked", Completed => "completed"
    }
}

string_enum! {
    /// Ordered phase in the account erasure state machine.
    pub enum ErasureStep {
        Inventory => "inventory", RetireSandboxes => "retire_sandboxes",
        ReleaseHomes => "release_homes", EraseRecords => "erase_records",
        EraseObjects => "erase_objects", RevokeCredentials => "revoke_credentials",
        VerifyNoResidue => "verify_no_residue"
    }
}

/// Recorded completion evidence for one erasure phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeletionStepProgress {
    /// Exact phase that completed.
    pub step: ErasureStep,
    /// Server-observed completion timestamp.
    pub completed_at: String,
    /// Nonnegative counts grouped by the server's evidence classes.
    pub counts: BTreeMap<String, u64>,
}

/// Durable acceptance or progress receipt, separate from completed erasure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountDeletionReceipt {
    /// Stable deletion operation identity.
    pub id: String,
    /// Current durable deletion state.
    pub status: AccountDeletionStatus,
    /// Current ordered erasure phase.
    pub step: ErasureStep,
    /// Server-observed acceptance timestamp.
    pub requested_at: String,
    /// Maximum completion deadline, seven days after acceptance.
    pub deadline_at: String,
    /// Successful completion timestamp; absent while erasure remains unfinished.
    pub completed_at: Option<String>,
    /// Recorded completed phases, empty in the initial acceptance release.
    pub steps: Vec<AccountDeletionStepProgress>,
}

/// Human owner's account operations.
pub struct Account<'a> {
    client: &'a Client,
}

/// Explicit owner-only deletion acceptance and progress operations.
pub struct AccountDeletion<'a> {
    client: &'a Client,
}

impl Client {
    /// Access owner account operations.
    pub fn account(&self) -> Account<'_> {
        Account { client: self }
    }
}

impl<'a> Account<'a> {
    /// Access irreversible account deletion and its durable receipt.
    pub fn deletion(&self) -> AccountDeletion<'a> {
        AccountDeletion {
            client: self.client,
        }
    }
}

impl AccountDeletion<'_> {
    /// Acknowledge irreversible CMA data deletion with the retained caller replay key.
    ///
    /// HTTP 202 acknowledges acceptance; it does not prove that erasure completed.
    pub async fn request(&self, key: &IdempotencyKey) -> Result<AccountDeletionReceipt> {
        self.client
            .command(
                Method::POST,
                &["account", "deletion"],
                Some(&serde_json::json!({"acknowledge":"irreversible"})),
                key,
                StatusCode::ACCEPTED,
            )
            .await
    }

    /// Read safe progress; a failure never implies that account erasure completed.
    pub async fn status(&self) -> Result<AccountDeletionReceipt> {
        self.client.get(&["account", "deletion"], &[]).await
    }
}
