import test from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import {execFileSync} from 'node:child_process';
import {join} from 'node:path';
import {hash} from '../scale-tests/lib/checks.mjs';
import {CustodyJournal} from './custody-journal.mjs';
import {readCustodyJournal} from './custody-reader.mjs';
import {CUSTODY_LIMITS, encodeRecord} from './custody-format.mjs';
import {binding, fixture, temporary} from './fixtures/custody.mjs';

async function prepared(directory, operation = 'response_input') {
  const value = fixture('effect_fixture', operation);
  const writer = await CustodyJournal.create(directory, binding);
  const receipt = await writer.recordIntent(value.intent, value.content);
  await writer.close();
  return {value, receipt};
}

test('reader retains the verified prefix on partial, corrupt, foreign-schema and noncanonical tails', async () => {
  const tails = ['{"partial":', '{not json}\n', '\n', Buffer.from([0xff, 10])];
  for (const tail of tails) await temporary(async directory => {
    const {receipt} = await prepared(directory);
    await fs.appendFile(join(directory, 'custody.jsonl'), tail);
    const view = await readCustodyJournal(directory, binding);
    assert.equal(view.status, 'QUARANTINED'); assert.equal(view.records, 3);
    assert.equal(view.tail_sha256, receipt.tail_sha256);
    assert.deepEqual(view.unresolved_effect_ids, ['effect_fixture']);
    assert.equal(view.cleanup, 'RETAIN_CUSTODY');
  });
  for (const mutation of ['schema', 'sequence', 'hash', 'previous', 'duplicate_key', 'extra', 'whitespace']) {
    await temporary(async directory => {
      await prepared(directory);
      const path = join(directory, 'custody.jsonl');
      const lines = (await fs.readFile(path, 'utf8')).trimEnd().split('\n');
      const record = JSON.parse(lines[2]);
      if (mutation === 'schema') record.schema = 'migration-agent-journal-v1';
      if (mutation === 'sequence') record.sequence = 8;
      if (mutation === 'hash') record.sha256 = hash('foreign');
      if (mutation === 'previous') record.previous = hash('foreign');
      if (mutation === 'extra') record.private = 'synthetic-private-canary';
      lines[2] = JSON.stringify(record);
      if (mutation === 'duplicate_key') lines[2] = lines[2].replace('"sequence":2', '"sequence":2,"sequence":2');
      if (mutation === 'whitespace') lines[2] += ' ';
      await fs.writeFile(path, lines.join('\n') + '\n');
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.status, 'QUARANTINED'); assert.equal(view.records, 2);
      assert.deepEqual(view.unresolved_effect_ids, ['effect_fixture']);
      assert.equal(JSON.stringify(view).includes('synthetic-private-canary'), false);
    });
  }
});

test('missing, changed, oversized, linked and permissive reference files preserve custody', async () => {
  for (const mutation of ['missing', 'changed', 'oversize', 'symlink', 'hardlink', 'permissions', 'fifo']) {
    await temporary(async (directory, parent) => {
      await prepared(directory);
      const body = join(directory, 'effect_fixture.body');
      if (mutation === 'missing') await fs.unlink(body);
      if (mutation === 'changed') await fs.writeFile(body, Buffer.alloc((await fs.stat(body)).size, 120));
      if (mutation === 'oversize') await fs.truncate(body, CUSTODY_LIMITS.contentBytes + 1);
      if (mutation === 'symlink') {await fs.rename(body, join(parent, 'foreign')); await fs.symlink(join(parent, 'foreign'), body);}
      if (mutation === 'hardlink') await fs.link(body, join(parent, 'linked'));
      if (mutation === 'permissions') await fs.chmod(body, 0o644);
      if (mutation === 'fifo') {await fs.unlink(body); execFileSync('mkfifo', ['-m', '600', body]);}
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.status, 'QUARANTINED');
      assert.deepEqual(view.unresolved_effect_ids, ['effect_fixture']);
      assert.equal(view.mutations_authorized, false);
    });
  }
});

test('reader rejects unsafe roots and journal bounds before consuming unbounded data', async () => {
  for (const mutation of ['root_permissions', 'root_symlink', 'journal_permissions', 'journal_oversize', 'missing_header']) {
    await temporary(async (directory, parent) => {
      await prepared(directory); let target = directory;
      const path = join(directory, 'custody.jsonl');
      if (mutation === 'root_permissions') await fs.chmod(directory, 0o755);
      if (mutation === 'root_symlink') {target = join(parent, 'alias'); await fs.symlink(directory, target);}
      if (mutation === 'journal_permissions') await fs.chmod(path, 0o644);
      if (mutation === 'journal_oversize') await fs.truncate(path, CUSTODY_LIMITS.journalBytes + 1);
      if (mutation === 'missing_header') await fs.truncate(path, 0);
      const view = await readCustodyJournal(target, binding);
      assert.equal(view.status, 'QUARANTINED'); assert.equal(view.records, 0);
      assert.equal(view.complete, false); assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
    });
  }
});

test('a live append cannot erase the already checked original prefix or make it durable', async () => {
  await temporary(async directory => {
    await prepared(directory); let changed = false;
    const io = {...fs, async open(path, ...args) {
      const handle = await fs.open(path, ...args);
      return {stat: () => handle.stat(), close: () => handle.close(), async read(...values) {
        const result = await handle.read(...values);
        if (String(path).endsWith('custody.jsonl') && !changed) {
          changed = true; await fs.appendFile(path, '{unfinished');
        }
        return result;
      }};
    }};
    const view = await readCustodyJournal(directory, binding, {io});
    assert.equal(changed, true); assert.equal(view.status, 'QUARANTINED');
    assert.equal(view.records, 3); assert.deepEqual(view.unresolved_effect_ids, ['effect_fixture']);
    assert.equal(view.durability, 'NOT_PROVEN_BY_READ');
  });
});

