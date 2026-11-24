import {randomUUID} from 'node:crypto';
import {performance} from 'node:perf_hooks';
import {artifact, completedResponse, hash, identity, lineage, nativeObservation,
  preservedTranscript, providerObservation, readerObservation, requireValue, same,
  transcript} from './migration_agent_observations.mjs';

export const STAGES = Object.freeze(['old-write','new-read-old-write','new-write','old-read-new-write']);
const terminal = value => ['completed','failed','incomplete','cancelled'].includes(value?.status);

// One instance spans the image switches. The isolated campaign supplies the
// existing API transport and a bounded read-only native-resource observer;
// this driver never selects a cluster, accesses Mongo, or executes in a pod.
export class RetainedAgentJourney {
  #artifacts; #index = 0; #failed = false; #active = false; #records = []; #readers = [];
  #api; #adapter; #observe; #now; #sleep; #uuid; #timeout;
  #signal;
  constructor({artifacts, api, adapter, observe, timeout = 240000,
    now = () => performance.now(), sleep = ms => new Promise(resolve => setTimeout(resolve,ms)),
    uuid = randomUUID, signal}) {
    this.#artifacts = Object.fromEntries(['old','new','runtime'].map(k => [k,artifact(artifacts?.[k])]));
    requireValue(this.#artifacts.old.image !== this.#artifacts.new.image, 'distinct_readers_required');
    for (const method of ['request','send']) requireValue(typeof api?.[method] === 'function', 'api_' + method + '_required');
    for (const method of ['reader','runtimeFor','prepareTurn','releaseTurn','oracle']) {
      requireValue(typeof adapter?.[method] === 'function', 'adapter_' + method + '_required');
    }
    requireValue(typeof observe === 'function' && Number.isFinite(timeout) && timeout >= 2000
      && timeout <= 600000, 'bounded_observer_required');
    this.#api = api; this.#adapter = adapter; this.#observe = observe;
    this.#now = now; this.#sleep = sleep; this.#uuid = uuid; this.#timeout = timeout;
    this.#signal = signal;
  }
  #checkInterrupted() {requireValue(!this.#signal?.aborted, 'journey_interrupted');}

  // Partial admission identity survives failures for the enclosing campaign's
  // cleanup journal. Returning clones prevents caller edits from relabeling it.
  resources() { return structuredClone(this.#records); }
  status() { return {next_stage:STAGES[this.#index] ?? null, failed:this.#failed, active:this.#active}; }
  async #event(type, fields) { await this.#observe({type, stage:STAGES[this.#index], ...structuredClone(fields)}); }

  async #poll(read, accept, code) {
    const deadline = this.#now() + this.#timeout;
    do {
      this.#checkInterrupted();
      const value = await read();
      if (accept(value)) return value;
      requireValue(this.#now() < deadline, code);
      await this.#sleep(2000);
    } while (true);
  }

  async #request(method, route, options) {
    this.#checkInterrupted();
    const response = await this.#api.request('a',method,route,options);
    await this.#event('api_response',{method,route,response});
    return response.body;
  }

  async #detail(record) {
    const detail = await this.#request('GET','/api/v1/agents/' + record.agent_id);
    requireValue(same(lineage(detail,record.agent_id),record.lineage), 'retained_lineage_changed');
    return detail;
  }

  async #native(record) {
    const raw = await this.#adapter.runtimeFor({agentId:record.agent_id});
    await this.#event('native_observation',{agent_id:record.agent_id,raw});
    const value = nativeObservation(raw,record.lineage,this.#artifacts.runtime);
    if (record.native) requireValue(same(value,record.native), 'retained_runtime_changed');
    return value;
  }

  async #history(record) {
    const result = transcript(await this.#request('GET','/api/v1/agents/' + record.agent_id + '/transcript'),record.agent_id);
    if (record.transcript) preservedTranscript(record.transcript,result);
    return result;
  }

  async #verify(record) {
    await this.#detail(record);
    await this.#native(record);
    for (const response of record.responses) {
      const value = await this.#request('GET','/api/v2/responses/' + response.id);
      requireValue(same(value,response), 'retained_response_changed');
    }
    const history = await this.#history(record);
    requireValue(same(history.turns,record.transcript.turns), 'unexpected_retained_turn');
  }

  async #create(stage) {
    const key = 'migration-' + this.#uuid(), name = key;
    requireValue(identity(key), 'fixture_key_invalid');
    const record = {key,name,writer:stage,operation_id:null,agent_id:null,responses:[],turns:[]};
    this.#records.push(record);
    await this.#event('create_intent',{key,name});
    this.#checkInterrupted();
    // Admission and send are never retried, even when a response is lost.
    const receipt = await this.#api.request('a','POST','/api/v1/agents',{
      body:{name,machine_included:false,desktop:false},key,allowed:[202]});
    requireValue(identity(receipt.body?.id), 'lifecycle_receipt_missing');
    record.operation_id = receipt.body.id;
    await this.#event('create_receipt',{receipt});
    await this.#poll(async () => {
      const value = await this.#api.request('a','GET','/api/v1/agent-lifecycle/' + record.operation_id);
      const op = value.body;
      requireValue(op?.id === record.operation_id, 'lifecycle_identity_mismatch');
      if (op.agent_id != null) {
        requireValue(identity(op.agent_id) && (!record.agent_id || record.agent_id === op.agent_id),
          'lifecycle_agent_changed');
        record.agent_id = op.agent_id;
      }
      await this.#event('lifecycle_observation',{operation:op});
      requireValue(!['failed','waiting_for_connection'].includes(op.status), 'create_lifecycle_failed');
      return op;
    }, op => op.status === 'succeeded' && identity(record.agent_id), 'create_lifecycle_timeout');
    const detail = await this.#poll(async () => {
      const value = await this.#request('GET','/api/v1/agents/' + record.agent_id);
      requireValue(value?.id === record.agent_id && !['failed','deleted','stopped'].includes(value.status),
        'created_agent_failed');
      return value;
    }, v => v.status === 'ready' && v.sandbox?.status === 'active'
      && (!v.first_message || v.first_message.state === 'completed'), 'agent_ready_timeout');
    record.lineage = lineage(detail,record.agent_id);
    record.native = await this.#native(record);
    record.transcript = await this.#history(record);
    await this.#event('agent_created',{record});
    return record;
  }

  async #reply(record) {
    const caseId = 'migration-' + this.#uuid(), marker = 'MIGRATION_' + this.#uuid();
    requireValue(identity(caseId) && identity(marker), 'fixture_turn_key_invalid');
    const turn = {case_id:caseId,marker,response_id:null}; record.turns.push(turn);
    await this.#event('send_intent',{agent_id:record.agent_id,turn});
    this.#checkInterrupted();
    await this.#adapter.prepareTurn({caseId,kind:'migration',marker});
    let stream, settled, failure;
    try {
      this.#checkInterrupted();
      stream = await this.#api.send('a',record.agent_id,'Return exactly ' + marker + ' without tools or file changes.',caseId);
      if (stream?.done && typeof stream.done.then === 'function') {
        settled = stream.done.then(() => null,error => error);
      }
      requireValue(identity(stream?.responseId) && typeof stream.abort === 'function'
        && settled, 'response_stream_binding_missing');
      turn.response_id = stream.responseId;
      // Attach immediately so an early stream failure never becomes an
      // unhandled rejection while the response/oracle reads are in flight.
      await this.#event('send_receipt',{agent_id:record.agent_id,turn});
      const held = await this.#poll(() => this.#adapter.oracle({caseId}),value => {
        requireValue(value?.case_id === caseId && Number.isInteger(value.request_count)
          && value.request_count >= 0 && value.request_count <= 1
          && value.completed_effect_count === 0, 'provider_held_contract_changed');
        return value.request_count === 1 && value.active_streams === 1;
      },'provider_request_timeout');
      providerObservation(held,caseId,0);
      this.#checkInterrupted();
      await this.#adapter.releaseTurn({caseId});
      const response = await this.#poll(() => this.#request('GET','/api/v2/responses/' + turn.response_id),
        terminal,'response_terminal_timeout');
      completedResponse(response,record.agent_id,turn.response_id,marker);
      const oracle = await this.#poll(() => this.#adapter.oracle({caseId}),value => {
        requireValue(value?.case_id === caseId && value.request_count === 1
          && [0,1].includes(value.completed_effect_count), 'provider_effect_count_mismatch');
        return value.active_streams === 0 && value.completed_effect_count === 1;
      },'provider_completion_timeout');
      providerObservation(oracle,caseId,1);
      await this.#event('provider_completion',{oracle});
      const history = await this.#history(record);
      requireValue(history.turns.length === record.transcript.turns.length + 1
        && history.turns.at(-1).owner_message.includes(marker)
        && history.turns.at(-1).agent_messages.some(text => text.includes(marker)), 'new_transcript_turn_missing');
      record.responses.push(structuredClone(response)); record.transcript = history;
      await this.#event('response_completed',{agent_id:record.agent_id,response,history});
    } catch (error) {failure = error;}
    if (failure && typeof stream?.abort === 'function') {
      try {stream.abort();}
      catch (error) {failure = new AggregateError([failure,error],'reply_and_abort_failed');}
    }
    if (settled) {
      try {const error = await settled; if (error) throw error;}
      catch (error) {failure = failure ? new AggregateError([failure,error],'reply_and_stream_failed') : error;}
    }
    if (failure) throw failure;
  }

  async run(stage) {
    this.#checkInterrupted();
    requireValue(!this.#failed && !this.#active && stage === STAGES[this.#index], 'finite_stage_order_required');
    this.#active = true;
    try {
      const expected = this.#artifacts[stage === 'old-write' || stage === 'old-read-new-write' ? 'old' : 'new'];
      const before = readerObservation(await this.#adapter.reader(),expected);
      const last = this.#readers.at(-1);
      if (last) requireValue(last.image === before.image ? same(last,before)
        : last.pod.uid !== before.pod.uid && last.pod.container_id !== before.pod.container_id,
      'reader_process_transition_invalid');
      await this.#event('reader_before',{reader:before});
      for (const record of this.#records) await this.#verify(record);
      if (stage === 'old-write' || stage === 'new-write') await this.#create(stage);
      for (const record of this.#records) {await this.#reply(record); await this.#verify(record);}
      const after = readerObservation(await this.#adapter.reader(),expected);
      requireValue(same(before,after), 'reader_changed_during_stage');
      const result = {schema:'migration-retained-agent-stage-v1',stage,artifact:expected,
        reader:after,agents:this.#records.map(r => ({agent_sha256:hash(r.agent_id),writer:r.writer,
          lineage_sha256:hash(r.lineage),runtime_sha256:hash(r.native),responses:r.responses.length,
          transcript_sha256:hash(r.transcript)})),outcome:'API_JOURNEY_OBSERVED',
        persisted_collection_coverage:'NOT_PROVEN',full_reader_writer_gate:'NOT_PROVEN'};
      await this.#event('stage_complete',{result});
      this.#readers.push(after); this.#index += 1;
      return structuredClone(result);
    } catch (error) {
      this.#failed = true;
      const reason = /^[a-z0-9_]{1,100}$/.test(error.message) ? error.message : 'adapter_or_transport_failure';
      try {await this.#event('stage_failed',{reason,resources:this.resources()});}
      catch (observerError) {throw new AggregateError([error,observerError],'stage_and_evidence_failed');}
      throw error;
    } finally {this.#active = false;}
  }
}
