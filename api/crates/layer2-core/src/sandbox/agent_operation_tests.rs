mod admission_fence_tests;
mod command_authorization_tests;
mod command_driver_selection_tests;
mod command_driver_tests;
mod command_evidence_tests;
mod command_tests;
mod command_work_tests;
mod first_message_tests;
mod fixtures;
mod ledger_tests;
mod linear_input_tests;
mod live_skill_tests;
mod managed_context_tests;
mod model_credit_admission_tests;
mod observation_tests;
mod preferences_tests;
mod queued_message_tests;
mod retired_prompt_tests;
mod settings_tests;
mod skill_contention_tests;
mod skill_upgrade_tests;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use layer3_adapter::{ConversationInput, PermissionDecision};

use crate::sandbox::store::{AgentConversationStore, AgentLogStore, AgentStore};
use crate::{RuntimeCapabilities, RuntimeCapability};

use super::*;
use fixtures::*;

mod approval_delivery_cases;
mod input_boundary_cases;
mod message_admission_cases;
mod turn_control_cases;
mod turn_timeout_tests;
