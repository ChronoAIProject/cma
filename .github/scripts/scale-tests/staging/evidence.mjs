import { openSync, writeSync, fsyncSync, closeSync } from 'node:fs';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { check, hash, isHash } from '../lib/checks.mjs';
import { catalogSha256 } from '../lib/catalog.mjs';
import { outputDirectory } from './authority.mjs';

const counts = new Set(['slot', 'elapsedMs', 'ackMs', 'streamFirstDeltaMs', 'browserFirstVisibleMs', 'terminalMs',
  'offered', 'admitted', 'completed', 'settledTurns', 'overdue', 'unresolved', 'started', 'failed', 'open', 'streamOpened', 'streamClosed']);
const flags = new Set(['ok', 'logicalCleanup', 'physicalCleanup', 'cleanup', 'settled']);
const statuses = new Set(['PASS', 'FAIL', 'BLOCKED', 'NOT_RUN', 'issued', 'acknowledged', 'uncertain', 'terminal']);
export function safeRow(value) {
  const row = {};
  for (const [key, item] of Object.entries(value)) {
    if (key.endsWith('Hash')) { check(isHash(item), 'invalid_evidence_hash'); row[key] = item; }
    else if (counts.has(key)) { check(Number.isFinite(item) && item >= 0, 'invalid_evidence_count'); row[key] = item; }
    else if (flags.has(key)) { check(typeof item === 'boolean', 'invalid_evidence_flag'); row[key] = item; }
    else if (key === 'status') { check(statuses.has(item), 'invalid_evidence_status'); row[key] = item; }
    else if (['scenario', 'code'].includes(key)) {
      check(typeof item === 'string' && /^[a-z][a-z0-9_]{0,80}$/.test(item), 'invalid_evidence_code'); row[key] = item;
    } else check(false, 'unapproved_evidence_field');
  }
  return row;
}
export class Evidence {
  static async open(directory) {
    await outputDirectory(directory);
    await mkdir(directory, { mode: 0o700 });
    return new Evidence(directory);
  }
  constructor(directory) {
    this.directory = directory; this.rows = []; this.bytes = '';
    this.fd = openSync(join(directory, 'samples.jsonl'), 'wx', 0o600);
  }
  record(value) {
    const row = { sequence: this.rows.length, at: new Date().toISOString(), ...safeRow(value) };
    const bytes = JSON.stringify(row) + '\n';
    writeSync(this.fd, bytes); fsyncSync(this.fd);
    this.bytes += bytes; this.rows.push(row);
    return row;
  }
  async finish(meta) {
    closeSync(this.fd);
    const report = { schema: 'cma-scale-results-v1', environment: 'staging', selection: 'staging_adapter',
      status: 'BLOCKED', code: 'adapter_evidence_not_campaign_qualification', catalog_sha256: catalogSha256,
      ...safeRow(meta), scenarios: this.rows };
    // Operational success is separate from qualification and physical release.
    report.status = 'BLOCKED';
    const bytes = JSON.stringify(report, null, 2) + '\n';
    await writeFile(join(this.directory, 'results.json'), bytes, { mode: 0o600, flag: 'wx' });
    await writeFile(join(this.directory, 'manifest.json'), JSON.stringify({ schema: 'cma-scale-manifest-v1',
      files: [{ file: 'samples.jsonl', sha256: hash(this.bytes) }, { file: 'results.json', sha256: hash(bytes) }] }) + '\n',
    { mode: 0o600, flag: 'wx' });
    return report;
  }
}
