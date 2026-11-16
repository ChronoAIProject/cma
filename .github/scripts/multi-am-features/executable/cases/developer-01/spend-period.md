# ADM-054 platform spend period

The existing public `spend_period` recipe reads real settings and writer
availability under the parent's exclusive settings lease. It supplies a valid
one-day UTC period in the exact twenty-character `YYYY-MM-DDTHH:MM:SSZ` format
whose cap is `Number.MAX_SAFE_INTEGER` (or one less when
needed to change the current value), plus warning/stop percentages, protected
reserve and lag allowance. The case requires the actual spend writer gate and
an authenticated compiled admin cookie session. No writer availability is
simulated or toggled by this row. The six automatic / 31 parent-required
setup counts are unchanged.

The backend's `PlatformBudgetPolicy::period_bounds` uses
`clock::seconds_from_rfc3339`: whole seconds, uppercase UTC, a valid calendar
date at or after the epoch, and start strictly before end. Positive fixture
validation shares the cloud-observation timestamp parser for this contract.
The frontend accepts broader RFC3339 syntax, including fractional seconds and
offsets; the backend receives those strings unchanged and rejects them. The
offline model therefore checks that backend boundary independently after
frontend validation. Its former fractional `.000Z` success was a source/model
defect, not a runtime observation. No product parser or API expectation changes.

The case owns fifteen negative variants. They cover unsafe integers,
fractions, exponents, leading zeros, negative/zero caps, percentage bounds,
negative lag, warning/stop ordering, reserve-plus-lag threshold, reversed/equal
period ordering, impossible dates and missing RFC3339 offsets. Setup cannot
reduce these variants. Each input must retain its exact draft text, mark the
intended control `aria-invalid=true`, and link through `aria-describedby` to
the exact source-defined error message. No settings mutation may dispatch,
even one refused by the server; complete snapshots on both AMs must stay equal.

The valid save joins the exact full PUT body and expected revision to the
acknowledged response and actual serving-AM receipt. Both AMs must return the
same complete snapshot, with every accounting-unit value preserved without
rounding. Replacing the editable period fields preserves optional platform
fields such as cloud policy and all unrelated settings. Negative intents are
recorded before clicking, so an erroneous acceptance can be identified during
cleanup rather than silently losing custody.

The shared Operations capture retains report-prefetch admissions separately
from the single allowed settings write. It preserves all raw serving snapshots,
timing and attribution; unresolved report admissions still fail after drain.
Browser close/drain precedes one memoized restoration using a fresh peer
revision CAS. Close, evidence and restoration failures remain independent;
unknown saves/restorations are never replayed. The parent disposes immutable
audit/report records with the local database. Import and offline model tests
start no application, browser, native or production resources. Runtime acceptance
still requires the sole parent's real two-AM campaign.
