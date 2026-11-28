// Adapt only the measured version-specific helper layout in a temporary copy.
// Every 0.165.0 runtime profile runs this adaptation and retains the strict checkpoint checks.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export function prepareProbe(destination, scripts = '.github/scripts', stableWireExclusions = true) {
  let source = readFileSync(join(scripts, 'runtime-image-probe.mjs'), 'utf8');
  const start = source.indexOf('  const aliasRoot = `${agentHome}/tmp/arg0`;');
  const end = source.indexOf("  complete('agent-home-helper-aliases');", start);
  assert(start > 0 && end > start);
  assert(source.slice(start, end).includes("['apply_patch', 'applypatch', 'codex-execve-wrapper', 'codex-linux-sandbox']"));
  source = "import { codexAliases } from './omarchy-codex-aliases.mjs';\n" + source.slice(0, start)
    + '  const aliasRoot = await codexAliases(agentHome, artifacts.codexVersion, onStage);\n' + source.slice(end);
  const exclusion = "exclusions: ['.cma/agents/ci-agent/home/tmp/arg0', '.codex/tmp/arg0']";
  assert.equal(source.split(exclusion).length, 2);
  // Historical 0.165 images required the archive path directly. New images
  // consume the stable adapter wire paths and resolve both helper directories.
  if (!stableWireExclusions) source = source.replace(exclusion,
    "exclusions: ['.cma/agents/ci-agent/home/tmp/arg0-linux', '.codex/tmp/arg0-linux']");
  writeFileSync(destination, source);
}
