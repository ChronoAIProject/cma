//! Explicit admitted socket and queue ports, independent of legacy flags.

use crate::ingress_budget::diagnostic::text;
#[cfg(test)]
use crate::ingress_budget::AdmittedSocket;
use crate::ingress_budget::{IngressWorkScope, SharedOwned};
use crate::{RuntimeOutputReceipt, RuntimeOutputSource};

#[path = "acp_ingress_typed.rs"]
pub(in crate::hosted) mod typed;

#[path = "acp_ingress_identity.rs"]
pub(in crate::hosted) mod identity;

#[path = "acp_ingress_components.rs"]
pub(super) mod components;

pub(in crate::hosted) type PreparedUpdate =
    Result<crate::RuntimeOutputUpdate, SharedOwned<crate::hosted::runtime_decode::Error>>;

impl crate::ingress_budget::IngressOwnershipPolicy {
    /// Compose conversation storage with the consumer/transport bounds. The
    /// attachment count is enforced by admit_attachment before transport
    /// startup; queued codec/source frames and other transports belong in
    /// additional_control under their separately enforced limits.
    pub fn conversation_bounds(
        total_bytes: usize,
        maximum_attachments: usize,
        additional_control: crate::ingress_budget::AllocationPlan,
        consumer_transformation: crate::ingress_budget::AllocationPlan,
    ) -> crate::Result<crate::ingress_budget::ClassBounds> {
        use crate::ingress_budget::{AllocationPlan, ClassBounds};
        if maximum_attachments == 0 {
            return Err(crate::AdapterError::Rejected(text(
                "attachment limit must be positive",
            )));
        }
        // One serial reader and one serial input worker can each build/send a
        // control reply while the independent ACK task owns a dequeued source.
        let reply = AllocationPlan::new(
            "control reply overlap",
            &[
                crate::hosted::permission_state::reply_plan()?.requested_bytes(),
                4097 + crate::ingress_budget::sizing::shared_owner_bytes::<Message>(),
            ],
        )?
        .requested_bytes();
        let diagnostics = crate::ingress_budget::diagnostic::scope_entry_plan()?.requested_bytes();
        let attachment = AllocationPlan::new(
            "attachment control",
            &[
                crate::RuntimeOutputReceipt::control_bound(128)?.requested_bytes(),
                2 * reply,
                1024,        // Input worker's validated conversation-id copy.
                diagnostics, // Serial reader can overlap the independent ACK task.
                diagnostics,
            ],
        )?
        .requested_bytes();
        let all = attachment.checked_mul(maximum_attachments).ok_or_else(|| {
            crate::AdapterError::Rejected(text("attachment control layout overflow"))
        })?;
        ClassBounds::new(
            total_bytes,
            AllocationPlan::new(
                "concurrent control",
                &[all, additional_control.requested_bytes()],
            )?,
            AllocationPlan::new(
                "largest complete transformation",
                &[typed::maximum_working_bytes()?.max(consumer_transformation.requested_bytes())],
            )?,
        )?
        .with_process_storage(components::storage_plan()?)?
        .with_attachment_limit(maximum_attachments)
    }
}

