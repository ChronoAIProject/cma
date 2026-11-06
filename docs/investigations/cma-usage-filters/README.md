# Ops usage filter repair (#1401)

## Reproduction and cause

The baseline is the #1388 web code as retained on develop with #1398 and #1399.
Before changing it, five characterization tests exercised `ReportSelector` with
realistic ready, pending and HTTP 503 responses. Both Owner and Sandbox selectors
call `useUsageReport` when opened, creating a separate discovery report, and do
not mount `ChoicePage` until it is ready. A pending report has no selectable
choices; a report creation failure prevents the choices request entirely. Opening
both controls can add two report create/poll lifecycles beside the dashboard.

Owner search calls `read_owners`, which searches the subject prefix, while the
store constructs `Owner { display_name: None }`. Serde omits that field. The
frontend falls back to the subject, appends the subject when a name exists, and
shows the selected ID in its trigger. Sandbox options append the Sandbox ID and
also retain only the ID for the trigger. Existing tests supplied display names
and immediately ready reports, missing those production response paths.

This is a reproduced report-readiness dependency and identity-presentation
failure. It does not establish that every historical production click failed
for exactly the same upstream reason; production was not modified or used for
synthetic traffic.

## Behavior and scope

The Ops report filters are Time range, Owner, Status, Runtime version and Sandbox.
Time range has Last hour, Last 24 hours, Last 7 days, Last 30 days and Custom (up
to 30 days), retaining local start/end controls only for Custom. Sandbox name
search now lives inside its dropdown; the separate name/ID text filter is gone.
Apply commits the draft atomically and Clear filters restores the default range
and unrestricted choices. Owner selection immediately resets and narrows Sandbox
choices before Apply. The existing chart Resource (CPU/Memory/Disk) and Grouping
(By Sandbox/By owner) controls remain dropdowns.

The existing Menu/Picker supplies the portal, scrolling, arrow/Home/End navigation,
Escape and focus restoration. Search is debounced by 250ms. Identity/runtime
choices have at most 50 rows per page plus the unrestricted choice; paging
replaces rows instead of accumulating thousands of nodes. Loading, empty,
access/error and retry states are visible. URL selections resolve labels through
one bounded selected-value request. Neither search, opening a menu nor paging
creates or polls a report. The dashboard still owns the single report lifecycle,
including #1398's filter-change cancellation and pending-report behavior.

Owner labels use retained directory display names. Missing, blank or ID-shaped
names use `Unnamed owner` / `Unnamed Sandbox`. Every identity has a stable
12-hex-character SHA-256-derived reference, prefixed `account` / `Sandbox`, to
distinguish duplicate names across pages and searches. The reference is not a
substring of the original ID. Selected triggers, tooltips and missing-selection
states never fall back to IDs. Filter values and URL state keep opaque IDs.

Similar behavior remains outside scope: the personal Usage popup still uses
`ReportSelector` and free-text Sandbox search; its Agent picker falls back to
Agent IDs; allocation chart labels can fall back to owner subjects when report
rows lack names, and inventory detail intentionally exposes copyable IDs. These
surfaces were not redesigned by this change.

## API and rollout

An API deploy is required. Existing report APIs do not provide independent
option lists, and their owner projection omits names. The additive, read-only
`POST /api/v1/admin/usage/metrics/options` accepts bounded typed discovery input
and returns at most 50 `{value,label}` entries with a signed cursor. Its JSON
body keeps searched names out of proxy/access-log URLs. Every request checks
`ReadAllSandboxes`, including pages and selected-value resolution; cursors bind
the principal and query. Store errors deliberately discard driver text, which
can contain search values. No report job is created, no identity is logged and
no data migration is required. It reuses existing inventory/directory data and
the bounded reporting connection pool. Report endpoints remain compatible with
#1398 and later. Deploy this API addition before the web image; the lead must
serialize API deployment with the billing token bridge.

No dependency was added. Production is read-only for this task; merge and deploy
belong to the lead and deployment teammate.

## Verification

- Five baseline reproduction tests, retained outside the repository in the task
  evidence directory, passed before implementation.
- Frontend unit suite: 390 files / 3,624 tests; targeted filter/polling tests cover
  pending reports, cancellation, pagination, keyboard clear, deep links, duplicate
  labels, errors/retry and empty states with 400 / 20,001 choices.
- Disposable Mongo integration exercises actual aggregation queries with 400 and
  20,001 Sandboxes: complete name search, pagination, owner narrowing, duplicate
  and missing names, runtime discovery, selected resolution, literal search,
  rejected cursor reuse and zero report jobs.
- REST/core tests cover per-request authorization, invalid input, cursor binding,
  service failure and safe fallback labels. SDK tests exercise POST transport,
  response validation and administrator-only use.
- The web image's existing CI smoke job now also opens/searches/selects/clears
  the five filters in both themes, exercises Custom dates and both chart pickers,
  verifies one report poll per applied query, option errors/empty states, and
  checks entry assets return 200 with the image's production nginx headers.
  It continues to mount stale retained assets, guarding the #1399 incident.
- Local evidence is saved at
  `/Users/chronoai/code/work/tech-lead/notes/2026-11-06-usage-filters/`.
  Exact CI image identity, run ID and screenshot artifact are recorded in the PR
  after the web-image job completes.
