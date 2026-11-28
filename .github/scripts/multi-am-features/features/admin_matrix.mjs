// Assertions shared with the run17 feature matrix, bound to the CI checkout;
// this module never starts a cluster, reads credentials, or changes AM topology.
import { performance } from 'node:perf_hooks';
import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import {errorCode, withRestoration} from '../failure.mjs';

export const ADMIN_PERMISSIONS = ['sandboxes','events','activity','users','control','settings'].map(x=>`cma:admin:${x}`);
const P = '/api/v1/admin';
const other = am => am === 'a' ? 'b' : 'a';
const hash = x => createHash('sha256').update(String(x)).digest('hex');
const clone = x => structuredClone(x);
const check = (v, code) => { if (!v) throw new Error(code); };
const revision = v => Number.isSafeInteger(v) && v >= 0;
const page = v => Array.isArray(v?.items) && (v.next === null || typeof v.next === 'string');
const settings = v => revision(v?.revision) && v.settings && v.settings.limits && v.settings.retention;
const plain = v => v !== null && typeof v === 'object' && !Array.isArray(v);
const sid = v => {check(typeof v === 'string' && /^[-a-zA-Z0-9_]{1,160}$/.test(v),'invalid_fixture_id'); return encodeURIComponent(v);};

function admissionSnapshot({revision, operator_stop, last_change, writes_available, history}) {
  return {revision, operator_stop, last_change, writes_available, history};
}

function admissionHistory(value) {
  if (!revision(value?.revision) || typeof value.writes_available !== 'boolean'
      || !Array.isArray(value.history) || value.history.length > 20) return false;
  if (value.revision === 0) {
    return value.operator_stop === null && value.last_change === null && value.history.length === 0;
  }
  return value.history.length > 0
    && value.history.every((change, index) => change?.revision === value.revision - index
      && change.revision > 0 && revision(change.at) && typeof change.actor === 'string'
      && typeof change.reason === 'string')
    && isDeepStrictEqual(value.last_change, value.history[0])
    && isDeepStrictEqual(value.operator_stop, value.last_change.new_stop);
}

export function validateFixture(f) {
  check(f?.scope === 'LOCAL_DISPOSABLE_TWO_AM' && /^[a-f0-9]{40}$/.test(f.source) && /^sha256:[a-f0-9]{64}$/.test(f.manifest),'exact_local_image_required');
  check(f.database_disposable === true && f.single_matrix_writer === true,'disposable_single_writer_required');
  for (const am of ['a','b']) {
    const u = new URL(f.am_origins?.[am]);
    check(['localhost','127.0.0.1','[::1]'].includes(u.hostname) && ['http:','https:'].includes(u.protocol)
      && !u.username && !u.password,'nonlocal_am_refused');
  }
  check(f.am_origins.a !== f.am_origins.b,'distinct_am_origins_required');
  check(ADMIN_PERMISSIONS.every(p=>f.admin_permissions?.includes(p)),'exact_admin_permissions_required');
  check(f.ordinary_user_has_admin === false,'ordinary_user_denial_identity_required');
}

export const READ_CASES = [
  ['sandbox_inventory',`${P}/sandboxes?limit=2`,page],
  ['profile_inventory',`${P}/profiles?limit=2`,page],
  ['runtime_inventory',`${P}/runtime-inventory`,v=>Array.isArray(v?.runtimes) && typeof v.observed_at==='string'],
  ['settings_read',`${P}/settings`,settings],
  ['activity_full',`${P}/activity?limit=2`,page],
  ['activity_summary',`${P}/activity?view=summary&limit=2`,page],
  ['settings_audit',`${P}/activity?action=configure&limit=2`,page],
  ['warm_pool_inventory',`${P}/warm-pools`,v=>Array.isArray(v?.items) && typeof v.available==='boolean'],
  ['auto_pause_preview',`${P}/auto-pause/preview`,v=>Number.isSafeInteger(v?.candidates)],
  ['admission_read',`${P}/admission`,admissionHistory],
  ['platform_spend_read',`${P}/spend`,v=>typeof v?.metering_enabled==='boolean'],
];

