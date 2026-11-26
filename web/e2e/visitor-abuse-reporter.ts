import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { FullResult, TestCase, TestError, TestResult } from '@playwright/test/reporter';
import RedactedReporter from './reporter';
import { object, outsideRepository } from './environment';
import { evidenceDirectoryIsReady } from './security';
import { blocked, count, metrics, newCounts, type Counts } from './visitor-abuse-model';
import {
  evidenceHash,
  validateScenarioBinding,
} from '../../.github/scripts/visitor-abuse-evidence.mjs';

const scenarioNames = ['TC117-boundary', 'TC117-shared', 'TC117-load-controls', 'TC117-utc'];

export default class TC117Reporter extends RedactedReporter {
  private cases: { case: string; status: string; retry: number }[] = [];
  onError(error: TestError) {
    const message = error.message?.replace(/^Error: /, '');
    const reason = Object.values(blocked).find((value) => message === value);
    if (reason) process.stderr.write(`${reason}\n`);
    else super.onError(error);
  }
  onTestEnd(test: TestCase, result: TestResult) {
    this.cases.push({
      case: test.title.split(' ')[0]!,
      status: result.status,
      retry: result.retry,
    });
    super.onTestEnd(test, result);
    for (const reason of Object.values(blocked))
      if (result.errors.some((error) => error.message?.includes(reason)))
        process.stderr.write(`${reason}\n`);
  }
  onEnd(result: FullResult) {
    if (!this.cases.length) {
      super.onEnd(result);
      return;
    }
    const unqualified = { status: 'failed' as const };
    super.onEnd({ ...result, ...unqualified });
    const path = process.env.CMA_E2E_OUTPUT_DIR;
    if (!path || !evidenceDirectoryIsReady(path, process.env.CMA_E2E_EVIDENCE_RUN))
      return unqualified;
    const output = outsideRepository(path, 'CMA_E2E_OUTPUT_DIR');
    const counts = newCounts();
    const run = process.env.CMA_E2E_EVIDENCE_RUN!;
    const evidence: { case: string; file: string; sha256: string }[] = [];
    let incomplete = result.status !== 'passed';
    const seen = new Set<string>();
    for (const row of this.cases) {
      if (!scenarioNames.includes(row.case) || seen.has(row.case)) {
        incomplete = true;
        continue;
      }
      seen.add(row.case);
      if (row.status !== 'passed' || row.retry !== 0) incomplete = true;
      try {
        const name = row.case.replace('TC117-', '');
        const file = `tc117-${name}.json`;
        const bytes = readFileSync(join(output, file));
        const record = validateScenarioBinding(bytes, { run, case: name });
        evidence.push({ case: row.case, file, sha256: evidenceHash(bytes) });
        const observed = object(record.counts);
        const combined = newCounts();
        for (const key of Object.keys(counts) as (keyof Counts)[])
          combined[key] = count(counts[key] + count(observed[key]));
        Object.assign(counts, combined);
      } catch {
        incomplete = true;
      }
    }
    const full = scenarioNames.every((name) => seen.has(name));
    writeFileSync(
      join(output, 'tc117-summary.json'),
      JSON.stringify({
        schema: 'cma-tc117-summary-v1',
        campaign: 'TC-117',
        run,
        evidence,
        evidenceScope: 'Issue1485Task5Harness',
        authorityBinding: 'external_attestation_required',
        status: incomplete ? 'failed' : full ? 'blocked' : 'partial',
        scenarioStatus: incomplete ? 'failed' : full ? 'passed' : 'partial',
        fullCampaign: false,
        qualificationBlockers: [
          'authority_cleanup_acknowledgment_missing',
          'candidate_before_after_binding_missing',
          'provenance_unverified',
        ],
        cases: this.cases,
        counts,
        metrics: metrics(counts),
        provenanceSource: 'staging-authority-before-and-after-attest',
        provenanceVerified: false,
        rawResults: [...seen].map((name) => `tc117-${name.replace('TC117-', '')}.json`),
      }),
      { mode: 0o600, flag: 'wx' },
    );
    return unqualified;
  }
}
