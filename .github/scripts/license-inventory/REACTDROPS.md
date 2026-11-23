# ReactDrops source and replacement inventory — DRAFT

The exact file inventory is [reactdrops-inventory.json](reactdrops-inventory.json).
It identifies **six derived files**, **three license/provenance files**, eight
direct integration/test files and 23 application/verification files. The latter
two groups are review surfaces, not assertions that their code is derived.
No listed file is deleted or replaced by this groundwork.

The provenance authority is the existing
[`source-map.json`](../../../web/src/decoration/vendor/reactdrops-topographic/source-map.json)
and [`NOTICE`](../../../web/src/decoration/vendor/reactdrops-topographic/NOTICE).
The pinned upstream revision is
[`aed0014649a2f0964041ac33c5f02dc3a40c5e13`](https://github.com/crisjavr/reactdrops/tree/aed0014649a2f0964041ac33c5f02dc3a40c5e13).
The inventory records full SHA-256 values for the six derived files and the three
notice/provenance files, so content changes can be distinguished from renaming.

## Derived material

| Current file under `web/src/decoration/` | Derivation | Removal or independent replacement boundary |
| --- | --- | --- |
| `vendor/reactdrops-topographic/topographic-field.ts` | Seeded elevation algorithm | Field generation |
| `vendor/reactdrops-topographic/topographic-surface-shaders.ts` | Flow shaders and sampling | Shader implementation |
| `vendor/reactdrops-topographic/topographic-surface-resources.ts` | WebGL resource setup | Allocation/disposal |
| `vendor/reactdrops-topographic/topographic-surface.ts` | Height texture and renderer | Surface factory/draw |
| `assets/topographic-dark.svg` | Generated contour mask | Independent artwork |
| `assets/topographic-light.svg` | Same derived mask bytes | Independent artwork |

The removed historical `topographic-contours.ts` helper is **not** a current
tracked file. Its omission does not remove the derivation of the SVGs. Upstream
React integration, presets, Canvas2D renderer, grain and CSS are recorded as not
vendored in the source map; do not mistake its list of upstream originals for
the list of files present in this tree.

## Pending disposition

**[PENDING: independent replacement, explicit written redistribution permission,
or a clearly defined exclusion and distribution boundary.]**

The [exact RCL text](../../../web/src/decoration/vendor/reactdrops-topographic/LICENSE)
permits specified End Product uses while restricting redistribution as reusable
development assets and source-code bundles. It must not be presented as a grant
to extract or relicense these files under a future CMA license. Retain license
and copyright notices while any covered material remains. Historical
distribution handling is a separate owner decision.

If removal is selected, the complete vendor directory and **both** SVGs need
disposition together with their imports, CSS masks, license emission and build
rewrite. A disabled feature flag does not remove files from source archives.
Renaming, porting or regenerating the same contour algorithms is not an
independent replacement. A simple independently authored CSS/SVG decoration is a
possible replacement boundary; no implementation or artwork is chosen here.

The application/verification group in the JSON identifies the wrapper, lifetime
policy, theme palette, landing/home consumers, browser checks and default-off
artifact checks that need review for either removal or replacement. Shared
application code should be preserved where still needed; these paths are not a
bulk deletion list. Keep or adapt the existing reduced-motion, theme, visibility,
deadline, rendering-budget and optional-load failure coverage to the approved
behavior.

After an approved change, inspect tracked files, the actual source archive and
both enabled/disabled Web outputs for old code, derivatives and emitted license
assets. Run frontend lint, typecheck, unit tests, production build and the
relevant visual/theme/reduced-motion browser checks. Update this catalog and the
source notices to describe the resulting distribution. This document records
current provenance and review boundaries; it does not certify a future release.