test('record capacity distinguishes refusal before dispatch from unknown after execution', async () => {
  for (const records of [3, 4]) await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding, {limits: {records}});
    const value = fixture(); let calls = 0;
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, async () => {calls++; return value.ack;}),
        /custody_write_failed/);
      assert.equal(calls, records === 3 ? 0 : 1);
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.records, records);
      assert.equal(view.effects[0].plan.state, records === 3 ? 'NO_DISPATCH_RECORDED' : 'OUTCOME_UNKNOWN');
      assert.equal(view.cleanup, 'RETAIN_CUSTODY');
    } finally {await writer.close();}
  });
});

test('original bytes survive without JSON normalization and caller mutations cannot change registered content', async () => {
  await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding);
    const value = fixture('effect_fixture', 'settings_patch');
    const original = Buffer.from(value.content.body);
    try {
      const readyPromise = writer.recordIntent(value.intent, value.content);
      value.content.body.fill(32);
      const ready = await readyPromise;
      assert.deepEqual(await fs.readFile(join(directory, 'effect_fixture.body')), original);
      await writer.dispatch(value.intent.effect_id, ready, async () => value.ack);
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.status, 'VERIFIED_PREFIX');
      assert.equal(view.effects[0].plan.state, 'ACKNOWLEDGED');
    } finally {await writer.close();}
  });
  await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding); const value = fixture();
    value.content.body = Buffer.from(JSON.stringify(JSON.parse(value.content.body)));
    try {await assert.rejects(writer.recordIntent(value.intent, value.content), /custody_write_failed/);}
    finally {await writer.close();}
    assert.equal((await readCustodyJournal(directory, binding)).effects.length, 0);
  });
});

test('oversized contents and total budgets refuse before any dispatch; numeric limits never round', async () => {
  for (const limits of [{contentBytes: 4}, {totalContentBytes: 4}, {journalBytes: 700}, {recordBytes: 700}]) {
    await temporary(async directory => {
      let writer; let calls = 0; const value = fixture();
      try {
        await assert.rejects(async () => {
          writer = await CustodyJournal.create(directory, binding, {limits});
          const ready = await writer.recordIntent(value.intent, value.content);
          await writer.dispatch(value.intent.effect_id, ready, async () => {calls++; return value.ack;});
        }, /custody_(create|write)_failed/);
        assert.equal(calls, 0);
      } finally {if (writer) await writer.close();}
    });
  }
  for (const records of [0, -1, 1.1, NaN, Infinity, '3', 513, Number.MAX_SAFE_INTEGER]) {
    await temporary(async directory => {
      await assert.rejects(CustodyJournal.create(directory, binding, {limits: {records}}), /custody_limits_invalid/);
      await assert.rejects(fs.stat(directory), {code: 'ENOENT'});
    });
  }
});

test('semantic corruption is rejected even with a recomputed record checksum', async () => {
  for (const type of ['created', 'ready', 'event', 'unknown']) await temporary(async directory => {
    await prepared(directory);
    const path = join(directory, 'custody.jsonl');
    const rows = (await fs.readFile(path, 'utf8')).trimEnd().split('\n').map(JSON.parse);
    const payload = type === 'created' ? rows[0].payload : type === 'ready'
      ? {type, effect_id: 'effect_fixture'} : type === 'event'
        ? {type, effect_id: 'missing', event: {}} : {type};
    await fs.appendFile(path, encodeRecord(3, rows[2].sha256, payload).line);
    const view = await readCustodyJournal(directory, binding);
    assert.equal(view.status, 'QUARANTINED'); assert.equal(view.records, 3);
    assert.deepEqual(view.unresolved_effect_ids, ['effect_fixture']);
  });
});

test('writer refuses directory replacement before dispatch', async () => {
  await temporary(async (directory, parent) => {
    const writer = await CustodyJournal.create(directory, binding); const value = fixture();
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      await fs.rename(directory, join(parent, 'original'));
      await fs.mkdir(directory, {mode: 0o700});
      let calls = 0;
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, async () => {calls++; return value.ack;}),
        /custody_write_failed/);
      assert.equal(calls, 0);
      assert.deepEqual((await readCustodyJournal(join(parent, 'original'), binding)).unresolved_effect_ids, ['effect_fixture']);
    } finally {await writer.close();}
  });
});

test('read recovery never adopts another campaign, fixture or candidate from stored metadata', async () => {
  await temporary(async directory => {
    await prepared(directory);
    for (const key of ['campaign_id', 'fixture_sha256', 'candidate_sha256']) {
      const original = {...binding, [key]: key === 'campaign_id' ? 'another_campaign' : hash('another binding')};
      const view = await readCustodyJournal(directory, original);
      assert.equal(view.status, 'QUARANTINED'); assert.equal(view.records, 0);
      assert.deepEqual(view.binding, original); assert.equal(view.mutations_authorized, false);
    }
    await assert.rejects(readCustodyJournal(directory, {...binding, credential: 'synthetic canary'}), /recovery_fields_invalid/);
  });
});
