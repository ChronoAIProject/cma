# Context-goal observation storage

The existing default-off `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED` policy
keeps context-goal session identity attached to the admitted immutable JSON
frame. It uses an exact borrowed identifier comparison. Readiness checks
snapshot the matching safety result without cloning the observed identifier.
The disabled path keeps its independently owned legacy identifier.

The observation retains the entire original frame, including unused siblings.
Its existing 32 MiB parsed-tree component reservation and optional shared
ingress reservation remain held through every observation and RPC consumer.
Replacement releases the previous observation owner after borrowing the new
already admitted frame; final release occurs only when the last owner drops.
This introduces no pool, capacity upgrade, wait or changed wire limit.

The complete-frame retention is a deliberate capacity tradeoff: a small session
identifier can keep a large admitted tree alive. That full tree stays charged.
Combined attachment/useful-capacity qualification must include this lifetime.
Observation replacement and reconnect provide release points; a native goal
continues to veto maintenance until reconnect even if later reported null.

Missing or malformed observations are ignored. A foreign session does not
replace an existing observation. Null/native-goal classification, exact session
matching, context-refresh and instruction-recovery gates keep their existing
behavior. A runtime receipt or context observation cannot fabricate a durable
ACK. Existing attachment admission refuses before cursor advancement when the
tree or shared pool cannot admit a new frame.

Qualification covers enabled/disabled safety equivalence, unrelated and foreign
observations, replacement/final release, local/shared pressure, a large unused
sibling and startup transfer. It measures requested allocation, not RSS; final
results are recorded after the repository gates pass.

Only context-goal observation storage is covered. The active session snapshot,
pre-activation queue identifiers, typed effects/schema containers, other
correlation and response copies, codec/fragment buffers, desktop, durable
working storage and full ingress/RSS qualification remain open under #1702.
Bridge/pool bookkeeping and allocator/OS overhead remain separate from the
parsed-tree allowance. The policy remains OFF on the Thursday 13:00 train.

Rollback disables the existing JSON policy for new attachments. Existing
admitted observations retain their owners until replacement or actual teardown.
No production activation or campaign is included.

Part of #1472. Implementation child #1731 under #1702.