pub(in crate::hosted) fn valid_prompt_id(value: &str) -> bool {
    value.strip_prefix("cma-prompt-v1-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

impl super::Bridge {
    pub(in crate::hosted) async fn control_reply(
        &self,
        call: &serde_json::Value,
        build: impl FnOnce() -> serde_json::Value,
    ) -> crate::Result<()> {
        let storage = self
            .ingress
            .as_ref()
            .map(|policy| {
                if !crate::hosted::acp_correlation::valid_call_with_policy(call, true) {
                    return Err(crate::AdapterError::Rejected(text(
                        "control reply correlation exceeds its bound",
                    )));
                }
                policy.reserve(
                    crate::ingress_budget::IngressClass::Control,
                    crate::hosted::permission_state::reply_plan()?,
                )
            })
            .transpose()?;
        let frame = build();
        let sent = self.send_control_frame(&frame).await;
        drop(frame);
        drop(storage);
        if self.ingress.is_some() {
            sent
        } else {
            Ok(())
        }
    }

    pub(super) async fn send_admitted_frame(&self, frame: &serde_json::Value) -> crate::Result<()> {
        self.send_admitted_class(frame, crate::ingress_budget::IngressClass::Data)
            .await
    }

    pub(in crate::hosted) async fn send_control_frame(
        &self,
        frame: &serde_json::Value,
    ) -> crate::Result<()> {
        if self.ingress.is_none() {
            return self.send(frame).await;
        }
        if self.retired.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(super::write::retired_before_write(false));
        }
        self.send_admitted_class(frame, crate::ingress_budget::IngressClass::Control)
            .await
    }

    async fn send_admitted_class(
        &self,
        frame: &serde_json::Value,
        class: crate::ingress_budget::IngressClass,
    ) -> crate::Result<()> {
        let mut length = EncodedLength(0);
        serde_json::to_writer(&mut length, frame).map_err(|_| {
            crate::AdapterError::Rejected(text("ACP frame exceeds its encoding bound"))
        })?;
        if class == crate::ingress_budget::IngressClass::Control && length.0 > 4096 {
            return Err(crate::AdapterError::Rejected(text(
                "control reply exceeds its supported layout",
            )));
        }
        let storage = self
            .ingress
            .as_ref()
            .expect("admitted write dispatch")
            .reserve(
                class,
                crate::ingress_budget::AllocationPlan::new(
                    "conversation serialization",
                    &[
                        length.0 + 1,
                        crate::ingress_budget::sizing::shared_owner_bytes::<Message>(),
                    ],
                )?,
            )?;
        let scope = IngressWorkScope::new(storage);
        scope
            .run_io(async {
                let mut bytes = Vec::with_capacity(length.0 + 1);
                bytes.push(super::LANE_CONVERSATION);
                serde_json::to_writer(&mut bytes, frame).map_err(|_| {
                    crate::AdapterError::Rejected(text("ACP frame could not be encoded"))
                })?;
                self.write_owned(Message::Binary(bytes)).await
            })
            .await
    }

    pub(in crate::hosted) fn prepare_delivery(
        &self,
        frame: crate::hosted::retained_json::JsonFrame,
    ) -> crate::Result<crate::hosted::retained_json::JsonFrame> {
        let Some(policy) = &self.ingress else {
            return Ok(frame);
        };
        // Receipt: validated session (1024 bytes), UUID, hex digest and its
        // old/new formatting capacity. Passive activity can also allocate an
        // evidence wrapper and a shared effect node beside the delivery owner.
        let storage = policy.reserve(
            crate::ingress_budget::IngressClass::Data,
            crate::ingress_budget::AllocationPlan::new(
                "immutable runtime delivery",
                &[
                    1024,
                    36,
                    3 * 64,
                    crate::ingress_budget::sizing::shared_owner_bytes::<crate::ConversationUpdate>(
                    ),
                    2 * size_of::<crate::ConversationUpdate>() + 2 * size_of::<usize>(),
                ],
            )?,
        )?;
        Ok(frame.with_delivery_storage(IngressWorkScope::new(storage)))
    }

    pub(in crate::hosted) fn prepare_terminal(
        &self,
        frame: &serde_json::Value,
    ) -> crate::Result<Option<crate::RuntimeOutputUpdate>> {
        let Some(policy) = &self.ingress else {
            return Ok(None);
        };
        if frame.get("method").is_some() || frame.get("id").is_none() {
            return Ok(None);
        }
        // Terminal classification can substitute fixed diagnostics (each below
        // 512 bytes). Embedded credit JSON uses an unescape vector whose old
        // and new capacities can coexist, independently of the typed decoder.
        let embedded = typed::Shape::inspect(frame)?.string_bytes;
        let working = embedded
            .checked_mul(3)
            .and_then(|n| n.checked_add(1024))
            .ok_or_else(|| {
                crate::AdapterError::Rejected(text("terminal diagnostic layout overflow"))
            })?;
        let admission = typed::TypedAdmission::with_alternative_work::<crate::ConversationUpdate>(
            frame, policy, 512, working,
        )?;
        let prompt_id = frame
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::PromptIdentity::parse);
        let agent_correlation_digest = &self.agent_correlation_digest;
        let update = admission.initialize(|| {
            let outcome = crate::hosted::acp_turn::turn_outcome_with_policy(frame, true);
            crate::hosted::acp_credit_diagnostic::trace_credit_refusal(
                &outcome,
                agent_correlation_digest,
                prompt_id.as_ref(),
            );
            crate::hosted::acp_turn::scoped_turn_outcome(outcome, true, prompt_id)
        });
        Ok(Some(crate::RuntimeOutputUpdate::from_admitted(
            update.into_shared()?,
        )))
    }

    pub(in crate::hosted) fn terminal_update(
        &self,
        frame: &crate::hosted::retained_json::JsonFrame,
    ) -> crate::Result<crate::ConversationUpdate> {
        if self.ingress.is_some() {
            return frame
                .prepared_terminal()
                .map(crate::ConversationUpdate::RetainedEffect)
                .ok_or_else(|| {
                    crate::AdapterError::Rejected(text("terminal has no admitted owner"))
                });
        }
        let prompt_id = frame
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::PromptIdentity::parse);
        let outcome =
            crate::hosted::acp_turn::turn_outcome_with_policy(&**frame, self.json_pools.is_some());
        crate::hosted::acp_credit_diagnostic::trace_credit_refusal(
            &outcome,
            &self.agent_correlation_digest,
            prompt_id.as_ref(),
        );
        Ok(crate::hosted::acp_turn::scoped_turn_outcome(
            outcome, true, prompt_id,
        ))
    }

    pub(in crate::hosted) fn prepare_update(
        &self,
        frame: &serde_json::Value,
        observed: bool,
    ) -> crate::Result<Option<PreparedUpdate>> {
        let Some(policy) = &self.ingress else {
            return Ok(None);
        };
        if frame.get("method").and_then(serde_json::Value::as_str)
            == Some("session/request_permission")
        {
            return self.prepare_permission(frame, policy);
        }
        if observed
            || frame.get("method").and_then(serde_json::Value::as_str) != Some("session/update")
        {
            return Ok(None);
        }
        let Some(update) = frame.get("params").and_then(|params| params.get("update")) else {
            return Ok(None);
        };
        let admission = typed::TypedAdmission::new::<crate::SessionUpdate>(update, policy)?;
        Ok(Some(
            match admission.decode::<crate::SessionUpdate>(update) {
                Ok(update) => Ok(crate::RuntimeOutputUpdate::from_admitted(
                    update.map(crate::ConversationUpdate::Said).into_shared()?,
                )),
                Err(error) => Err(error.into_shared()?),
            },
        ))
    }
}

