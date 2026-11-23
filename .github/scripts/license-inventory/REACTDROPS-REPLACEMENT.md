# Decoration replacement behavior — DRAFT

This is a proposed independent replacement for the ReactDrops decoration.
**[PENDING: owner selects replacement and approves the visual behavior below.]**
It defines acceptance behavior; it does not implement an effect, remove assets,
grant a license or authorize a release.

## Inputs and independence

The design inputs are CMA's placement, accessibility, theme and lifetime
contracts. Shape coordinates, path data, shaders, procedural parameters and
generated geometry from ReactDrops are excluded. Existing artwork must not be
traced, translated or regenerated to implement this proposal. Review newly
authored geometry against the requirements below rather than matching the old
effect's pixels. This specification is not a formal clean-room attestation.

The behavior baseline is the first-party contract at
`cfefa623431882fbac2f65bb2a34153dbf687047`. These are source/test requirements;
the proposed appearance and performance still need browser verification.

| First-party contract | Requirement retained |
| --- | --- |
| [Landing placement](../../../web/src/landing/SignInLanding.tsx) | Decoration belongs behind the existing character; sign-in actions remain independent. |
| [Home eligibility tests](../../../web/src/create/TaskHomeDecoration.test.tsx) | Only the eligible personal first-visit slot mounts decoration; draft input identity and contents survive transitions. |
| [Provider tests](../../../web/src/decoration/HomeDecorationProvider.test.tsx) | Providing a decoration node alone does not initialize an effect. |
| [Backdrop tests](../../../web/src/decoration/TopographicBackdrop.test.tsx) | Default-off inertness, static fallback, reduced-motion/focus safeguards and cleanup. |
| [Playback policy tests](../../../web/src/decoration/topographicPolicy.test.ts) | Bounded optional playback, no replay or catch-up, and conservative eligibility. |
| [Slot presentation](../../../web/src/decoration/TopographicBackdrop.css) | Existing envelope, clipping and separation from layout and pointer interaction. |
| [Design tokens](../../../web/src/styles/design-platform.css) | Shared semantic border and brand colors for light and dark themes. |

## Recommended appearance: static open arcs

Create one open elliptical arc near the upper-left edge, a second separate arc
near the lower-right edge and one short accent dash. Leave the center open for
the character. The composition should frame the character quietly and remain
subordinate to the heading and primary action. Newly authored elementary geometry
is sufficient; this proposal requires no procedural renderer or external asset.

The three-shape limit is a proposed design constraint. The slot dimensions below
come from CMA's existing presentation contract. Fresh shape coordinates and
shared stroke/spacing choices require design review before implementation.

| Property | Proposed acceptance behavior |
| --- | --- |
| Placements | Landing brand area and the existing eligible personal first-visit Home slot. |
| Eligibility | Default-off remains inert. Home loading/error/stale navigation, existing Agents, shared or organization scope, contextual creation, selected templates and submitted tasks do not acquire decoration. |
| Envelope | Fit and clip within the host, preserving the maximum 560 by 220 CSS-pixel slot and the first-visit height cap of 164 CSS pixels. |
| Geometry | At most three simple unfilled primitives, with an open center; no embedded bitmap, font, external reference, filter or pointer response. |
| Palette | Neutral strokes use `--dsw-alias-border-l2`; at most one accent uses `--dsw-alias-brand-primary`. Use shared stroke/spacing conventions. |
| Themes | One geometry uses semantic tokens in both themes. Theme changes recolor it without replacing the application subtree. |
| Hierarchy | Character, meaningful text and controls remain unobscured. Decoration adds no layout shift, scrolling or new layout dimensions. |
| Interaction | No pointer target, focus target, navigation action or input remount is introduced. Existing draft text and selection remain intact. |
| Accessibility | Exclude decoration from the accessibility tree and tab order. It carries no information or announcement. |
| Failure | If required styling is unavailable, omit decoration while preserving the character, text and actions. |
| Static resource bound | No effect-attributable external request, added runtime dependency, canvas/WebGL context, animation loop, timer or observer. |

