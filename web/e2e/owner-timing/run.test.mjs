import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, readdir, realpath, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { parseConfig, run } from './run.mjs';
import { isReportRequest, reportObservation } from './admin.mjs';
import { managerSha, reportId, startFixture, subject, webSha } from './fixture.mjs';

const fixtureOrigin = 'http://127.0.0.1:1234';
const base = '/api/v1/admin/usage/metrics';

test('requires explicit authority, a bound account and a full candidate for measurement', () => {
  const config = {
    environment: 'fixture',
    origin: fixtureOrigin,
    mode: 'measure',
    output: '/tmp/results',
  };
  assert.throws(() => parseConfig(config), /measurement_authority_incomplete/);
  assert.throws(() => parseConfig({ ...config, attempts: 9 }), /attempts_invalid/);
  assert.throws(() => parseConfig({ ...config, environment: 'staging' }), /environment_invalid/);
  assert.throws(
    () => parseConfig({ ...config, origin: 'https://example.test/another' }),
    /origin_invalid/,
  );
});

test('binds a prepared page and a legacy page to their report identity', () => {
  const page = { report_id: reportId, items: [] };
  const body = { view: { report: { report_id: reportId } }, initial: { rows: page } };
  assert.deepEqual(reportObservation(base + '/initial/view', 'POST', 200, body), {
    id: reportId,
    count: 0,
    projected: true,
  });
  assert.equal(reportObservation(base + '/initial/view', 'POST', 202, body), null);
  assert.equal(
    reportObservation(base + '/initial/view', 'POST', 200, {
      ...body,
      initial: { rows: { ...page, report_id: 'another' } },
    }),
    null,
  );
  assert.deepEqual(
    reportObservation(base + '/reports/' + reportId + '/sandboxes', 'GET', 200, page),
    { id: reportId, count: 0, projected: false },
  );
  assert.equal(isReportRequest(base + '/initial/reports/' + reportId, 'POST'), true);
  assert.equal(isReportRequest(base + '/initial/reports/' + reportId + '/delete', 'POST'), false);
  assert.equal(isReportRequest('/api/v1/sandboxes', 'POST'), false);
});

const cookie = 'synthetic-session-fixture';

async function prepareRun(t, fixtureOptions) {
  const fixture = await startFixture(fixtureOptions);
  t.after(() => fixture.close());
  const root = await mkdtemp(join(await realpath(tmpdir()), 'owner-timing-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const storage = join(root, 'browser.json');
  await writeFile(
    storage,
    JSON.stringify({
      cookies: [
        {
          name: 'sample',
          value: cookie,
          domain: '127.0.0.1',
          path: '/',
          expires: -1,
          httpOnly: true,
          secure: false,
          sameSite: 'Lax',
        },
      ],
      origins: [],
    }),
    { mode: 0o600 },
  );
  const config = {
    environment: 'fixture',
    origin: fixture.origin,
    mode: 'measure',
    output: join(root, 'timing'),
    approve_create_cleanup: true,
    approve_admin_report: true,
    subject_sha256: createHash('sha256').update(subject).digest('hex'),
    storage_state: storage,
    web_sha: webSha,
    am_sha: managerSha,
    ...(process.env.CMA_TEST_CHROME ? { browser_executable: process.env.CMA_TEST_CHROME } : {}),
  };
  return { fixture, config, root };
}

test(
  'executes serial browser measurements and refuses a mismatched build without creating',
  { timeout: 90000 },
  async (t) => {
    const { fixture, config, root } = await prepareRun(t);
    const blocked = await run({ ...config, am_sha: webSha, output: join(root, 'blocked') });
    assert.equal(blocked.metadata.run_failure.code, 'serving_revision_mismatch');
    assert.equal(blocked.summary.create_plain.blocked, 10);
    assert.deepEqual(fixture.resources(), { current: 0, maxActive: 0, populated: false });

    const probes = await run({ ...config, mode: 'probe', output: join(root, 'probes') });
    assert.equal(probes.summary.public_build_info.completed, 10);
    assert.equal(probes.summary.signed_out_session.completed, 10);
    assert.equal(probes.summary.create_plain.blocked, 1);
    assert.equal(fixture.resources().populated, false);

    const report = await run(config);
    assert.equal(report.metadata.run_failure, undefined);
    assert.equal(report.metadata.revision_confirmed, true);
    for (const row of Object.values(report.summary)) {
      assert.equal(row.attempts, 10);
      assert.equal(row.completed, 10);
      assert.equal(row.acceptance, 'NOT_PROVEN');
      assert.equal(row.p95_ms, row.max_ms);
    }
    for (const sample of report.samples) {
      if (sample.case === 'create_plain' || sample.case === 'create_desktop') {
        assert.equal(sample.desktop_verified, sample.case === 'create_desktop');
      }
    }
    assert.deepEqual(fixture.counts, {
      plain: 10,
      desktop: 10,
      sends: 10,
      cancels: 10,
      reports: 10,
      deletes: 40,
    });
    assert.deepEqual(fixture.resources(), { current: 0, maxActive: 1, populated: true });
    assert.deepEqual(await readdir(config.output), ['results.json', 'summary.md']);
    const artifact = await readFile(join(config.output, 'results.json'), 'utf8');
    for (const forbidden of [
      cookie,
      subject,
      reportId,
      'response-fixture',
      'synthetic-session-fixture',
    ]) {
      assert.equal(artifact.includes(forbidden), false);
    }
    await assert.rejects(run(config), { code: 'EEXIST' });
  },
);

test(
  'fails plain samples that return a desktop Agent and cleans up each allocation',
  { timeout: 90000 },
  async (t) => {
    const { fixture, config } = await prepareRun(t, { plainReturnsDesktop: true });
    const report = await run(config);
    assert.equal(report.metadata.run_failure, undefined);
    assert.equal(report.summary.create_plain.completed, 0);
    assert.equal(report.summary.create_plain.failed, 10);
    assert.equal(report.summary.create_plain.acceptance, 'NOT_PROVEN');
    assert.ok(
      report.samples
        .filter((sample) => sample.case === 'create_plain')
        .every(
          (sample) => sample.outcome === 'failed' && sample.code === 'creation_variant_mismatch',
        ),
    );
    assert.equal(report.summary.create_desktop.completed, 10);
    assert.equal(report.summary.agent_open.blocked, 10);
    assert.deepEqual(fixture.counts, {
      plain: 10,
      desktop: 10,
      sends: 0,
      cancels: 0,
      reports: 10,
      deletes: 40,
    });
    assert.deepEqual(fixture.resources(), { current: 0, maxActive: 1, populated: true });
    assert.deepEqual(await readdir(config.output), ['results.json', 'summary.md']);
  },
);
