import test from 'node:test';
import assert from 'node:assert/strict';
import {open} from './helpers.mjs';
import {executeCase} from '../../runner.mjs';

const image = 'local/api@sha256:' + 'a'.repeat(64), source = 'b'.repeat(40);

for (const scenario of [
  {name: 'successful close', error: null, closeError: false, expected: 'PASS'},
  {name: 'unknown mutation found during drain', error: 'browser_mutation_outcome_unknown', closeError: false, expected: 'FAIL'},
  {name: 'close itself fails', error: null, closeError: true, expected: 'FAIL'},
]) test(`document final evidence follows browser drain: ${scenario.name}`, async () => {
  const events = []; let closed = false;
  const browser = {
    async open() {return {page: {}, switchAM: async () => {}, close: async () => {
      events.push('close'); closed = true;
      if (scenario.closeError) throw Error('browser_close_failed');
    }};},
    async evidence() {
      events.push('evidence');
      if (closed && scenario.error) throw Error(scenario.error);
      return {schema: 'multi-am-browser-evidence-v1', real_ui: true, secrets_redacted: true,
        compiled_source: source, requests: [{am: 'a', request_id: 'earlier-receipt'}]};
    },
  };
  const row = {case_id: 'document_cleanup', required_steps: ['observe'], family: 'user'};
  const definition = {id: row.case_id, scope: 'browser', fixtures: ['browser'], steps: row.required_steps,
    async run(ctx) {await ctx.step('observe', async () => {
      await open(ctx); ctx.assert(true, 'browser_opened');
    });}};
  const result = await executeCase(definition, row, {phase: 'before_restart', iteration: 1,
    am: 'a', image, source, fixtures: {browser}});
  assert.equal(result.status, scenario.expected, 'final browser failures cannot be omitted');
  assert.deepEqual(events, ['close', 'evidence']);
  if (scenario.error) assert.equal(result.cleanup.find(row => row.label === 'retain_document_browser').error_code,
    scenario.error);
  if (scenario.closeError) assert.equal(result.cleanup.find(row => row.label === 'close_document_browser').error_code,
    'browser_close_failed');
});

