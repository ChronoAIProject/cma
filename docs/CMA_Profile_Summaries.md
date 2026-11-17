# Compatibility profile list summaries

The Agent, Workspace and Sandbox profile lists remain compatibility and recovery
surfaces. They do not introduce profile selection into the normal product
journey. Their response fields are unchanged.

## Selection and publication contract

Each list requires `ManageOwnProfiles` and selects only the authenticated owner's
profiles. Private, unpublished and retired profiles remain visible to that owner;
internal profiles and other owners' profiles remain excluded, including public
profiles owned by somebody else. Marketplace selection is a separate operation.

Pages retain ascending profile-ID order, the exclusive last-ID cursor and the
existing 1–200 item limit (default 100). The extra candidate determines whether
`next` exists. An empty page remains an empty page, not a refusal.

Layer 1 projects a Layer 2 owner-summary page. The Mongo implementation reads the
profile drafts and immutable publication evidence in one snapshot, including any
`getMore` batches. A publication committed during that read appears on a later
request; it cannot mix a newer publication into the remainder of the current
page. Cursors do not pin a snapshot across separate requests.

`latest_revision` comes from the greatest numeric published revision. It is null
only when no publication exists. Publication timestamps, draft versions and
denormalized publication hints do not select the revision. Existing source
decoders verify the profile and the two newest revisions, preserving the former
latest-read lookahead integrity check. The extra pagination candidate's profile
is validated, but its revisions are not used or validated. Older revision history
is outside this read's evidence boundary.

Invalid selected evidence, a database error or an expired read deadline refuses
the whole page with the existing error mapping. A failed cursor never returns a
partial successful page. No read repairs or writes source data.

## Work bounds and indexes

One aggregation selects at most 201 profiles and joins at most two revisions per
profile. Mongo may use the existing owner or `_id_` index to select the profile
page. The existing unique `{ profile_id: 1, revision: 1 }` index supplies the
reverse revision lookup. The revision work does not grow with retained revision
history. Owner and internal filtering retain the existing selection semantics:
unrelated or internal profile keys may still be scanned before 201 matching candidates are
found. This change bounds enrichment, not that candidate scan.

The driver requests at most 603 source rows per batch and streams their
validation. It retains only the selected profiles and publication metadata.
Large source documents may require additional `getMore` commands under Mongo's
batch byte limit; source documents are returned separately instead of one
combined draft-and-revisions result. Both the server operation and the complete
snapshot read have a 15-second deadline. Deadline or snapshot-expiry failures
use the same unavailable response as other database read failures.

The command contract for a 200-profile page with small source documents is at
most three Mongo commands, independent of revision depth. Large-document and
concurrent-publication contracts exercise multiple batches. Counts include the
complete profile handler's Mongo reads, excluding fixture writes and separately
issued parity/explain queries.

## Persistence and rollback

This is a read-only use of existing schema-v1 profile and revision documents and
their existing indexes. It adds no collection, index, grant, persisted field,
writer gate, configuration or reader floor. Existing publication transactions,
backup/restore behavior and older readers remain unchanged. Rollback restores
the previous enrichment queries over the same data.
