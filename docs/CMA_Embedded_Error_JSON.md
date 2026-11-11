# Embedded runtime error JSON bounds

When `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED` is true, credit-error
classification reads only the `error` and `error_code` discriminators from the
JSON embedded in a transport diagnostic. It does not materialize the embedded
object/array tree. The flag remains false by default and stays OFF on the
Thursday 13:00 train. Existing attachments retain their pinned policy.

A diagnostic can contain a large JSON array inside its message string. The outer
frame's structural preflight counts that content as a string; it cannot bound an
independent tree created by reparsing the string. The legacy classifier creates
that tree. The enabled classifier uses borrowed Serde visitors and consumes
unrelated values without allocating their containers.

Unknown values descend through the normal parser, preserving its recursion
limit. Recognized keys and string values are compared while borrowed; escaped
strings may use the parser's reusable unescape scratch. The helper refuses an
embedded body over 4 MiB; admitted outer frames are already limited to 4 MiB.
No allocation is proportional to the number of embedded container nodes.

The enabled path preserves:

- Classification of the first JSON object followed by transport diagnostics.
- Escaped recognized keys/values and duplicate-key last-value semantics.
- Exact string and unsigned-integer discriminator types.
- Malformed-input fallback, prompt identity and passive-authority scoping.
- Both direct error messages and runtime terminal-failure metadata.

Active waiters, recovered prompt results and passive terminal observations select
this path from the attachment's JSON-retention policy. Disabled mode uses the
legacy classifier. No source receipt, ACK or terminal is invented by pressure.
This parser does not introduce a capacity-refusal path or an extra waiting queue.

## Qualification and remaining scope

The allocation regression constructs a maximal admitted outer error string with
a large nested container and measures the real turn-outcome function in both
modes. It separately exercises escaped-key scratch and verifies zero measured
bytes after the final outcome is dropped. The existing seven parsed-JSON
allocation fixtures still run through shared, serialized test instrumentation.
Requested allocations are not process RSS or allocator/OS overhead.

Final measurement and combined-gate results are recorded after qualification.
This slice removes one nested-tree amplification path. It does not establish
admission or final-owner accounting for all typed effects, metadata/correlation
copies, codec storage, desktop, assemblies or fold/serialization work. Those
remain open under #1702 and the shared 128 MiB/RSS acceptance criteria.

Rollout follows the JSON-retention policy's existing compatible-runtime and
coordinated activation gates. Rollback disables that flag for new attachments;
no Sandbox restart or production campaign is performed by this source change.

Part of #1472. Implementation child #1724 under #1702.
