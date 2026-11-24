import {constants} from 'node:fs';
import {join, resolve} from 'node:path';
import {check} from '../scale-tests/lib/checks.mjs';
import {fields} from './recovery-contract.mjs';
import {planEffectRecovery, recoveryIntentDigest} from './effect-recovery.mjs';
import {applyPayload, bindingFor, CUSTODY_LIMITS, decodeRecord, newLedger} from './custody-format.mjs';
import {defaultCustodyIO, JOURNAL_FILE, openDirectory, openPrivateFile,
  privateStat, readBounded, sameFile, verifyContent} from './custody-storage.mjs';

/** Read a bounded prefix against an independently retained original binding.
 * Never resume or repair a writer or adopt a new binding from the journal.
 * Visible bytes, even a complete ACK, do not prove another process has fsynced
 * them. This report never mints durable commit receipts or execution authority.
 */
export async function readCustodyJournal(directory, binding, {io = defaultCustodyIO} = {}) {
  const expected = bindingFor(binding);
  let folder; let file; let ledger; let tail = null; let records = 0; let reason = null;
  try {
    const path = resolve(directory);
    folder = await openDirectory(path, io);
    const folderStat = await folder.stat();
    file = await openPrivateFile(join(path, JOURNAL_FILE), constants.O_RDONLY, io);
    const {bytes, stat, changed} = await readBounded(file, CUSTODY_LIMITS.journalBytes, true);
    let offset = 0;
    while (offset < bytes.length) {
      const end = bytes.indexOf(10, offset);
      check(end >= 0, 'custody_incomplete_tail');
      const limits = ledger?.limits ?? CUSTODY_LIMITS;
      check(records < limits.records && end + 1 - offset <= limits.recordBytes, 'custody_record_bound');
      const text = new TextDecoder('utf-8', {fatal: true}).decode(bytes.subarray(offset, end));
      const record = decodeRecord(text, records, tail);
      if (records === 0) {
        fields(record.payload, ['type', 'binding', 'limits']);
        check(record.payload.type === 'created', 'custody_header_invalid');
        const observed = newLedger(record.payload.binding, record.payload.limits);
        check(Object.entries(expected).every(([key, value]) => observed.binding[key] === value), 'custody_binding_mismatch');
        ledger = observed;
        check(bytes.length <= ledger.limits.journalBytes, 'custody_file_bound');
      } else {
        const next = structuredClone(ledger);
        applyPayload(next, record.payload);
        ledger = next;
      }
      records++; tail = record.sha256; offset = end + 1;
    }
    check(ledger, 'custody_header_missing');
    for (const effect of ledger.effects.values()) {
      for (const ref of effect.content) await verifyContent(path, ref, io);
      check(effect.ready, 'custody_intent_not_ready');
    }
    check(!changed && sameFile(stat, await file.stat()) && sameFile(stat, await io.lstat(join(path, JOURNAL_FILE))),
      'custody_file_changed');
    const currentFolder = await io.lstat(path);
    privateStat(currentFolder, true);
    check(currentFolder.dev === folderStat.dev && currentFolder.ino === folderStat.ino,
      'custody_directory_changed');
  } catch {
    // Keep the last validated prefix and its unknown effects; raw failures may
    // contain a private path or malformed body and never enter the report.
    reason = 'custody_observation_incomplete';
  } finally {
    try {if (file) await file.close();}
    catch {reason = 'custody_observation_incomplete';}
    try {if (folder) await folder.close();}
    catch {reason = 'custody_observation_incomplete';}
  }
  const effects = ledger ? [...ledger.effects.values()].map(effect => Object.freeze({
    effect_id: effect.intent.effect_id, intent_digest: recoveryIntentDigest(effect.intent),
    content_ready_recorded: effect.ready, plan: planEffectRecovery(effect.intent, effect.events),
  })) : [];
  return Object.freeze({scope: 'READ_ONLY_CUSTODY', status: reason ? 'QUARANTINED' : 'VERIFIED_PREFIX',
    binding: expected, reason, records, tail_sha256: tail, durability: 'NOT_PROVEN_BY_READ',
    complete: false, mutations_authorized: false, cleanup: 'RETAIN_CUSTODY', qualification: 'NOT_ASSESSED',
    effects: Object.freeze(effects), unresolved_effect_ids: Object.freeze(effects.map(effect => effect.effect_id))});
}
