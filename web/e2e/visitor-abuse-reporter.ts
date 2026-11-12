import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { FullResult, TestCase, TestResult } from '@playwright/test/reporter';
import RedactedReporter from './reporter';
import { outsideRepository } from './environment';
import { evidenceDirectoryIsReady } from './security';
import { blocked, metrics, newCounts, type Counts } from './visitor-abuse-model';

export default class TC117Reporter extends RedactedReporter {
  private cases: { case: string; status: string; retry: number }[] = [];
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
    super.onEnd(result);
    const path = process.env.CMA_E2E_OUTPUT_DIR;
    if (!path || !evidenceDirectoryIsReady(path, process.env.CMA_E2E_EVIDENCE_RUN)) return;
    const output = outsideRepository(path, 'CMA_E2E_OUTPUT_DIR');
    const counts = newCounts();
    let incomplete = result.status !== 'passed';
    for (const row of this.cases) {
      if (row.status !== 'passed' || row.retry !== 0) incomplete = true;
      try {
        const evidence = JSON.parse(
          readFileSync(join(output, `tc117-${row.case.replace('TC117-', '')}.json`), 'utf8'),
        ) as { counts: Counts; status: string };
        if (evidence.status !== 'passed') incomplete = true;
        for (const key of Object.keys(counts) as (keyof Counts)[])
          counts[key] += evidence.counts[key];
      } catch {
        incomplete = true;
      }
    }
    const full = ['TC117-boundary', 'TC117-shared', 'TC117-load-controls', 'TC117-utc'].every(
      (name) => this.cases.some((row) => row.case === name),
    );
    writeFileSync(
      join(output, 'tc117-summary.json'),
      JSON.stringify({
        campaign: 'TC-117',
        evidenceScope: 'Issue1485Task5Harness',
        authorityBinding: 'external_attestation_required',
        status: incomplete ? 'failed' : full ? 'passed' : 'partial',
        fullCampaign: full && !incomplete,
        cases: this.cases,
        counts,
        metrics: metrics(counts),
        provenanceSource: 'staging-authority-before-and-after-attest',
        provenanceVerified: false,
        rawResults: this.cases.map((row) => `tc117-${row.case.replace('TC117-', '')}.json`),
      }),
      { mode: 0o600, flag: 'wx' },
    );
    return incomplete ? { status: 'failed' as const } : undefined;
  }
}