Static presentation removes the existing optional animated flourish. This is an
intentional visual behavior change requiring approval; it is not a promise of
visual parity. The implementation candidate is a small inline vector using the
existing React/CSS stack. No markup, path data or implementation is supplied here.

## Optional reveal, only if selected

**[PENDING: owner requests motion instead of the static-only recommendation.]**
Keep the same independent geometry and allow one brief opacity reveal. Do not
move or morph paths. Propose at most one second of reveal, bounded by the existing
four-second elapsed-time ceiling. The shorter reveal duration is a new proposal.

The static presentation is the fallback and final state. Reduced motion, coarse
pointer, viewport width at or below 720 CSS pixels, Save-Data, missing eligibility
APIs or existing interaction focus select static immediately. The default-off
feature state performs no scheduling or initialization.

| Event | Required optional-motion behavior |
| --- | --- |
| First eligible visible mount after content paint | Admit at most one active effect per document and one opportunity per placement. |
| Interactive control receives focus, or reduced motion becomes enabled | Finish permanently for that placement and show static geometry. Reversing the condition does not restart motion. |
| Document becomes hidden, or host becomes hidden/offscreen | Stop motion immediately and finish permanently at the static state. |
| Theme change or resize | Recolor/refit without renewing admission or restarting motion. |
| Remount, StrictMode or return to Home | Preserve the consumed opportunity within the document lifetime. |
| Another placement is already active | Render static without queuing a later reveal. |
| Completion, interruption or unmount | Cancel enhancement and release every listener, observer and animation handle; cleanup is idempotent. |
| Failure or late callback | Preserve static presentation or omit decoration. No fatal application error, retry loop or resurrection of completed motion. |

Finishing when hidden changes the current pause-with-deadline behavior. Approval
of the motion alternative must explicitly include this simplification. A browser
opacity animation does not inherit the old renderer's draw-submission counter.
Verify the resource intent through zero JavaScript frame loops, one short active
animation, prompt cancellation and no remaining activity after completion.

The static-only choice needs no playback machinery. Add motion support only if
that alternative is approved.

## Shared behavior interface

Both placements use one decoration module with a small interface: placement and
documented inert/failure behavior. Callers retain first-visit eligibility and
their content. Geometry, theme presentation and optional playback stay inside
the module. Decoration never owns drafts, focus, authentication or navigation.

The existing decoration slot is the seam. If optional motion is selected, static
presentation and reveal share the same geometry behind that seam. Clock and
admission dependencies should be controllable for deterministic tests. A static
implementation needs no renderer factory or general effect registry.

Review first-party slot/theme provenance before reuse. Do not reuse any of the
six derived files or recover their content through renamed assets, generated
output or another package. Optional enhancement failures may report a fixed,
non-sensitive diagnostic category at most once per document. Ordinary static
rendering needs no logging or telemetry dependency.

## Acceptance after implementation is authorized

- [ ] Record new geometry inputs and authorship; review for copied or traced
  material. Review any newly introduced third-party material separately.
- [ ] Prove default-off and excluded Home states do not mount an effect, and
  providing a node alone does not initialize one.
- [ ] Preserve draft DOM identity, contents, selection and keyboard focus across
  decoration, template and navigation transitions; preserve the sign-in action.
- [ ] Review both placements in both themes, narrow/wide hosts and browser zoom;
  verify clipping, readability, character prominence and no layout shift.
- [ ] Check accessibility-tree exclusion and keyboard behavior. Reduced motion
  must have no animated frames or transient reveal.
- [ ] Verify the static resource bound. If motion is approved, test all events
  above, simultaneous placements, expiry, failed initialization and late callbacks.
- [ ] Check tracked source, source archives and enabled/disabled Web outputs
  against the [removal inventory](REACTDROPS.md). An independently replaced
  distribution must contain no old code or derived SVG, including copied output.
- [ ] Run frontend lint, typecheck, unit tests, production build, relevant visual
  and browser checks, source license inventory, and secret/public-tree checks on
  the implementation head.

Shared application paths in the inventory are review surfaces, not a bulk
deletion list. Attribution handling depends on the approved disposition and
remaining material. Historical distribution handling, project licensing and
release scope remain separate owner decisions. None of the implementation
acceptance items is asserted complete by this specification.
