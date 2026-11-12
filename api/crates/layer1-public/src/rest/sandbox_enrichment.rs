//! Optional presentation reads run only after the hierarchy's authorization fence.
use layer2_core::{sandbox::SandboxHierarchyReading, Caller};

use super::super::instances::HierarchyState;
use super::super::{public_names, sandbox_colors};

pub(super) async fn read(
    state: &HierarchyState,
    caller: &Caller,
    reading: &SandboxHierarchyReading,
) -> (Option<String>, Option<String>) {
    let color = async {
        let timer = state
            .hierarchy
            .detail_phase(layer2_core::sandbox::SandboxDetailPhase::Colors);
        let value = timer
            .scope(async {
                match reading {
                    // Shared readers never read or assign the owner's palette.
                    SandboxHierarchyReading::Personal(personal)
                        if personal.sandbox.owner == caller.subject =>
                    {
                        sandbox_colors::colors(state, &caller.subject, &[&personal.sandbox])
                            .await
                            .remove(personal.sandbox.id.as_str())
                    }
                    _ => None,
                }
            })
            .await;
        timer.complete();
        value
    };
    let name = async {
        let SandboxHierarchyReading::Personal(personal) = reading else {
            return None;
        };
        let timer = state
            .hierarchy
            .detail_phase(layer2_core::sandbox::SandboxDetailPhase::Names);
        let names = timer
            .scope(public_names::resolve(
                &state.users,
                std::slice::from_ref(&personal.sandbox.owner),
            ))
            .await;
        timer.complete();
        names.get(personal.sandbox.owner.as_str()).cloned()
    };
    if state.concurrent_detail_enrichment {
        // Independent service handles; neither future borrows a ClientSession.
        tokio::join!(color, name)
    } else {
        (color.await, name.await)
    }
}
