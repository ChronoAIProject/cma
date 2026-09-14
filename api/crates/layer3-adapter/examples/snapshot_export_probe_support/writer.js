const assert = require('node:assert/strict');
const fs = require('node:fs');
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
(async () => {
  assert.equal(process.env.CMA_PROBE_POD_UID, process.argv[1]);
  const work = '/home/cma/export-probe';
  const ticks = [];
  for (const deadline = Date.now() + 30 * 60 * 1000; Date.now() < deadline && !fs.existsSync(`${work}/stop`);) {
    ticks.push(Date.now());
    fs.writeFileSync(`${work}/changed`, `live ${ticks.length}`);
    fs.writeFileSync(`${work}/created-later`, 'later creation');
    if (fs.existsSync(`${work}/deleted-later`)) fs.unlinkSync(`${work}/deleted-later`);
    await delay(10);
  }
  console.log(JSON.stringify({ticks}));
})().catch(() => { console.error('fixture writer failed'); process.exitCode = 1; });
