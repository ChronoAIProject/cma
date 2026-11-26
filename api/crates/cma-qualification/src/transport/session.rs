use super::*;
use crate::{ArmedControl, ReachedControl, RenewalArm};
use layer2_core::qualification::ScheduleEffect;

enum Session {
    Empty,
    Control {
        arm: ArmedControl,
        reached: Option<ReachedControl>,
    },
    Renewal(RenewalArm),
}

impl Session {
    async fn arm(
        &mut self,
        request: &Request,
        plan: &Plan,
        registry: &Registry,
        coordinator: Option<&OwnershipCoordinator>,
        isolated_store: Option<&Arc<dyn OwnershipStore>>,
    ) -> Option<Reply> {
        let owner = match (plan.mode, request.owner_digest, coordinator) {
            (Mode::Control, None, _) => None,
            (Mode::ControlWithOwner | Mode::Renewal, Some(digest), Some(coordinator)) => Some(
                coordinator
                    .capture_qualification_owner_digest(plan.context.agent_digest(), digest)
                    .await?,
            ),
            _ => return None,
        };
        *self = match plan.mode {
            Mode::Control | Mode::ControlWithOwner => {
                let result = match owner {
                    Some(owner) => registry.arm_with_owner(plan.context, plan.lifetime, owner),
                    None => registry.arm(plan.context, plan.lifetime),
                };
                Self::Control {
                    arm: result.ok()?,
                    reached: None,
                }
            }
            Mode::Renewal => Self::Renewal(
                registry
                    .arm_renewal(owner?, Arc::clone(isolated_store?), plan.lifetime)
                    .ok()?,
            ),
        };
        Some(Reply::Armed)
    }

    async fn wait(&mut self, stream: &mut UnixStream) -> io::Result<Reply> {
        match self {
            Self::Control { arm, reached } if reached.is_none() => {
                let observation = tokio::select! {
                    biased;
                    _ = stream.read_u8() => return Err(refusal("qualification peer closed before observation")),
                    observation = arm.reached() => observation,
                };
                let Some(receipt) = observation else {
                    return Ok(Reply::Refused);
                };
                let reply = Reply::Reached {
                    agent_revision: receipt.schedule.agent_revision,
                    scheduled: receipt.schedule.effect == ScheduleEffect::Scheduled,
                };
                *reached = Some(receipt);
                Ok(reply)
            }
            Self::Renewal(arm) => {
                let evidence = arm.evidence();
                Ok(Reply::Renewal {
                    started: evidence.started,
                    result: evidence.result,
                    caller_dropped: evidence.caller_dropped,
                })
            }
            _ => Ok(Reply::Refused),
        }
    }

    fn cancel_owner(&mut self) -> Reply {
        if let Self::Control {
            arm,
            reached: Some(reached),
        } = self
        {
            if arm.cancel_owner(reached) {
                return Reply::Cancelled;
            }
        }
        Reply::Refused
    }

    fn release(&mut self) -> Reply {
        match std::mem::replace(self, Self::Empty) {
            Self::Control { arm, .. } => arm.release(),
            Self::Renewal(arm) => arm.disarm(),
            Self::Empty => return Reply::Refused,
        }
        Reply::Released
    }
}

pub(super) async fn serve(
    mut stream: UnixStream,
    plan: &Plan,
    registry: &Registry,
    coordinator: Option<&OwnershipCoordinator>,
    isolated_store: Option<&Arc<dyn OwnershipStore>>,
    used: &mut bool,
) -> io::Result<()> {
    let mut session = Session::Empty;
    let mut history = Vec::<(Request, Reply)>::new();
    loop {
        let read_budget = if matches!(session, Session::Empty) {
            IO_TIMEOUT
        } else {
            plan.lifetime.max(IO_TIMEOUT)
        };
        let request = timeout(read_budget, read_request(&mut stream)).await??;
        if request.plan_digest != plan.digest
            || (request.action != Action::Arm && request.owner_digest.is_some())
        {
            return Err(refusal("qualification request plan mismatch"));
        }
        let sequence = usize::from(request.sequence);
        if let Some((original, reply)) = history.get(sequence) {
            if request != *original {
                return Err(refusal("qualification command replay changed"));
            }
            timeout(IO_TIMEOUT, write_reply(&mut stream, *reply)).await??;
            continue;
        }
        if sequence != history.len() || sequence > 7 {
            return Err(refusal("qualification command sequence refused"));
        }
        let reply = match request.action {
            Action::Arm if !*used => {
                let armed = timeout(
                    IO_TIMEOUT,
                    session.arm(&request, plan, registry, coordinator, isolated_store),
                )
                .await?;
                if armed.is_some() {
                    *used = true;
                }
                armed.unwrap_or(Reply::Refused)
            }
            Action::Wait => session.wait(&mut stream).await?,
            Action::CancelOwner => session.cancel_owner(),
            Action::Release => session.release(),
            _ => Reply::Refused,
        };
        timeout(IO_TIMEOUT, write_reply(&mut stream, reply)).await??;
        history.push((request, reply));
    }
}