struct EncodedLength(usize);

impl std::io::Write for EncodedLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|n| *n <= super::MAX_FRAME_BYTES)
            .ok_or_else(|| std::io::Error::other("conversation encoding exceeds its bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[cfg(test)]
use futures_util::{Sink, Stream};
#[cfg(test)]
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;
#[cfg(test)]
use tokio_tungstenite::tungstenite::Error;
use tokio_tungstenite::tungstenite::Message;

#[path = "acp_ingress_socket.rs"]
mod socket;
pub(super) use socket::{
    admitted, legacy, BridgeSink, BridgeSocket, BridgeSource, IncomingFrame, OwnedSocket,
};

pub(super) enum AckSender {
    Legacy(mpsc::Sender<RuntimeOutputSource>),
    Admitted(crate::runtime_output::AdmittedAckSender),
}

pub(super) enum AckReceiver {
    Legacy(mpsc::Receiver<RuntimeOutputSource>),
    Admitted(crate::runtime_output::AdmittedAckReceiver),
}

pub(super) enum Ack {
    Legacy(RuntimeOutputSource),
    Admitted(SharedOwned<RuntimeOutputSource>),
}

impl std::borrow::Borrow<RuntimeOutputSource> for Ack {
    fn borrow(&self) -> &RuntimeOutputSource {
        match self {
            Self::Legacy(source) => source,
            Self::Admitted(source) => source,
        }
    }
}

impl AckSender {
    pub(super) fn receipt(&self, source: RuntimeOutputSource) -> RuntimeOutputReceipt {
        match self {
            Self::Legacy(sender) => RuntimeOutputReceipt::new(source, sender.clone()),
            Self::Admitted(sender) => RuntimeOutputReceipt::new_admitted(source, sender.clone()),
        }
    }
}

impl AckReceiver {
    pub(super) async fn recv(&mut self) -> Option<Ack> {
        match self {
            Self::Legacy(receiver) => receiver.recv().await.map(Ack::Legacy),
            Self::Admitted(receiver) => receiver.recv().await.map(Ack::Admitted),
        }
    }
}

#[cfg(test)]
#[path = "acp_ingress_allocation_tests.rs"]
mod allocation_tests;

#[cfg(test)]
#[path = "acp_ingress_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "acp_ingress_fixture.rs"]
pub(in crate::hosted) mod fixture;

#[cfg(test)]
#[path = "acp_ingress_socket_tests.rs"]
mod socket_tests;

#[cfg(test)]
#[path = "acp_ingress_permission_tests.rs"]
mod permission_tests;

#[cfg(test)]
#[path = "acp_ingress_startup_tests.rs"]
mod startup_tests;

#[cfg(test)]
#[path = "acp_ingress_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
#[path = "acp_ingress_context_tests.rs"]
mod context_tests;

#[cfg(test)]
#[path = "acp_ingress_watch_tests.rs"]
mod watch_tests;

#[cfg(test)]
#[path = "acp_ingress_error_tests.rs"]
mod error_tests;

#[cfg(test)]
#[path = "acp_ingress_compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(test)]
#[path = "acp_ingress_settings_tests.rs"]
mod settings_tests;
