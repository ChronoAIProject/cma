import test from 'node:test';
import assert from 'node:assert/strict';
import {activityTable} from './activity-browser.mjs';
import {Blocked} from '../../runner.mjs';

const records = () => Array.from({length: 51}, (_, index) => ({
  id: 'audit-' + index, actor: 'synthetic-operator', actor_name: 'Synthetic operator',
  target: {kind: 'resource', id: 'target-' + index}, action: 'delete',
  at: '2026-11-16T00:00:00Z', outcome: {status: ['succeeded', 'refused', 'failed'][index % 3]},
}));

async function runUntilBrowser(rows) {
  let opened = 0;
  const fixture = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, cleanup: async () => {},
    records: rows, actor: 'synthetic-operator', actorLabel: 'Synthetic operator',
    action: 'delete', actionLabel: 'Delete'};
  const ctx = {am: 'a', defer() {}, fixture: name => name === 'admin-fixtures'
    ? {take: async () => fixture}
    : {open: async () => {opened++; throw Error('browser_setup_reached');}}};
  let failure;
  try {await activityTable.run(ctx);} catch (error) {failure = error;}
  return {failure, opened};
}

test('activity cannot execute without a missing-name record on its visible page', async () => {
  for (const missingIndex of [null, 50]) {
    const rows = records();
    if (missingIndex !== null) delete rows[missingIndex].actor_name;
    const result = await runUntilBrowser(rows);
    assert.ok(result.failure instanceof Blocked);
    assert.equal(result.failure.message, 'admin_activity_missing_name_fixture');
    assert.equal(result.opened, 0);
  }
});

test('the activity fixture still requires each outcome on the visible page', async () => {
  for (const excluded of ['succeeded', 'refused', 'failed']) {
    const rows = records(); rows[0].actor_name = null;
    for (const row of rows.slice(0, 50)) if (row.outcome.status === excluded) {
      row.outcome.status = excluded === 'succeeded' ? 'refused' : 'succeeded';
    }
    const result = await runUntilBrowser(rows);
    assert.ok(result.failure instanceof Blocked);
    assert.equal(result.failure.message, 'admin_activity_outcome_fixture_missing');
    assert.equal(result.opened, 0);
  }
});

test('a complete activity cohort reaches the actual browser for missing and blank names', async () => {
  for (const name of [undefined, null, '', '   ']) {
    const rows = records(); rows[10].actor_name = name;
    const result = await runUntilBrowser(rows);
    assert.equal(result.failure.message, 'browser_setup_reached');
    assert.equal(result.opened, 1);
  }
});

test('the display assertions include a separate unnamed record beyond the outcome samples', async () => {
  const rows = records(); rows[10].actor_name = null;
  const names = [];
  const table = {getByRole: () => ({filter({hasText}) {
    const record = rows.find(row => row.target.id === hasText);
    assert.ok(record);
    return {getByText(value) {names.push({target: hasText, value}); return {isVisible: async () => true};},
      locator: () => ({getAttribute: async () => record.at, textContent: async () =>
        new Date(record.at).toLocaleString('en-SG', {
          timeZone: 'Asia/Singapore', dateStyle: 'medium', timeStyle: 'short',
        })})};
  }})};
  const fixture = {scope: 'LOCAL_DISPOSABLE_TWO_AM', createdByRun: true, cleanup: async () => {},
    records: rows, actor: 'synthetic-operator', actorLabel: 'Synthetic operator',
    action: 'delete', actionLabel: 'Delete'};
  // Offline locator contract only; no browser or feature acceptance is run.
  const ctx = {am: 'a', defer() {}, assert: (value, message) => assert.ok(value, message),
    step: async (name, run) => {if (name === 'step_2') await run();},
    fixture: name => name === 'admin-fixtures' ? {take: async () => fixture} : {
      open: async () => ({page: {getByRole: () => table}, origin: 'http://127.0.0.1:19001',
        switchAM: async () => {}, close: async () => {}}),
    }};
  await activityTable.run(ctx);
  assert.ok(names.some(row => row.target === rows[10].target.id && row.value === 'Name unavailable'));
  assert.equal(names.filter(row => row.value === 'Name unavailable').length, 1);
});
