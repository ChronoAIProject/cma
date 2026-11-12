# Segmented transcript pages

`GET /api/v1/agents/{id}/transcript?limit=32` reads a bounded page from the
certified segmented history. The limit is 1–32 **source events**, so a page may
contain no messages when those events affect other views. It still advances.
A long turn spans pages; each page repeats the turn's scalar metadata and
outcome, with only the messages from that source interval. Merge turns by
`turn_sequence` and messages by their original `sequence`. Preserve page order
when extending `agent_messages`. A queued owner message precedes its turn and
appears exactly once on that turn's first page.

A response with more history includes `next`, an object containing `turn`,
`after`, `through`, and `limit`. Pass all four as query parameters for the next
request. `after` is a source cursor, not the final visible message sequence.
An absent `next` means completion. A changed source high-water mark refuses the
continuation; start a new first page and replace the old snapshot. Sharing and
owner authority are checked again after the buffered read, before returning it.

Segmented pages seek turns through the numeric evidence index and read at most
32 requested originals plus four header points. Capture validates the active
physical segment (at most 10,128 entries / 65 MiB) once per request. Closed
history is never replayed to reach a cursor. Requested point sets have a 4 MiB
source-byte cap, and each emitted turn page has a 4 MiB serialized cap. These
bounds apply independently of the number of sealed segments and turn length.

Omitting all query parameters preserves the existing complete response shape,
without a `next` property. The segmented compatibility response has a fixed
200-source-event traversal budget and a 4 MiB serialized budget. A larger
history returns HTTP 400 `transcript_pagination_required` with an explicit
pagination instruction; it never falls through to an unlimited
complete-log allocation. Legacy histories keep their existing exact fold and
limits; explicit paging slices that exact result. Legacy optimization flags
remain default off. Format-2 dispatch always uses certified segmented readers.

Titles use the certified first task turn and four original points (start,
owner, last assistant, terminal); the existing exact fold owns title truncation.
Setup, queued input, steering, and terminal eligibility retain their prior
semantics. Usage consumes the aggregate certified at the same captured source.
Missing, stale, foreign, unsupported, or corrupt segmented proof is a refusal,
never an invitation to scan raw history or repair data during a read.

Suggestion reads, claims, and completions require a valid segmented range even
inside a transaction and even for an empty tail. Enqueue additionally validates
the exact contiguous append batch and its already-published source entries in
the same transaction. An unpublished append is refused; no pending-publication
exception is granted by `snapshot=false`.

The TypeScript and Rust SDK `transcript` methods keep the old single request
for within-budget histories and automatically follow pages only after that
specific typed refusal. They preserve the complete returned shape. Both also
expose an explicit `transcriptPage` / `transcript_page` method for incremental
rendering. Hydration checks immutable source identity, monotonic cursors and
message order, and stable scalar turn metadata. It propagates cancellation and
authority refusal without returning a partial transcript. Browser REST hydration
uses the same SDK path and retains its existing navigation and draft lifecycle.
