# Third-party notices

[Apache-2.0](../LICENSE) applies to first-party CMA source. Third-party materials
retain their own copyright, licenses and notices. This index identifies vendored
materials in the source tree and points to their notices and provenance.

| Material | License and notice | Provenance |
| --- | --- | --- |
| Babel parser | [MIT](scripts/image-content-vendor/BABEL-LICENSE) | [Version 7.29.3](scripts/image-content-vendor/PROVENANCE.json) |
| Moby seccomp profiles | [Apache-2.0](scripts/seccomp-vendor/MOBY-PROFILES-LICENSE) | [Pinned upstream sources](scripts/desktop-seccomp.mjs) |
| Agent Sandbox controller assets | [Apache-2.0](scripts/migration-native-assets/LICENSE.agent-sandbox.txt) | [Controller release v1.0.2](scripts/migration-native-assets/manifest.json). Other materials in this asset directory have separate origins that still need review. |
| `actions/download-artifact` test fixture | [MIT in the fixture header](scripts/fixtures/download-artifact/download-artifact.js) | [Pinned upstream source](scripts/fixtures/download-artifact/PROVENANCE.json) |
| DeepSeek Harness glyphs copied into `web/src/ui/icons/` | [MIT](../web/src/styles/vendor/LICENSE-deepseek-harness.txt) | [Upstream snapshot checked against the copied glyphs](https://github.com/deepseek-ai/deepseek-harness/blob/a0bfc2c3feee58560f75fa2813ae735466b279b1/packages/client/ui-primitives/src/icons/index.tsx) |
| Archivo | [OFL-1.1](../web/src/assets/fonts/OFL-Archivo.txt) | Upstream revision pending verification |
| Familjen Grotesk | [OFL-1.1](../web/src/assets/fonts/OFL-FamiljenGrotesk.txt) | Upstream revision pending verification |
| Geist and Geist Mono | [OFL-1.1](../web/src/assets/fonts/OFL-Geist.txt) | Upstream revisions pending verification |
| IBM Plex Mono | [OFL-1.1](../web/src/assets/fonts/OFL-IBMPlexMono.txt) | Upstream revisions pending verification |
| JetBrains Mono | [OFL-1.1](../web/src/assets/fonts/OFL-JetBrainsMono.txt) | Upstream revision pending verification |
| Shrikhand | [OFL-1.1](../web/src/assets/fonts/OFL-Shrikhand.txt) | Upstream revision pending verification |

For locked Cargo and npm dependencies, use the reproducible
[source license inventory](scripts/license-inventory/README.md) to generate
license declarations and review flags. Binaries, container images and other
packaged distributions need inventories and notices for their actual contents.
