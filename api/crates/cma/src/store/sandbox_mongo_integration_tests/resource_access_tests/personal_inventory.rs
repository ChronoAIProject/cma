//! Public personal Agent-list parity, release fences and complete Mongo budgets.
use super::*;
use layer2_core::sandbox::{AgentListQuery, AgentReadService};

#[path = "personal_inventory/contracts.rs"]
mod contracts;
#[path = "personal_inventory/fixture.rs"]
mod fixture;
#[path = "personal_inventory/identity.rs"]
mod identity;
#[path = "personal_inventory/meter.rs"]
mod meter;
#[path = "personal_inventory/public.rs"]
mod public;

#[path = "personal_inventory/fences.rs"]
mod fences;

#[path = "personal_inventory/failures.rs"]
mod failures;

#[path = "personal_inventory/provenance.rs"]
mod provenance;

#[path = "personal_inventory/concurrency.rs"]
mod concurrency;

#[path = "personal_inventory/shares.rs"]
mod shares;

#[path = "personal_inventory/profile.rs"]
mod profile;

#[path = "personal_inventory/availability.rs"]
mod availability;

#[path = "personal_inventory/organization.rs"]
mod organization;

#[path = "personal_inventory/list_query_budgets.rs"]
mod list_query_budgets;
