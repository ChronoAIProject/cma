import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import assert from 'node:assert/strict';

export const name = 'profile_policy::secret_scanner::tests::pinned_engine_detects_complete_rules_beyond_prefix_markers';
export const filter = `package(=cma) and binary(=cma) and test(=${name})`;
export const archive = '551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb';
export function setup(t, reuse = true) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'scanner-contract-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const scripts = join(root, '.github/scripts'), metadata = join(root, 'api/target/nextest');
  for (const dir of [scripts, metadata, join(root, 'api/crates/cma'), join(root, 'bin'), join(root, 'temp')]) mkdirSync(dir, { recursive: true });
  for (const file of ['scanner-contract.py', 'install-image-gitleaks.sh', 'backend-nextest.sh']) {
    copyFileSync(new URL(file, import.meta.url), join(scripts, file));
  }
  copyFileSync(new URL('../secret-scanner-build-tests.sh', import.meta.url), join(root, '.github/secret-scanner-build-tests.sh'));
  copyFileSync(new URL('../../api/crates/cma/Dockerfile', import.meta.url), join(root, 'api/crates/cma/Dockerfile'));
  const binary = join(root, 'api/target/cma'); writeFileSync(binary, 'synthetic test executable identity');
  const inventory = { 'rust-suites': { 'cma::bin/cma': { 'package-name': 'cma', 'binary-id': 'cma::bin/cma',
    'binary-name': 'cma', kind: 'bin', status: 'listed', 'binary-path': binary,
    testcases: { [name]: { ignored: true, 'filter-match': { status: 'matches' } } } } } };
  writeFileSync(join(metadata, 'cargo-metadata.json'), JSON.stringify({ workspace_root: join(root, 'api'), target_directory: join(root, 'api/target') }));
  writeFileSync(join(metadata, 'binaries-metadata.json'), JSON.stringify({ 'rust-build-meta': { 'target-directory': join(root, 'api/target') } }));
  const prelude = `#!${process.execPath}\nconst fs = require('node:fs'); const root = ${JSON.stringify(root)};
const args = process.argv.slice(2); const control = JSON.parse(fs.readFileSync(root + '/control.json'));
`;
  function executable(command, source) {
    const path = join(root, 'bin', command); writeFileSync(path, prelude + source); chmodSync(path, 0o700);
  }
  executable('cargo', `fs.appendFileSync(root + '/calls.jsonl', JSON.stringify({args,tool:process.env.CMA_TEST_GITLEAKS,incremental:process.env.CARGO_INCREMENTAL})+'\\n');
if(args[1]==='list'){process.stdout.write(fs.readFileSync(root+'/inventory.json')); process.exit(control.listExit||0);}
if(args[1]==='run'){
  if(!fs.existsSync(process.env.CMA_TEST_GITLEAKS)) process.exit(99);
  if(control.changeTest) fs.appendFileSync(root+'/api/target/cma','changed');
  if(control.changeEngine) fs.appendFileSync(process.env.CMA_TEST_GITLEAKS,'changed');
  if(control.changeHead) fs.writeFileSync(root+'/changed-head','b'.repeat(40));
  process.exit(control.runExit||0);
}process.exit(98);`);
  executable('curl', `fs.appendFileSync(root+'/install-calls.jsonl',JSON.stringify(args)+'\\n');
if(control.downloadFail)process.exit(19);fs.writeFileSync(args[args.indexOf('--output')+1],'synthetic archive');`);
  executable('sha256sum', `const input=fs.readFileSync(0,'utf8');
if(!args.includes('--check') || !input.startsWith(${JSON.stringify(archive)}+'  ') || control.checksumFail)process.exit(23);`);
  executable('tar', `const dir=args[args.indexOf('-C')+1];fs.copyFileSync(root+'/engine',dir+'/gitleaks');fs.chmodSync(dir+'/gitleaks',0o700);`);
  executable('git', `process.stdout.write((fs.existsSync(root+'/changed-head')?fs.readFileSync(root+'/changed-head','utf8'):'a'.repeat(40))+'\\n');`);
  writeFileSync(join(root, 'engine'), prelude + `if(args[0]==='version'){process.stdout.write(control.wrongVersion?'8.30.0\\n':'8.30.1\\n');process.exit(0);}
const input=fs.readFileSync(0,'utf8');
if(process.env.HOME || process.env.GITLEAKS_CONFIG_TOML!=='[extend]\\nuseDefault = true\\n')process.exit(91);
for(const flag of ['--redact=100','--ignore-gitleaks-allow','--gitleaks-ignore-path'])if(!args.includes(flag))process.exit(92);
if(control.engineFail)process.exit(33);
if(!input.includes('SERVICE_API_KEY=')){process.stdout.write('[]');process.exit(0);}
process.stdout.write(JSON.stringify([{StartLine:1,EndLine:1,Match:'REDACTED',Secret:control.unredacted?input:'REDACTED'}]));process.exit(42);`);
  return { root, scripts, metadata, inventory, control: {}, env: { PATH: join(root, 'bin') + ':' + process.env.PATH,
    RUNNER_TEMP: join(root, 'temp'), CI: reuse ? 'true' : 'false', CMA_TEST_GITLEAKS: '/untrusted/caller/tool',
    ...(reuse ? { CMA_NEXTEST_REUSE: '1' } : {}) } };
}
export function run(s, args = []) {
  writeFileSync(join(s.root, 'inventory.json'), typeof s.inventory === 'string' ? s.inventory : JSON.stringify(s.inventory));
  writeFileSync(join(s.root, 'control.json'), JSON.stringify(s.control));
  const result = spawnSync('python3', ['-B', join(s.scripts, 'scanner-contract.py'), ...args], { env: s.env, encoding: 'utf8', timeout: 30000 });
  assert.equal(result.error, undefined); assert.equal(result.signal, null);
  let calls = [];
  try { calls = readFileSync(join(s.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse); }
  catch (error) { assert.equal(error.code, 'ENOENT'); }
  return { ...result, calls };
}
