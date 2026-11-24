import test from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import {join} from 'node:path';
import {hash} from '../scale-tests/lib/checks.mjs';
import {CustodyJournal} from './custody-journal.mjs';
import {readCustodyJournal} from './custody-reader.mjs';
import {CUSTODY_LIMITS, decodeRecord, encodeRecord, limitsFor} from './custody-format.mjs';
import {privateStat, readBounded, verifyContent} from './custody-storage.mjs';
import {binding, fixture, observedIO, temporary} from './fixtures/custody.mjs';

test('effect/content boundaries accept exact limits and refuse duplicates, overflow and foreign binding', async () => {
  await temporary(async directory => {
    const value = fixture(); value.content.body = Buffer.alloc(CUSTODY_LIMITS.contentBytes, 120);
    value.intent.request_sha256 = hash(value.content.body);
    const total = value.content.body.length + value.content.key.length;
    const writer = await CustodyJournal.create(directory, binding, {limits: {effects: 1, totalContentBytes: total}});
    try {
      await writer.recordIntent(value.intent, value.content);
      assert.equal((await readCustodyJournal(directory, binding)).status, 'VERIFIED_PREFIX');
      const next = fixture('next');
      await assert.rejects(writer.recordIntent(next.intent, next.content), /custody_write_failed/);
    } finally {await writer.close();}
  });
  for (const mode of ['duplicate', 'binding', 'extra_content', 'oversize', 'bad_key', 'missing_content']) {
    await temporary(async directory => {
      const writer = await CustodyJournal.create(directory, binding); const value = fixture();
      try {
        if (mode === 'duplicate') await writer.recordIntent(value.intent, value.content);
        if (mode === 'binding') value.intent.candidate_sha256 = hash('different candidate');
        if (mode === 'extra_content') value.content.credential = Buffer.from('synthetic private canary');
        if (mode === 'oversize') {value.content.body = Buffer.alloc(CUSTODY_LIMITS.contentBytes + 1); value.intent.request_sha256 = hash(value.content.body);}
        if (mode === 'bad_key') {value.content.key = Buffer.from('key\nnewline'); value.intent.key_sha256 = hash(value.content.key);}
        if (mode === 'missing_content') delete value.content.body;
        await assert.rejects(writer.recordIntent(value.intent, value.content), {message: 'custody_write_failed'});
      } finally {await writer.close();}
    });
  }
  assert.throws(() => limitsFor({unknown: 1}), /recovery_fields_invalid/);
  assert.throws(() => limitsFor(null), /custody_limits_invalid/);
});

test('recordFact requires its original dispatch receipt and rejects supplied sequence/binding', async () => {
  for (const mode of ['clone', 'cross_effect', 'sequence', 'binding', 'redispatch', 'accessor']) {
    await temporary(async directory => {
      const writer = await CustodyJournal.create(directory, binding); const value = fixture('first');
      const next = fixture('second'); let getterCalls = 0;
      try {
        const ready = await writer.recordIntent(value.intent, value.content);
        const otherReady = await writer.recordIntent(next.intent, next.content);
        const first = await writer.dispatch('first', ready, async () => value.ack);
        const second = await writer.dispatch('second', otherReady, async () => next.ack);
        const receipt = mode === 'clone' ? {...first.dispatch} : mode === 'cross_effect' ? second.dispatch : first.dispatch;
        const fact = {...value.ack};
        if (mode === 'sequence') fact.sequence = 1;
        if (mode === 'binding') fact.intent_digest = hash('different');
        if (mode === 'redispatch') {delete fact.source; delete fact.receipt; fact.type = 'dispatch_recorded';}
        if (mode === 'accessor') Object.defineProperty(fact, 'type', {get() {getterCalls++; return 'acknowledged';}});
        await assert.rejects(writer.recordFact('first', receipt, fact), {message: 'custody_write_failed'});
        assert.equal(getterCalls, 0);
        const view = await readCustodyJournal(directory, binding);
        assert.equal(view.effects.length, 2); assert.equal(view.cleanup, 'RETAIN_CUSTODY');
      } finally {await writer.close();}
    });
  }
});

test('content and journal changes after ready stop the callback before dispatch', async () => {
  for (const mode of ['content', 'journal_truncated', 'journal_replaced']) await temporary(async directory => {
    const writer = await CustodyJournal.create(directory, binding); const value = fixture(); let calls = 0;
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      if (mode === 'content') await fs.writeFile(join(directory, 'effect_fixture.body'), 'changed');
      if (mode === 'journal_truncated') await fs.truncate(join(directory, 'custody.jsonl'), 0);
      if (mode === 'journal_replaced') {
        await fs.unlink(join(directory, 'custody.jsonl'));
        await fs.writeFile(join(directory, 'custody.jsonl'), '', {mode: 0o600});
      }
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, async () => {calls++; return value.ack;}),
        {message: 'custody_write_failed'});
      assert.equal(calls, 0);
    } finally {await writer.close();}
  });
});

