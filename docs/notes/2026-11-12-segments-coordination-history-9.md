# Reader handoff and initial later-progress evidence

### S1 final published response handoff — f0d9746ef

Normally pushed `f0d9746efd52d9d183dcb9d72e2f275cca6b8278` on
`tl/cma-log-segments/response-consumers`. Code stays exactly
`b9d390634745c0d78e287ea238934187ebe18a94`; later commits are documentation
only. Current develop8167d01d8 is included. See the updated
`2026-11-12-response-consumer-handoff.md` and its committed docs/notes copy.

Current code:16 configuration/window passes and13 actual Mongo passes. Strict
CMA finished with22 normal-target and20 test-target unused shared roots, no
response/compatibility diagnostic; preserve the real S2/S4/S5 caller integration
instead of suppressions. Final fmt/whitespace/source-size6160 passed; canonical
secret scan3834commits,zero findings. No independent PR or readiness claim.

All owned gates ended. Target and source exports removed:2.245GiB,62.32GiB free
at readback. Recorded fixture containers/volumes absent; both lock inodes intact.
Project9 stays In Progress. Evidence milestones:
- https://github.com/ChronoAIProject/cma/issues/1349#issuecomment-6807100035
- https://github.com/ChronoAIProject/cma/issues/1674#issuecomment-6807100943


### S5 current domain pass and later-progress Mongo checkpoint —5123295c3

At clean11f381ea0e2677affdc710aa1a575903824f20a2 the affected-layer gate9 passed:49core/8HTTP/7adapter, zero failures and zero ignored. All three new current_activation domain cases executed. Exact log2026-11-12-s5-affected-units-11f381ea0.log. The existing shared lock was released; one target is retained for immediate strict/CMA/Mongo qualification.

Normally merged published audit-compatible current-proof44ad3819889e8c196a0dafd5a851227a8360cbb2 (API unchanged). Added three actual-Mongo test functions covering five fresh fixtures plus the current-evidence refusal variants: real worker and loss handler after keyed output or shared-publisher turn/input/fence progress; stale native confirmation; missing/v1/wrong-digest current publication; current publication or Retiring parent changes during native IO before acknowledgement. Read snapshots use one committed transaction. All tests assert unchanged original carrier/publication and no ordinary work/input replay. Native is a counted no-input stub, not production proof. Source files368/249/222lines (all under500; exact post-format counts in source).

Pushed5123295c3 after fetch/normal develop merge (already current). Strict four-package gate8 is queued on this clean source; light gates are running. Actual later-progress execution still requires the allocated S4 service.rs::activation_authorized consumer of the already-published11f381ea0 helpers. S5 has not edited that transferred path. Earlier strict PASS stays at024de5168 and affected-layer counts at11f381ea0. No new-head Mongo/activation/PR-ready claim. Temporary staged Rust files were applied and removed from notes.
