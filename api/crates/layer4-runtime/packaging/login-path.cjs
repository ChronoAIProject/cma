// Preserve the pinned login-shell behavior when the runtime profile supplies PATH.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const profile = fs.readFileSync('/etc/profile', 'utf8');
const defaults = [
  'if [ "$(id -u)" -eq 0 ]; then',
  '  PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"',
  'else',
  '  PATH="/usr/local/bin:/usr/bin:/bin:/usr/local/games:/usr/games"',
  'fi',
  'export PATH',
].join('\n');
assert.equal(profile.split(defaults).length, 2, 'unexpected Debian login PATH defaults');
const fallback = [
  'if [ -z "${PATH:-}" ]; then',
  ...defaults.split('\n').slice(0, -1).map((line) => `  ${line.replace('$(id -u)', '$(/usr/bin/id -u)')}`),
  'fi',
  'export PATH',
].join('\n');
fs.writeFileSync('/etc/profile', profile.replace(defaults, fallback));
