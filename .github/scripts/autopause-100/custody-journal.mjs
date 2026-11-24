import {check, GateError} from '../scale-tests/lib/checks.mjs';
import {normalizeIntent} from './recovery-contract.mjs';
import {planEffectRecovery} from './effect-recovery.mjs';
import {applyPayload, bindingFor, contentFor, encodeRecord, eventFor, limitsFor, newLedger} from './custody-format.mjs';
import {CustodyStorage, defaultCustodyIO} from './custody-storage.mjs';

const construction = Symbol('custody-writer');

/**
 * Create-only, private custody writer for authorized synthetic fixture bytes.
 * No credentials, auth headers, network client or automatic launcher are accepted.
 * io is a trusted storage port (default: Node filesystem), injectable for faults.
 * A commit receipt proves this writer completed its synchronization protocol;
 * it cannot prove the provenance of transport facts or grant remote authority.
 */
export class CustodyJournal {
  #storage; #ledger; #sequence = 0; #tail = null; #bytes = 0;
  #busy = false; #poisoned = false; #closed = false;
  #intents = new Map(); #dispatches = new Map();
  constructor(storage, ledger, token) {
    check(token === construction, 'custody_factory_required');
    this.#storage = storage; this.#ledger = ledger;
  }

  static async create(directory, binding, {limits = {}, io = defaultCustodyIO} = {}) {
    const ledger = newLedger(bindingFor(binding), limitsFor(limits));
    let storage;
    try {
      storage = await CustodyStorage.create(directory, io);
      const writer = new CustodyJournal(storage, ledger, construction);
      await writer.#append({type: 'created', binding: ledger.binding, limits: ledger.limits});
      return writer;
    } catch {
      try {if (storage) await storage.close();}
      catch {throw new GateError('custody_create_failed');}
      throw new GateError('custody_create_failed');
    }
  }

  async #exclusive(action) {
    check(!this.#closed && !this.#poisoned && !this.#busy, 'custody_writer_unavailable');
    this.#busy = true;
    try {return await action();}
    catch {
      this.#poisoned = true;
      // Storage and callback errors may contain request bytes or private paths.
      throw new GateError('custody_write_failed');
    } finally {this.#busy = false;}
  }

  async #append(payload, effectId = null) {
    const next = structuredClone(this.#ledger);
    if (payload.type !== 'created') applyPayload(next, payload);
    const record = encodeRecord(this.#sequence, this.#tail, payload);
    const bytes = Buffer.byteLength(record.line);
    check(this.#sequence < next.limits.records && bytes <= next.limits.recordBytes
      && this.#bytes + bytes <= next.limits.journalBytes, 'custody_capacity_exhausted');
    await this.#storage.append(record.line);
    this.#ledger = next; this.#bytes += bytes; this.#tail = record.sha256;
    const receipt = Object.freeze({schema: 'autopause-custody-commit-v1',
      campaign_id: next.binding.campaign_id, effect_id: effectId,
      sequence: this.#sequence, tail_sha256: this.#tail});
    this.#sequence++;
    return receipt;
  }

  async recordIntent(value, content) {
    return this.#exclusive(async () => {
      const intent = normalizeIntent(value);
      const prepared = contentFor(intent, content, this.#ledger.limits);
      await this.#append({type: 'intent', intent, content: prepared.map(item => item.ref)}, intent.effect_id);
      for (const item of prepared) await this.#storage.content(item.ref.name, item.bytes);
      const receipt = await this.#append({type: 'ready', effect_id: intent.effect_id}, intent.effect_id);
      this.#intents.set(intent.effect_id, receipt);
      return receipt;
    });
  }

  #effect(id) {
    const effect = this.#ledger.effects.get(id);
    check(effect, 'custody_effect_missing');
    return effect;
  }

  async #fact(id, fact) {
    const event = eventFor(this.#effect(id), fact);
    const receipt = await this.#append({type: 'event', effect_id: id, event}, id);
    if (planEffectRecovery(this.#effect(id).intent, this.#effect(id).events).state === 'QUARANTINED') {
      this.#poisoned = true;
    }
    return receipt;
  }

  /** Call one explicitly supplied effect only after its dispatch is synced.
   * The callback receives the writer's original dispatch receipt and returns
   * an acknowledged/unavailable fact. A thrown callback leaves UNKNOWN and
   * poisons this writer; restart observation never repeats the callback.
   */
  async dispatch(id, intentReceipt, perform) {
    return this.#exclusive(async () => {
      check(typeof perform === 'function' && this.#intents.has(id)
        && this.#intents.get(id) === intentReceipt && !this.#dispatches.has(id), 'custody_dispatch_receipt_invalid');
      for (const effect of this.#ledger.effects.values()) {
        const state = planEffectRecovery(effect.intent, effect.events).state;
        check(!['OUTCOME_UNKNOWN', 'QUARANTINED'].includes(state), 'custody_unknown_effect');
      }
      await this.#storage.verify(this.#effect(id).content);
      const dispatchReceipt = await this.#fact(id, {type: 'dispatch_recorded'});
      this.#dispatches.set(id, dispatchReceipt);
      let outcome;
      try {outcome = await perform(dispatchReceipt);}
      catch {
        await this.#fact(id, {type: 'unavailable', reason: 'transport_failed'});
        throw new GateError('custody_effect_unknown');
      }
      const checked = eventFor(this.#effect(id), outcome);
      check(['acknowledged', 'unavailable'].includes(checked.type), 'custody_outcome_invalid');
      const outcomeReceipt = await this.#fact(id, outcome);
      return Object.freeze({dispatch: dispatchReceipt, outcome: outcomeReceipt});
    });
  }

  async recordFact(id, dispatchReceipt, fact) {
    return this.#exclusive(async () => {
      check(this.#dispatches.has(id) && this.#dispatches.get(id) === dispatchReceipt,
        'custody_dispatch_receipt_invalid');
      const checked = eventFor(this.#effect(id), fact);
      check(checked.type !== 'dispatch_recorded', 'custody_fact_invalid');
      return this.#fact(id, fact);
    });
  }

  async close() {
    check(!this.#busy, 'custody_writer_busy');
    if (this.#closed) return;
    this.#closed = true;
    try {await this.#storage.close();}
    catch {this.#poisoned = true; throw new GateError('custody_close_failed');}
  }
}
