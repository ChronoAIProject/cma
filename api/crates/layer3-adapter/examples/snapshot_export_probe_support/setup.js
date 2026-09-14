const assert = require('node:assert/strict');
const fs = require('node:fs');
const {randomBytes} = require('node:crypto');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
(async () => {
  assert.equal(process.env.CMA_PROBE_POD_UID, process.argv[1]);
  for (let attempt = 0; ; attempt++) {
    try {
      const response = await fetch('http://127.0.0.1:9090/ready');
      assert.equal(response.status, 200);
      break;
    } catch { assert(attempt < 100); await delay(100); }
  }
  const response = await fetch('http://127.0.0.1:9090/runtime/control', {
    method: 'POST', headers: {'content-type': 'application/json'}, body: process.argv[2],
  });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).t, 'install');
  const work = '/home/cma/export-probe';
  const sessions = '/home/cma/.cma/agents/agt_probe/home/sessions';
  fs.mkdirSync(work); fs.mkdirSync(sessions, {recursive: true});
  fs.writeFileSync(`${work}/changed`, 'captured content');
  fs.writeFileSync(`${work}/deleted-later`, 'captured deletion target');
  fs.writeFileSync(`${work}/payload`, randomBytes(16 * 1024 * 1024));
  fs.writeFileSync(`${sessions}/main.jsonl`, '{"payload":{"id":"probe-conversation"}}\n{"event":"captured reply"}\n');
  // This data-only campaign establishes its fixture before CSI capture.
  require('node:child_process').execFileSync('/bin/sync');
  console.log('prepared');
})().catch(() => { console.error('source preparation failed'); process.exitCode = 1; });
