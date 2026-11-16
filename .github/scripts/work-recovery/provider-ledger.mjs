// A receiver ledger independent of AM processes. Never retain full keys or tokens.
import { createHash, createHmac, randomBytes } from 'node:crypto';
import { closeSync, constants, fstatSync, fsyncSync, lstatSync, openSync, readFileSync,
  unlinkSync, writeSync } from 'node:fs';
import { join } from 'node:path';

const hash = value => createHash('sha256').update(value).digest('hex');
const kinds = new Set(['mint_received', 'mint_refused', 'mint_accepted',
  'preparation_held', 'acceptance_held', 'response_held', 'response_dropped',
  'response_sent', 'key_revoked', 'fixture_closed']);

function validEvent(kind, data) {
  const required = kind === 'fixture_closed' ? [] : kind === 'mint_accepted'
    ? ['request', 'name', 'key_id'] : kind === 'key_revoked' ? ['request', 'key_id'] : ['request'];
  return kinds.has(kind) && required.every(key => Object.hasOwn(data, key))
    && Object.keys(data).every(key => ['request', 'name', 'key_id'].includes(key))
    && (data.request === undefined || (Number.isSafeInteger(data.request) && data.request >= 1))
    && (data.name === undefined || /^cma:work-recovery-fixture:mint_[a-f0-9]{32}$/.test(data.name))
    && (data.key_id === undefined || /^fixture-key-[a-f0-9]{32}$/.test(data.key_id));
}

export function openLedger(root) {
  const directory = lstatSync(root);
  if (!directory.isDirectory() || directory.isSymbolicLink() || (directory.mode & 0o077)) {
    throw new Error('private_fixture_directory_required');
  }
  const lockPath = join(root, 'provider.lock');
  const lock = openSync(lockPath, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY, 0o600);
  const lockIdentity = fstatSync(lock);
  let fd;
  const rows = [];
  let closed = false;
  try {
    fd = openSync(join(root, 'provider.jsonl'), constants.O_CREAT | constants.O_APPEND
      | constants.O_RDWR | constants.O_NOFOLLOW, 0o600);
    const stat = fstatSync(fd);
    if (!stat.isFile() || (stat.mode & 0o077) || stat.size > 4 * 1024 * 1024) {
      throw new Error('invalid_fixture_ledger');
    }
    const content = readFileSync(fd, 'utf8');
    if (content && !content.endsWith('\n')) throw new Error('incomplete_fixture_ledger');
    for (const line of content.split('\n').filter(Boolean)) {
      const { digest, ...row } = JSON.parse(line);
      const { sequence, previous, incarnation, monotonic_ns, kind, ...data } = row;
      if (sequence !== rows.length + 1 || previous !== (rows.at(-1)?.digest ?? null)
        || !/^[a-f0-9]{32}$/.test(incarnation) || !/^[0-9]{1,20}$/.test(monotonic_ns)
        || !validEvent(kind, data) || digest !== hash(JSON.stringify(row))) {
        throw new Error('invalid_fixture_ledger');
      }
      rows.push({ ...row, digest });
    }
  } catch (error) {
    if (fd !== undefined) closeSync(fd);
    closeSync(lock); unlinkSync(lockPath);
    throw error;
  }
  const incarnation = randomBytes(16).toString('hex');
  const salt = randomBytes(32);
  const joinHash = (domain, value) => createHmac('sha256', salt)
    .update(JSON.stringify([domain, value])).digest('hex');
  const keys = new Map();
  for (const row of rows) {
    if (row.kind === 'mint_accepted') keys.set(row.key_id, { id: row.key_id, name: row.name });
    if (row.kind === 'key_revoked') keys.delete(row.key_id);
  }
  function append(kind, data = {}) {
    if (closed || !kinds.has(kind) || rows.length >= 4096) throw new Error('fixture_ledger_closed_or_full');
    // Callers supply only synthetic metadata through this allowlist. Raw
    // requests, responses, credentials and exception messages cannot enter it.
    if (!validEvent(kind, data)) throw new Error('invalid_fixture_event');
    const row = { sequence: rows.length + 1, previous: rows.at(-1)?.digest ?? null,
      incarnation, monotonic_ns: process.hrtime.bigint().toString(), kind, ...data };
    const event = { ...row, digest: hash(JSON.stringify(row)) };
    const bytes = Buffer.from(JSON.stringify(event) + '\n');
    let written = 0;
    while (written < bytes.length) {
      const count = writeSync(fd, bytes, written);
      if (count === 0) throw new Error('fixture_ledger_short_write');
      written += count;
    }
    fsyncSync(fd);
    rows.push(event);
    if (kind === 'mint_accepted') keys.set(data.key_id, { id: data.key_id, name: data.name });
    if (kind === 'key_revoked') keys.delete(data.key_id);
    return event.sequence;
  }
  return {
    append,
    nextRequest: () => Math.max(0, ...rows.map(row => row.request ?? 0)) + 1,
    keys: () => [...keys.values()].map(row => ({ ...row })),
    snapshot: () => ({ schema: 'ordinary-work-provider-ledger-v1',
      attempted_sends: null,
      received_mint_posts: rows.filter(row => row.kind === 'mint_received').length,
      accepted_calls: rows.filter(row => row.kind === 'mint_accepted').length,
      distinct_effects: new Set(rows.filter(row => row.kind === 'mint_accepted').map(row => row.key_id)).size,
      physical_commits: rows.filter(row => row.kind === 'mint_accepted').length,
      replays: 0, refusals: rows.filter(row => row.kind === 'mint_refused').length,
      events: rows.map(({ name, key_id, ...row }) => ({ ...row,
        operation_hash: name ? joinHash('operation', name) : null,
        effect_hash: key_id ? joinHash('effect', key_id) : null })),
      join_scope: 'one fixture incarnation; fresh in-memory HMAC salt on reopen',
      boundary: 'synthetic metadata-only provider; received POSTs are not client-side send observations',
    }),
    close() {
      if (closed) return;
      closed = true;
      closeSync(fd); closeSync(lock);
      const current = lstatSync(lockPath);
      if (current.ino !== lockIdentity.ino || current.dev !== lockIdentity.dev) {
        throw new Error('fixture_lock_changed');
      }
      unlinkSync(lockPath);
    },
  };
}
