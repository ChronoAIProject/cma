import {readFile, writeFile} from 'node:fs/promises';
import {resolve, join} from 'node:path';

const directory = resolve(process.argv[2]);
const result = JSON.parse(await readFile(join(directory, 'result.json'), 'utf8'));
const fixture = JSON.parse(await readFile(join(directory, 'fixture-result.json'), 'utf8'));
const observations = (await readFile(join(directory, 'observations.jsonl'), 'utf8'))
  .trim().split('\n').filter(Boolean).map(line => JSON.parse(line));
const summary = result.summary ?? {requested: result.requested, attempted: 0, passed: 0, failed: 0, resume_p95_ms: null};
const policy = observations.find(x => x.kind === 'policy');
const exception = observations.find(x => x.kind === 'always_on');
const hold = result.cycles?.find(x => x.active_turn)?.active_turn;
const verdict = result.verdict === 'PASS' && fixture.verdict === 'PASS' && summary.requested === 100
  ? 'PASS for the local 100-cycle subset' : 'NOT QUALIFIED';
const seconds = value => value == null ? 'NOT MEASURED' : (value / 1000).toFixed(3) + ' s';
const lines = [
  '# Auto-pause 100-cycle local evidence', '',
  `**${verdict}.** ${result.error ? 'Failure: `' + result.error + '`.' : ''}`, '',
  `Issue: #1547 (M-SUST-03). Branch: \`tl/cma-13b/autopause-100\`.`, '',
  `- Product source: \`${result.source}\``,
  `- API image: \`${result.image}\``,
  `- Runtime images: \`${JSON.stringify(result.runtime_images)}\``,
  `- Runtime sources: \`${JSON.stringify(result.runtime_sources)}\``,
  '- Environment: owned disposable local Kind, ARM64 node with Rosetta AMD64 workloads, two real AMs, authenticated replica-set MongoDB, native Sandbox controller and CSI home volumes.',
  '- Model and identity providers: local deterministic dependencies; AM, runtime, public HTTP routes, lifecycle persistence and storage are real.',
  `- Window (UTC): ${result.start} to ${result.end}`, '',
  '| Measurement | Result |', '|---|---|',
  `| Requested / attempted / passed / failed cycles | ${summary.requested} / ${summary.attempted} / ${summary.passed} / ${summary.failed} |`,
  `| Message-to-active-provider resume p95 | ${seconds(summary.resume_p95_ms)}; target ≤30 s |`,
  `| Default / accelerated idle setting | ${policy ? policy.default_seconds + ' s / ' + policy.accelerated_seconds + ' s' : 'NOT VERIFIED'} |`,
  `| Admin Always-on hold | ${exception ? exception.verdict + ', ' + seconds(exception.hold_ms) + ', ' + exception.samples + ' samples' : 'NOT VERIFIED'} |`,
  `| Active-turn protection | ${hold ? seconds(hold.hold_ms) + ', ' + hold.samples + ' samples, interrupted=' + hold.interrupted : 'NOT VERIFIED'} |`,
  `| Fixture processes joined | ${fixture.children_joined} |`, '',
  'Each completed cycle waits for natural automatic pause and zero runtime Pods, then sends one real message without a manual resume call. It compares original Sandbox/native identity, home/PVC UID and volume, sentinel bytes/hash, admission receipt, and original response receipt/output hash. All response receipts are checked again after the final cycle. Provider count must remain exactly one per message.', '',
  'Resume timing starts before the message is sent and ends when the same response is active and the runtime has reached the held provider. The 90-second active-turn hold is excluded. Percentiles use nearest rank and failed attempts are retained; samples are never silently replaced.', '',
  `Raw results: \`${directory}\` (result.json, observations.jsonl, provider observations, fixture-result.json and private diagnostic logs). Credentials stay in private local fixture files and are not included in this report.`, '',
  'Reproduce with `.github/scripts/autopause-100/launch.py --ready <owned-local-ready.json> --output <new-private-directory> --cycles 100`, then `node .github/scripts/autopause-100/report.mjs <directory> <AUTOPAUSE-100.md>`. The ready manifest pins the source, images, owned Kind context, TLS test bundle and local deterministic provider. Unit checks: `node --test .github/scripts/autopause-100/engine.test.mjs`.', '',
  'This local task does not establish the full public-release gate. The 48-hour canary, production enablement/coverage, browser resume UI, desktop/day-boundary cases, capacity and cost evidence remain outside this run. No production access, operator pod exec or Secret resource reads are used.', '',
];
await writeFile(process.argv[3] ?? join(directory, 'AUTOPAUSE-100.md'), lines.join('\n'));