test('foreign ownership, non-files and short/changing reads refuse with bounded allocation', async () => {
  await temporary(async (directory, parent) => {
    const stat = await fs.stat(parent);
    const foreign = Object.assign(Object.create(stat), {uid: process.getuid() + 1});
    assert.throws(() => privateStat(foreign, true), /custody_private_path_required/);
    assert.throws(() => privateStat(stat), /custody_private_path_required/);
    await fs.writeFile(join(parent, 'bytes'), 'fixture', {mode: 0o600});
    const file = await fs.open(join(parent, 'bytes'), 'r');
    try {
      const short = {stat: () => file.stat(), read: async () => ({bytesRead: 0})};
      await assert.rejects(readBounded(short, 7), /custody_file_changed/);
      const observed = await readBounded(short, 7, true);
      assert.equal(observed.changed, true); assert.equal(observed.bytes.length, 0);
      const unchanged = await readBounded(file, 7);
      assert.equal(unchanged.bytes.toString(), 'fixture'); assert.equal(unchanged.changed, false);
    } finally {await file.close();}
    await fs.writeFile(join(parent, 'bad.key'), 'bad\nkey', {mode: 0o600});
    await assert.rejects(verifyContent(parent, {role: 'key', name: 'bad.key', bytes: 7, sha256: hash('bad\nkey')}, fs),
      /custody_key_invalid/);
  });
});

test('reader refuses record/content shape changes and unknown header versions without tail repair', async () => {
  for (const mode of ['record_bound', 'content_ref', 'intent_not_ready', 'header_type', 'header_limits']) {
    await temporary(async directory => {
      const writer = await CustodyJournal.create(directory, binding); const value = fixture();
      await writer.recordIntent(value.intent, value.content); await writer.close();
      const path = join(directory, 'custody.jsonl');
      const rows = (await fs.readFile(path, 'utf8')).trimEnd().split('\n').map(JSON.parse);
      if (mode === 'record_bound') rows[0].payload.limits.recordBytes = 600;
      if (mode === 'content_ref') rows[1].payload.content[0].name = '../foreign';
      if (mode === 'intent_not_ready') rows.pop();
      if (mode === 'header_type') rows[0].payload.type = 'other';
      if (mode === 'header_limits') rows[0].payload.limits.records = 513;
      let previous = null;
      const lines = rows.map((row, index) => {
        const encoded = encodeRecord(index, previous, row.payload); previous = encoded.sha256; return encoded.line;
      }).join('');
      await fs.writeFile(path, lines);
      const view = await readCustodyJournal(directory, binding);
      assert.equal(view.status, 'QUARANTINED');
      assert.equal(await fs.readFile(path, 'utf8'), lines);
    });
  }
  assert.throws(() => decodeRecord('null', 0, null), /recovery_record_invalid/);
});

test('close failures remain typed and cannot return a healthy observation or writable handle', async () => {
  await temporary(async directory => {
    let inject = false;
    const io = {...fs, async open(...args) {
      const file = await fs.open(...args);
      return new Proxy(file, {get(target, key) {
        if (key === 'close') return async () => {await file.close(); if (inject) throw Error('private-close-canary');};
        const value = Reflect.get(target, key, target); return typeof value === 'function' ? value.bind(target) : value;
      }});
    }};
    const writer = await CustodyJournal.create(directory, binding, {io});
    const value = fixture(); await writer.recordIntent(value.intent, value.content);
    inject = true;
    await assert.rejects(writer.close(), {message: 'custody_close_failed'});
    await assert.rejects(writer.recordIntent(value.intent, value.content), /custody_writer_unavailable/);
    const view = await readCustodyJournal(directory, binding, {io});
    assert.equal(view.status, 'QUARANTINED'); assert.equal(JSON.stringify(view).includes('private-close-canary'), false);
  });
});

test('a one-time pre-dispatch failure never permits another callback on the poisoned writer', async () => {
  await temporary(async directory => {
    let failed = false; let calls = 0; const value = fixture();
    const writer = await CustodyJournal.create(directory, binding, {io: observedIO(point => {
      if (!failed && point === 'before:write:journal:dispatch_recorded') {failed = true; throw Error('one-time failure');}
    })});
    try {
      const ready = await writer.recordIntent(value.intent, value.content);
      const perform = async () => {calls++; return value.ack;};
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, perform), /custody_write_failed/);
      await assert.rejects(writer.dispatch(value.intent.effect_id, ready, perform), /custody_writer_unavailable/);
      assert.equal(calls, 0);
    } finally {await writer.close();}
  });
});

test('file size bounds run before any read or allocation proportional to an oversized file', async () => {
  await temporary(async (directory, parent) => {
    const path = join(parent, 'oversized');
    await fs.writeFile(path, '', {mode: 0o600});
    await fs.truncate(path, CUSTODY_LIMITS.journalBytes + 1);
    const file = await fs.open(path, 'r'); let reads = 0;
    try {
      const observed = {stat: () => file.stat(), read: (...args) => {reads++; return file.read(...args);}};
      await assert.rejects(readBounded(observed, CUSTODY_LIMITS.journalBytes), /custody_file_bound/);
      assert.equal(reads, 0);
    } finally {await file.close();}
  });
});
