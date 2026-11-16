import {readFile, writeFile, appendFile} from 'node:fs/promises';
import {join} from 'node:path';
import {campaign} from './engine.mjs';
import {PauseAdapter} from './http.mjs';

const config = JSON.parse(await readFile(process.argv[2], 'utf8'));
const output = config.output_dir;
const observations = [];
const observe = value => observations.push({at: new Date().toISOString(), ...value});
const adapter = new PauseAdapter(config, observe);
const result = {source: config.api_source, image: config.expected_api_image,
  runtime_images: config.runtime_images, runtime_sources: config.runtime_sources,
  start: new Date().toISOString(), requested: config.cycles ?? 100, verdict: 'RUNNING'};
async function save() {
  await writeFile(join(output, 'result.json'), JSON.stringify(result, null, 2) + '\n', {mode: 0o600});
  if (observations.length) await appendFile(join(output, 'observations.jsonl'),
    observations.splice(0).map(x => JSON.stringify(x) + '\n').join(''), {mode: 0o600});
}
try {
  await save();
  Object.assign(result, await campaign(adapter, {count: result.requested,
    record: async progress => {Object.assign(result, progress); await save();
      console.log(JSON.stringify({cycle: progress.cycles.length, ...progress.summary}));}}));
  result.verdict = result.summary.verdict;
} catch (error) {
  result.verdict = 'FAIL'; result.error = error.message;
  if (error.response) await writeFile(join(output, 'error-response.private.json'),
    JSON.stringify(error.response, null, 2), {mode: 0o600});
} finally {
  result.cleanup_errors = await adapter.cleanup();
  if (result.cleanup_errors.length) result.verdict = 'FAIL';
  result.end = new Date().toISOString(); await save();
}
process.exitCode = result.verdict === 'PASS' ? 0 : 1;
