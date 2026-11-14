//! Identity-free completion telemetry for the first Agent Ready publication.
//!
//! Lifecycle records retain the original BSON millisecond admission boundary;
//! legacy and existing-workspace Agents still report an explicitly unavailable
//! total until they gain an equivalent durable boundary.

use layer2_core::sandbox::{Agent, AgentOrigin, Sandbox};
use std::time::{SystemTime, UNIX_EPOCH};

use super::startup_timing::StartupPhaseDurations;

pub(super) struct CompletionTiming {
    pub admission_ms: Option<i64>,
    pub admitted_desktop: Option<bool>,
    pub admitted_machine_included: Option<bool>,
    pub ready_at: SystemTime,
    pub publish_ready_ms: u64,
    pub startup_phases: Option<StartupPhaseDurations>,
}

pub(super) fn emit(
    agent: &Agent,
    ready_path: &'static str,
    runtime_path: &'static str,
    timing: CompletionTiming,
) {
    let create_path = match &agent.origin {
        AgentOrigin::Lifecycle { .. } => "lifecycle",
        AgentOrigin::Manual => "existing_workspace",
        AgentOrigin::Trigger { .. } | AgentOrigin::WorkspaceProfile { .. } => "background",
    };
    let create_class = match &agent.origin {
        AgentOrigin::Trigger { .. } | AgentOrigin::WorkspaceProfile { .. } => "unknown",
        AgentOrigin::Lifecycle { .. } => {
            lifecycle_class(timing.admitted_desktop, timing.admitted_machine_included)
        }
        // Existing-workspace agents have no persisted lifecycle request in
        // this path. A first-message record is user content, not durable
        // create intent, so keep the class unknown until that admission fact
        // is carried explicitly.
        AgentOrigin::Manual => "unknown",
    };

    emit_fields(create_path, create_class, ready_path, runtime_path, timing);
}

fn lifecycle_class(desktop: Option<bool>, machine_included: Option<bool>) -> &'static str {
    match (desktop, machine_included) {
        (Some(desktop), Some(machine_included)) if desktop || machine_included => "desktop",
        (Some(false), Some(false)) => "plain",
        _ => "unknown",
    }
}

pub(super) fn runtime_path(agent: &Agent, sandbox: &Sandbox) -> &'static str {
    let lifecycle_origin_matches = match (&agent.origin, &sandbox.origin) {
        (
            AgentOrigin::Lifecycle {
                operation_id: agent_operation,
            },
            layer2_core::sandbox::SandboxOrigin::Lifecycle {
                operation_id: sandbox_operation,
            },
        ) => agent_operation == sandbox_operation,
        _ => false,
    };
    if !lifecycle_origin_matches {
        return "unknown";
    }
    let Some(runtime) = sandbox.runtime.as_ref() else {
        return "unknown";
    };
    let Some(home) = sandbox.lifecycle.home.as_ref() else {
        return "unknown";
    };
    let Some(native) = home.native_pool.as_ref() else {
        return "unknown";
    };
    if native.claim.is_some()
        && native.cleanup.is_none()
        && !native.cleaned
        && home.attachment().is_some()
        && runtime.provision_generation == Some(native.request.runtime_generation)
    {
        "warm"
    } else if runtime.provision_generation.is_some() {
        "cold"
    } else {
        "unknown"
    }
}