export function verifyRuntimeInventory(observation, inventories) {
  for (const inventory of inventories) {
    const matches = inventory.runtimes.filter(row => row.owner_reference === observation.sandbox_id);
    check(Number.isFinite(Date.parse(inventory.observed_at)) && matches.length === 1
      && matches[0].runtime_id === observation.runtime_id && matches[0].generation === observation.runtime_generation
      && matches[0].status === observation.provider_status, 'runtime_inventory_durable_observation_mismatch');
  }
}

export class AdminMatrix {
  // request(role, am, method, path, {body,key,allowed,timeout}) must make a real
  // owner-authenticated local request and return {status,body}; no auto retries.
  constructor({fixture, request, record, reserveResumePair, withholdAdmissionReply = false,
    onBoundary = async()=>{}, onFailure = async()=>{}}) {
    validateFixture(fixture);
    check(typeof request==='function' && typeof record==='function','owner_callbacks_required');
    check(!withholdAdmissionReply || typeof reserveResumePair === 'function', 'admission_reply_control_requires_paired_resume');
    Object.assign(this,{fixture,request,record,reserveResumePair,withholdAdmissionReply,onBoundary,onFailure});
    this.rows=[]; this.current=null;
  }
  async call(am,method,path,body,allowed=[200],role='admin',key,transport = {}) {
    const started=performance.now();
    const observation={am,method,route:path.split('?')[0].replace(/\/(sandboxes|profiles|warm-pools)\/[^/]+/g,'/$1/{id}'),role};
    try {
      const response=await this.request(role,am,method,path,{body,key,allowed,timeout:30000,...transport});
      observation.status=response?.status;
      observation.am=response?.timing?.am ?? am;
      observation.request_id=response?.timing?.request_id;
      if (transport.discardSuccessfulReply && response.status === 200) {
        check(response.timing?.reply_discarded === true && response.body === null, 'admission_reply_was_not_discarded');
        observation.reply_discarded = true;
      }
      check(allowed.includes(response?.status),'unexpected_http_'+response?.status);
      return response.body;
    } catch(error) {
      observation.status ??= error?.response?.status;
      observation.am=error?.response?.timing?.am ?? observation.am;
      observation.request_id ??= error?.response?.timing?.request_id;
      observation.error_code=/^[a-zA-Z0-9_:-]{1,160}$/.test(error?.message??'')?error.message:'transport_or_contract_error';
      throw error;
    } finally {observation.elapsed_ms=performance.now()-started;this.current?.requests.push(observation);}
  }
  async runCase(id,iteration,work) {
    const row={feature:id,iteration,verdict:'RUNNING',requests:[],started_at:new Date().toISOString()};
    const start=performance.now(); this.current=row;
    try {await work(); row.verdict='PASS';}
    catch(e) {row.verdict='FAIL'; row.error_code=/^[a-zA-Z0-9_:-]{1,160}$/.test(e?.message??'')?e.message:'transport_or_contract_error'; throw e;}
    finally {row.elapsed_ms=performance.now()-start;row.ended_at=new Date().toISOString();this.rows.push(row);this.current=null;await this.record(row);}
  }
  async withRestoration(work, restore) {
    return withRestoration(work, restore, async (stage, error) => {
      if (this.current) this.current[stage + '_error_code'] = errorCode(error);
      const receipt = await this.onFailure(stage, error, this.current);
      if (this.current && receipt) (this.current.failures ??= []).push(receipt);
    });
  }
  async reads(iteration, verifyRuntimeInventory) {
    for (const [id,path,shape] of READ_CASES) await this.runCase(id,iteration,async()=>{
      const first = await this.call('a', 'GET', path);
      const second = await this.call('b', 'GET', path);
      check(shape(first) && shape(second), id + '_shape');
      if (id === 'admission_read') {
        check(isDeepStrictEqual(admissionSnapshot(first), admissionSnapshot(second)), 'admission_history_peer_mismatch');
      }
      if (id === 'runtime_inventory' && verifyRuntimeInventory) await verifyRuntimeInventory(iteration, first, second);
    });
  }
  async denials(iteration) {
    for(const [id,path] of READ_CASES) await this.runCase(id+'_nonadmin_denial',iteration,async()=>{
      for(const am of ['a','b']) await this.call(am,'GET',path,undefined,[403],'user');
    });
  }
  async settingsRoundtrip(iteration,section,transform) {
    const am=iteration%2?'a':'b', peer=other(am), path=`${P}/settings`;
    await this.runCase('settings_'+section+'_cross_am_cas',iteration,async()=>{
      const before=await this.call(am,'GET',path); check(settings(before),'settings_snapshot_shape');
      const old=clone(before.settings[section]); check(old!==undefined,'settings_section_missing');
      const desired=transform(clone(old)); let acknowledged;
      try {
        acknowledged=await this.call(am,'PATCH',path,{expected_revision:before.revision,[section]:desired});
        check(settings(acknowledged) && acknowledged.revision>before.revision,'settings_revision_not_advanced');
        const visible=await this.call(peer,'GET',path);
        check(visible.revision===acknowledged.revision && isDeepStrictEqual(visible.settings[section],desired),'cross_am_settings_not_visible');
        await this.call(peer,'PATCH',path,{expected_revision:before.revision,[section]:old},[409]);
      } finally {
        // Reconcile with a fresh read after both acknowledged and uncertain
        // writes. Never repeat the original mutation or overwrite an alien value.
        const current=await this.call(peer,'GET',path);
        if(!isDeepStrictEqual(current.settings[section],old)) {
          check(isDeepStrictEqual(current.settings[section],desired),'settings_restore_foreign_value');
          await this.call(peer,'PATCH',path,{expected_revision:current.revision,[section]:old});
        }
        const restored=await this.call(am,'GET',path);
        check(isDeepStrictEqual(restored.settings[section],old),'settings_restore_not_visible');
      }
    });
  }
  async settingsCases(iteration) {
    await this.settingsRoundtrip(iteration,'retention',v=>({...v,agent_log_days:v.agent_log_days===30?31:30}));
    await this.settingsRoundtrip(iteration,'limits',v=>({...v,visitor_max_launches_per_day:v.visitor_max_launches_per_day===5?6:5}));
    await this.settingsRoundtrip(iteration,'usage_reporting',v=>({...v,measured_visible:!v.measured_visible}));
    await this.settingsRoundtrip(iteration,'marketplace',v=>({...v,enabled:!v.enabled}));
    await this.settingsRoundtrip(iteration,'triggers',v=>({...v,enabled:!v.enabled}));
  }
  async triggerPause(iteration) {
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/settings`;
    await this.runCase('trigger_orchestration_pause_resume',iteration,async()=>{
      const before=await this.call(am,'GET',path); const old=before.settings.triggers.orchestration_paused;
      try {
        await this.call(am,'POST',`${P}/triggers/pause`);
        check((await this.call(peer,'GET',path)).settings.triggers.orchestration_paused===true,'trigger_pause_not_visible');
        await this.call(peer,'POST',`${P}/triggers/resume`);
        check((await this.call(am,'GET',path)).settings.triggers.orchestration_paused===false,'trigger_resume_not_visible');
      } finally {
        const current=await this.call(peer,'GET',path);
        if(current.settings.triggers.orchestration_paused!==old)
          await this.call(peer,'POST',`${P}/triggers/${old?'pause':'resume'}`);
        check((await this.call(am,'GET',path)).settings.triggers.orchestration_paused===old,'trigger_restore_failed');
      }
    });
  }
  async admission(iteration, verifyAdmission) {
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/admission`;
    await this.runCase('admission_stop_resume',iteration,async()=>{
      const initial=await this.call(am,'GET',path);
      check(initial.writes_available===true && initial.operator_stop===null,'admission_fixture_must_be_open_writable');
      const reason=`local-matrix-${this.fixture.run_id}-${iteration}`;
      const discardSuccessfulReply = this.withholdAdmissionReply;
      await this.withRestoration(async () => {
        const stopReply = await this.call(am, 'POST', `${path}/stop`, {expected_revision: initial.revision, reason},
          [200], 'admin', undefined, {discardSuccessfulReply});
        const stopped = discardSuccessfulReply ? await this.call(peer, 'GET', path) : stopReply;
        check(stopped.operator_stop?.reason === reason && stopped.revision === initial.revision + 1
          && stopped.last_change?.reason === reason,
          'admission_stop_ack');
        const replay = await this.call(peer, 'POST', `${path}/stop`,
          {expected_revision: initial.revision, reason: reason + '-competing'});
        check(replay.revision === stopped.revision && isDeepStrictEqual(replay.operator_stop, stopped.operator_stop),
          'admission_stop_replay_changed');
        const peerState=await this.call(peer,'GET',path);
        check(admissionHistory(peerState) && peerState.revision === stopped.revision
          && isDeepStrictEqual(peerState.operator_stop, stopped.operator_stop)
          && isDeepStrictEqual(peerState.last_change, stopped.last_change)
          && peerState.last_change.old_stop === null, 'admission_stop_not_visible');
        await verifyAdmission?.(peer, 'paused');
        const stale = await this.call(peer, 'POST', `${path}/resume`, {expected_revision: initial.revision, reason}, [409]);
        check(stale?.code === 'admission_control_conflict', 'admission_resume_stale_refusal_untyped');
        const resumePath = `${path}/resume`;
        const resume = {expected_revision: stopped.revision, reason};
        let resumed;
        if (this.reserveResumePair) {
          await this.reserveResumePair();
          const attempts = await Promise.allSettled([peer, am].map(target =>
            this.call(target, 'POST', resumePath, resume, [200, 409], 'admin', undefined,
              {pacedPair: true, discardSuccessfulReply})));
          for (const attempt of attempts) if (attempt.status === 'rejected') throw attempt.reason;
          const results = attempts.map(attempt => attempt.value);
          const accepted = results.filter(value => discardSuccessfulReply ? value === null
            : value?.revision === stopped.revision + 1 && value.operator_stop === null);
          check(accepted.length === 1 && results.filter(value => value?.code === 'admission_control_conflict').length === 1,
            'admission_concurrent_resume_unbound');
          [resumed] = accepted;
        } else {
          resumed = await this.call(peer, 'POST', resumePath, resume);
        }
        if (!discardSuccessfulReply) {
          check(resumed.revision === stopped.revision + 1 && resumed.operator_stop === null, 'admission_resume_ack');
        }
        const settled = await this.call(am, 'GET', path);
        check(admissionHistory(settled) && settled.revision === stopped.revision + 1 && settled.operator_stop === null
          && settled.last_change.reason === reason
          && (discardSuccessfulReply || isDeepStrictEqual(settled.last_change, resumed.last_change))
          && isDeepStrictEqual(settled.last_change.old_stop, stopped.operator_stop)
          && isDeepStrictEqual(settled.history[1], stopped.last_change), 'admission_resume_not_visible');
        await verifyAdmission?.(am, 'open');
      }, async () => {
        const current=await this.call(peer,'GET',path);
        if(current.operator_stop!==null) {
          check(current.operator_stop.reason===reason,'admission_restore_foreign_stop');
          await this.call(peer,'POST',`${path}/resume`,{expected_revision:current.revision,reason});
        }
        check((await this.call(am,'GET',path)).operator_stop===null,'admission_restore_failed');
      });
    });
  }
  async sandboxAlwaysOn(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id,'disposable_sandbox_required');
    const id=sid(fixture.sandbox_id),path=`${P}/sandboxes/${id}`,am=iteration%2?'a':'b',peer=other(am);
    await this.runCase('sandbox_always_on_cross_am',iteration,async()=>{
      const detail=await this.call(am,'GET',path),before=detail.sandbox;
      check(before?.id===fixture.sandbox_id && revision(before.revision),'sandbox_detail_identity');
      const old=before.profile.always_on===true,desired=!old;
      await this.withRestoration(async () => {
        const changed=await this.call(am,'POST',`${path}/always-on`,{always_on:desired,expected_revision:before.revision});
        // POST returns SandboxView. The detail GET below returns an aggregate.
        check(changed.id===fixture.sandbox_id && changed.always_on===desired,'always_on_ack');
        const current=(await this.call(peer,'GET',path)).sandbox;
        check(current.id===fixture.sandbox_id && current.profile.always_on===desired,'always_on_not_visible');
        if (fixture.verifyAdmission) await fixture.verifyAdmission('changed');
        await this.call(peer,'POST',`${path}/always-on`,{always_on:old,expected_revision:before.revision},[409]);
        const events=await this.call(peer,'GET',`${path}/events?limit=50`);
        check(Array.isArray(events) && events.every(e=>e.sandbox_id===fixture.sandbox_id),'sandbox_event_identity');
        if (fixture.verifyBrowserRelinquishment) {
          check(!old && desired && revision(current.revision), 'browser_relinquishment_requires_fresh_approval');
          this.current.owner_relinquishment = await fixture.verifyBrowserRelinquishment(current.revision);
        }
      }, async () => {
        const current=(await this.call(peer,'GET',path)).sandbox;
        if((current.profile.always_on===true)!==old)
          await this.call(peer,'POST',`${path}/always-on`,{always_on:old,expected_revision:current.revision});
        check(((await this.call(am,'GET',path)).sandbox.profile.always_on===true)===old,'always_on_restore_failed');
        if (fixture.verifyAdmission) await fixture.verifyAdmission('restored');
      });
    });
  }
  async retireSandbox(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id,'disposable_sandbox_required');
    const id=sid(fixture.sandbox_id),path=`${P}/sandboxes/${id}`,am=iteration%2?'a':'b',peer=other(am);
    await this.runCase('sandbox_retire_public_admin',iteration,async()=>{
      const before=(await this.call(am,'GET',path)).sandbox;
      check(before.id===fixture.sandbox_id && !['retired','retiring'].includes(before.status),'fresh_retirement_fixture_required');
      await this.call(am,'POST',`${path}/retire`);
      const end=performance.now()+120000;
      for (;;) {
        const after=(await this.call(peer,'GET',path)).sandbox;
        check(after.id===fixture.sandbox_id,'retirement_identity');
        if(after.status==='retired') break;
        check(performance.now()<end,'retirement_terminal_timeout');
        await new Promise(r=>setTimeout(r,500));
      }
    });
  }
  async warmPool(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && fixture.maximum_replicas===1,'bounded_warm_pool_fixture_required');
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/warm-pools`;
    await this.runCase('warm_pool_configure_resize',iteration,async()=>{
      const baseline=await this.call(am,'GET',path);
      check(baseline.available===true,'warm_pool_backend_unavailable');
      const body={cpu_millis:fixture.cpu_millis,memory_mib:fixture.memory_mib,target_replicas:0};
      check(body.cpu_millis>0 && body.memory_mib>0,'warm_pool_resources_required');
      check(!baseline.items.some(x=>x.cpu_millis===body.cpu_millis && x.memory_mib===body.memory_mib),'warm_pool_requires_fresh_configuration');
      let pool;
      try {
        pool=await this.call(am,'POST',path,body,[202],'admin',`matrix-${this.fixture.run_id}-${iteration}`);
        sid(pool.id);check(pool.target_replicas===0 && revision(pool.revision),'warm_pool_create_ack');
        const changed=await this.call(peer,'PUT',`${path}/${sid(pool.id)}`,{expected_revision:pool.revision,target_replicas:1},[202]);
        check(changed.id===pool.id && changed.target_replicas===1,'warm_pool_resize_ack');
        const visible=(await this.call(am,'GET',path)).items.find(x=>x.id===pool.id);
        check(visible?.target_replicas===1,'warm_pool_target_not_visible');
        await this.call(am,'PUT',`${path}/${sid(pool.id)}`,{expected_revision:pool.revision,target_replicas:0},[409]);
        const until=performance.now()+120000;
        for(;;) {
          const observed=(await this.call(peer,'GET',path)).items.find(x=>x.id===pool.id);
          if(observed?.observed===true && observed.ready_replicas===1 && observed.status==='ready') break;
          check(performance.now()<until,'warm_pool_ready_timeout');
          await new Promise(r=>setTimeout(r,1000));
        }
      } finally {
        // A lost configure reply may have created a durable pool. Match the
        // newly-created exact resource configuration instead of resending POST.
        const candidates=(await this.call(peer,'GET',path)).items.filter(x=>x.cpu_millis===body.cpu_millis && x.memory_mib===body.memory_mib);
        check(candidates.length<=1,'warm_pool_cleanup_identity_ambiguous');
        if(candidates.length) {
          const current=candidates[0];
          if(current.target_replicas!==0) await this.call(peer,'PUT',`${path}/${sid(current.id)}`,{expected_revision:current.revision,target_replicas:0},[202]);
          const until=performance.now()+120000;
          for(;;) {
            const current=(await this.call(am,'GET',path)).items.find(x=>x.id===candidates[0].id);
            check(current?.target_replicas===0,'warm_pool_disable_not_visible');
            if(current.observed===true && current.ready_replicas===0) break;
            check(performance.now()<until,'warm_pool_cleanup_timeout');
            await new Promise(r=>setTimeout(r,1000));
          }
        }
      }
    });
  }
  async retireProfile(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && ['sandbox','workspace','agent'].includes(fixture.kind),'disposable_profile_required');
    const am=iteration%2?'a':'b',peer=other(am),id=sid(fixture.id);
    await this.runCase('profile_retirement_'+fixture.kind,iteration,async()=>{
      const retired=await this.call(am,'POST',`${P}/profiles/${fixture.kind}/${id}/retire`);
      check(retired.id===fixture.id && retired.kind===fixture.kind && typeof retired.retired_at==='string','profile_retire_ack');
      let cursor;let found;
      for(let n=0;n<20;n++) {
        const result=await this.call(peer,'GET',`${P}/profiles?kind=${fixture.kind}&limit=200${cursor?'&cursor='+encodeURIComponent(cursor):''}`);
        check(page(result),'profile_list_shape'); found=result.items.find(x=>x.id===fixture.id);
        if(found || !result.next) break; cursor=result.next;
      }
      check(found?.retired_at===retired.retired_at,'profile_retirement_not_visible');
    });
  }
  async runtimeUpdate(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && fixture.expected_target && typeof fixture.verifyHome==='function','runtime_update_fixture_and_home_probe_required');
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/sandboxes/${sid(fixture.sandbox_id)}`;
    await this.runCase('runtime_upgrade_preserves_home',iteration,async()=>{
      const before=await this.call(am,'GET',path);
      check(before.runtime_update?.update_available===true && before.runtime_update.can_update===true,'runtime_upgrade_fixture_requires_real_old_image');
      check(before.runtime_update.latest_version===fixture.expected_target,'runtime_upgrade_target_mismatch');
      const home=await fixture.verifyHome('before');
      check(typeof home==='string' && /^[a-f0-9]{64}$/.test(home),'home_digest_required');
      await this.call(am,'POST',`${path}/runtime-update`,{target_version:fixture.expected_target},[202]);
      const until=performance.now()+180000;
      for(;;) {
        const current=await this.call(peer,'GET',path);
        check(current.sandbox.id===fixture.sandbox_id,'runtime_upgrade_identity');
        check(current.runtime_update?.state!=='failed','runtime_upgrade_failed');
        if(current.runtime_update?.state==='up_to_date' && current.runtime_update.current_version===fixture.expected_target) break;
        check(performance.now()<until,'runtime_upgrade_terminal_timeout');
        await new Promise(r=>setTimeout(r,1000));
      }
      check(await fixture.verifyHome('after')===home,'runtime_upgrade_home_changed');
    });
  }
  async usage(iteration) {
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/usage/metrics/reports`;
    for(const [window,seconds] of [['24h',86400],['7d',604800],['30d',2592000]])
      await this.runCase('admin_usage_'+window,iteration,async()=>{
        const to=Math.floor(Date.now()/1000),start=performance.now();
        let report=await this.call(am,'POST',path,{from:to-seconds,to},[200,202]);
        const id=sid(report.report_id),until=performance.now()+120000;
        for(;;) {
          check(report.report_id===decodeURIComponent(id),'usage_report_identity');
          if(report.status==='ready') {check(report.report_complete===true,'usage_report_incomplete');break;}
          check(report.status==='pending','usage_report_terminal_failure');
          check(performance.now()<until,'usage_report_completion_timeout');
          await new Promise(r=>setTimeout(r,500));report=await this.call(peer,'GET',`${path}/${id}`);
        }
        const complete=performance.now()-start;
        this.current.criterion={target_ms:2000,observed_ms:complete,verdict:complete<=2000?'PASS':'FAIL'};
        for(const suffix of ['sandboxes?limit=2','owners?limit=2','owner-allocations?metric=cpu&limit=2','series?metric=cpu&group_by=total']) {
          const value=await this.call(peer,'GET',`${path}/${id}/${suffix}`);
          check(value.report_id===decodeURIComponent(id),'usage_projection_report_identity');
        }
      });
  }
  async run({iterations=3,groups=['reads','denials','settings','triggers','admission'],sandboxFixtures=[],warmPoolFixtures=[],profileFixtures=[],runtimeFixtures=[],verifyAdmission,verifyRuntimeInventory}={}) {
    check(Number.isInteger(iterations)&&iterations>=3&&iterations<=10,'minimum_three_bounded_iterations');
    check(groups.every(x=>['reads','denials','settings','triggers','admission','always_on','retire','warm_pool','profile_retire','runtime_update','usage'].includes(x)),'unknown_case_group');
    for(let i=1;i<=iterations;i++) {
      await this.onBoundary({phase:'iteration_start',iteration:i});
      if(groups.includes('reads')) await this.reads(i, verifyRuntimeInventory);
      if(groups.includes('denials')) await this.denials(i);
      if(groups.includes('settings')) await this.settingsCases(i);
      if(groups.includes('triggers')) await this.triggerPause(i);
      if(groups.includes('admission')) await this.admission(i, verifyAdmission);
      if(groups.includes('always_on')) await this.sandboxAlwaysOn(i,sandboxFixtures[i-1]);
      if(groups.includes('warm_pool')) await this.warmPool(i,warmPoolFixtures[i-1]);
      if(groups.includes('profile_retire')) await this.retireProfile(i,profileFixtures[i-1]);
      if(groups.includes('runtime_update')) await this.runtimeUpdate(i,runtimeFixtures[i-1]);
      if(groups.includes('usage')) await this.usage(i);
      if(groups.includes('retire')) await this.retireSandbox(i,sandboxFixtures[i-1]);
      await this.onBoundary({phase:'iteration_complete',iteration:i});
    }
    return {verdict:'PASS',scope:'SELECTED_LOCAL_API_CASES_ONLY',source:this.fixture.source,manifest:this.fixture.manifest,groups,
      iterations,case_count:this.rows.length,ui_verification:false,rolling_restart_verified:false,hpa_verified:false};
  }
}
