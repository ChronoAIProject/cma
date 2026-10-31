// Adapt only the measured version-specific helper layout in a temporary copy.
// The normal runtime probe and all default CI gates remain unchanged.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

export function prepareProbe(destination) {
  let source = readFileSync('.github/scripts/runtime-image-probe.mjs', 'utf8');
  const start = source.indexOf('  const aliasRoot = `${agentHome}/tmp/arg0`;');
  const end = source.indexOf("  complete('agent-home-helper-aliases');", start);
  assert(start > 0 && end > start);
  assert(source.slice(start, end).includes("['apply_patch', 'applypatch', 'codex-execve-wrapper', 'codex-linux-sandbox']"));
  source = "import { codexAliases } from './omarchy-codex-aliases.mjs';\n" + source.slice(0, start)
    + '  const aliasRoot = await codexAliases(agentHome, artifacts.codexVersion);\n' + source.slice(end);
  const exclusion = "exclusions: ['.cma/agents/ci-agent/home/tmp/arg0']";
  assert.equal(source.split(exclusion).length, 2);
  source = source.replace(exclusion, "exclusions: ['.cma/agents/ci-agent/home/tmp/arg0-linux']");
  writeFileSync(destination, source);
}