fn emit_fields(
    create_path: &'static str,
    create_class: &'static str,
    ready_path: &'static str,
    runtime_path: &'static str,
    timing: CompletionTiming,
) {
    let ready_ms = timing
        .ready_at
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    let (timing_basis, timing_valid, total_ms) = timing_values(timing.admission_ms, ready_ms);

    macro_rules! record {
        ($($extra:tt)*) => {
            {
                tracing::info!(
                    target: "agent_create_completed",
                    parent: None,
                    event = "agent_create_completed",
                    schema_version = 1u8,
                    create_path,
                    create_class,
                    runtime_path,
                    ready_path,
                    boundary = "ready_commit_ack",
                    timing_basis,
                    timing_valid,
                    phase_coverage = "partial",
                    phase_accounting = "observed_attempts",
                    publish_ready_ms = timing.publish_ready_ms,
                    $($extra)*
                    "Agent create completed"
                );
            }
        };
    }

    match (total_ms, timing.startup_phases) {
        (Some(total_ms), Some(phases)) => record!(
            total_ms,
            open_conversation_ms = phases.open_conversation_ms,
            open_context_ms = phases.open_context_ms,
            open_intent_ms = phases.open_intent_ms,
            open_runtime_ms = phases.open_runtime_ms,
        ),
        (Some(total_ms), None) => record!(total_ms,),
        (None, Some(phases)) => record!(
            open_conversation_ms = phases.open_conversation_ms,
            open_context_ms = phases.open_context_ms,
            open_intent_ms = phases.open_intent_ms,
            open_runtime_ms = phases.open_runtime_ms,
        ),
        (None, None) => record!(),
    }
}

fn timing_values(
    admission_ms: Option<i64>,
    ready_ms: Option<i64>,
) -> (&'static str, bool, Option<u64>) {
    match (admission_ms, ready_ms) {
        (Some(start), Some(end)) => match end
            .checked_sub(start)
            .and_then(|elapsed| u64::try_from(elapsed).ok())
        {
            Some(total_ms) => ("persisted_wall_ms", true, Some(total_ms)),
            None => ("persisted_wall_ms", false, None),
        },
        _ => ("unavailable", false, None),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::UNIX_EPOCH;

    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn formatted_event_has_fixed_fields_and_no_parent_identity() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Capture(buffer.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let parent = tracing::info_span!("parent", agent_id = "agent-id-canary");
            let _entered = parent.enter();
            super::emit_fields(
                "lifecycle",
                "plain",
                "ordinary",
                "unknown",
                super::CompletionTiming {
                    admission_ms: Some(1_000),
                    admitted_desktop: Some(false),
                    admitted_machine_included: Some(false),
                    ready_at: UNIX_EPOCH + std::time::Duration::from_millis(6_001),
                    publish_ready_ms: 12,
                    startup_phases: Some(super::StartupPhaseDurations {
                        open_conversation_ms: Some(100),
                        open_context_ms: Some(10),
                        open_intent_ms: Some(20),
                        open_runtime_ms: Some(70),
                    }),
                },
            );
        });
        let line = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(line.contains("event=\"agent_create_completed\""), "{line}");
        assert!(line.contains("create_path=\"lifecycle\""), "{line}");
        assert!(line.contains("timing_valid=true"), "{line}");
        assert!(line.contains("total_ms=5001"), "{line}");
        assert!(line.contains("publish_ready_ms=12"), "{line}");
        assert!(!line.contains("agent-id-canary"), "{line}");
        assert!(!line.contains("agent_id"), "{line}");
    }

    #[test]
    fn persisted_timing_keeps_the_five_second_boundaries_and_rejects_bad_clocks() {
        for (elapsed, expected) in [(4_999, 4_999), (5_000, 5_000), (5_001, 5_001)] {
            assert_eq!(
                super::timing_values(Some(1_000), Some(1_000 + elapsed)),
                ("persisted_wall_ms", true, Some(expected))
            );
        }
        assert_eq!(
            super::timing_values(Some(5_001), Some(5_000)),
            ("persisted_wall_ms", false, None)
        );
        assert_eq!(
            super::timing_values(Some(i64::MIN), Some(i64::MAX)),
            ("persisted_wall_ms", false, None)
        );
    }

    #[test]
    fn lifecycle_class_uses_the_persisted_desktop_or_machine_intent() {
        assert_eq!(super::lifecycle_class(Some(false), Some(false)), "plain");
        assert_eq!(super::lifecycle_class(Some(true), Some(false)), "desktop");
        assert_eq!(super::lifecycle_class(Some(false), Some(true)), "desktop");
        assert_eq!(super::lifecycle_class(Some(true), Some(true)), "desktop");
        assert_eq!(super::lifecycle_class(None, Some(false)), "unknown");
        assert_eq!(super::lifecycle_class(Some(false), None), "unknown");
    }
}
