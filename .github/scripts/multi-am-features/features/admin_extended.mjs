// Additional local-only assertions. Extends the frozen core module without
// changing owner fixture code or the image. Invoked by the same sole parent.
import {AdminMatrix} from './admin_matrix.mjs';
import {isDeepStrictEqual as equal} from 'node:util';
import {performance} from 'node:perf_hooks';
const P='/api/v1/admin';
const check=(v,c)=>{if(!v)throw Error(c);};
const other=x=>x==='a'?'b':'a';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const id=x=>{check(typeof x==='string'&&/^[-a-zA-Z0-9_]{1,160}$/.test(x),'fixture_id_required');return encodeURIComponent(x);};
const controls=v=>{const x=structuredClone(v);if(x.inactivity)delete x.inactivity.grace_started_at;return x;};
export class ExtendedAdminMatrix extends AdminMatrix {
  async replaceRestore(iteration,caseId,change,verify) {
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/settings`;
    await this.runCase(caseId,iteration,async()=>{
      const old=await this.call(am,'GET',path);const desired=change(structuredClone(old.settings),old);
      try {
        const updated=await this.call(am,'PUT',path,{expected_revision:old.revision,settings:desired});
        check(updated.revision>old.revision,'settings_replace_revision');
        const seen=await this.call(peer,'GET',path);
        check(seen.revision===updated.revision && equal(controls(seen.settings),controls(updated.settings)),'settings_replace_peer_mismatch');
        verify(seen.settings);
        await this.call(peer,'PUT',path,{expected_revision:old.revision,settings:old.settings},[409]);
      } finally {
        const now=await this.call(peer,'GET',path);
        if(!equal(controls(now.settings),controls(old.settings))) {
          check(equal(controls(now.settings),controls(desired)),'settings_replace_restore_foreign_value');
          await this.call(peer,'PUT',path,{expected_revision:now.revision,settings:old.settings});
        }
        check(equal(controls((await this.call(am,'GET',path)).settings),controls(old.settings)),'settings_replace_restore_failed');
      }
    });
  }
  async fullReplace(iteration) {
    let desired;
    return this.replaceRestore(iteration,'settings_full_replace',settings=>{
      settings.retention.agent_log_days=settings.retention.agent_log_days===90?91:90;desired=structuredClone(settings);return settings;
    },settings=>check(equal(controls(settings),controls(desired)),'settings_full_replace_contract'));
  }
  async inactivity(iteration) {
    const enabledAt=Date.now();
    return this.replaceRestore(iteration,'settings_inactivity_grace',settings=>{
      settings.inactivity={auto_pause_enabled:true,pause_after_seconds:settings.inactivity.pause_after_seconds===900?901:900,grace_started_at:null};return settings;
    },settings=>{
      check(settings.inactivity.auto_pause_enabled===true,'inactivity_enable_missing');
      const time=Date.parse(settings.inactivity.grace_started_at);
      check(Number.isFinite(time)&&time>=enabledAt-2000&&time<=Date.now()+2000,'inactivity_new_grace_missing');
    });
  }
  async defaults(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id,'owned_default_profiles_required');
    for(const kind of ['sandbox','workspace','agent']) {
      const ref=fixture.defaults?.[kind+'_profile'];id(ref?.id);check(Number.isInteger(ref.revision)&&ref.revision>0,'profile_revision_required');
    }
    return this.replaceRestore(iteration,'settings_default_profiles',settings=>({...settings,defaults:fixture.defaults}),
      settings=>check(equal(settings.defaults,fixture.defaults),'default_profile_readback'));
  }
  async budget(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && fixture.policy && fixture.exact_source_validated===true,'budget_policy_fixture_required');
    return this.replaceRestore(iteration,'settings_spend_policy', (settings,state)=>{
      check(state.model_budget_writes_available===true,'budget_write_gate_unavailable');
      if(fixture.policy.request_models?.length)check(state.model_request_budget_writes_available===true,'request_budget_write_gate_unavailable');
      if(fixture.policy.platform?.cloud)check(state.platform_spend_writes_available===true,'cloud_budget_write_gate_unavailable');
      return {...settings,model_budget:fixture.policy};
    },settings=>check(equal(settings.model_budget,fixture.policy),'budget_policy_peer_mismatch'));
  }
  async reset(iteration,fixture) {
    check(fixture?.expected_defaults && fixture.exact_source_validated===true,'source_derived_reset_defaults_required');
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/settings`;
    await this.runCase('settings_reset_restore',iteration,async()=>{
      const old=await this.call(am,'GET',path);
      try {
        const reset=await this.call(am,'POST',`${path}/reset`,{expected_revision:old.revision});
        check(equal(controls(reset.settings),controls(fixture.expected_defaults)),'settings_reset_source_defaults_mismatch');
        check(equal((await this.call(peer,'GET',path)).settings,reset.settings),'settings_reset_peer_mismatch');
      } finally {
        const now=await this.call(peer,'GET',path);
        if(!equal(controls(now.settings),controls(old.settings))) {
          check(equal(controls(now.settings),controls(fixture.expected_defaults)),'settings_reset_restore_foreign_value');
          await this.call(peer,'PUT',path,{expected_revision:now.revision,settings:old.settings});
        }
        check(equal(controls((await this.call(am,'GET',path)).settings),controls(old.settings)),'settings_reset_restore_failed');
      }
    });
  }
  async idlePause(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && fixture.actual_idle_threshold_elapsed===true,'actual_idle_fixture_required');
    const idle=id(fixture.idle_sandbox),protectedId=id(fixture.always_on_sandbox),am=iteration%2?'a':'b',peer=other(am);
    await this.runCase('idle_pause_eligibility_and_protection',iteration,async()=>{
      const protectedBefore=(await this.call(am,'GET',`${P}/sandboxes/${protectedId}`)).sandbox;
      check(protectedBefore.profile.always_on===true && protectedBefore.status==='active','always_on_fixture_required');
      const preview=await this.call(am,'GET',`${P}/auto-pause/preview`);check(preview.candidates>=1,'idle_candidate_missing');
      await this.call(am,'POST',`${P}/auto-pause/pause-idle-now`,{confirmed:false},[400]);
      const result=await this.call(am,'POST',`${P}/auto-pause/pause-idle-now`,{confirmed:true});
      check(result.scheduled+result.already_satisfied>=1,'idle_pause_not_scheduled');
      const until=performance.now()+120000;
      for(;;) {
        const idleNow=(await this.call(peer,'GET',`${P}/sandboxes/${idle}`)).sandbox;
        const protectedNow=(await this.call(peer,'GET',`${P}/sandboxes/${protectedId}`)).sandbox;
        check(protectedNow.status==='active'&&protectedNow.profile.always_on===true,'always_on_sandbox_paused');
        if(idleNow.status==='paused')break;
        check(performance.now()<until,'idle_pause_terminal_timeout');await sleep(1000);
      }
    });
  }
  async protectedProfile(iteration,fixture) {
    check(fixture?.kind && ['sandbox','workspace','agent'].includes(fixture.kind) && fixture.is_current_default===true,'current_default_fixture_required');
    await this.runCase('protected_profile_retire_denied',iteration,async()=>{
      for(const am of ['a','b']) await this.call(am,'POST',`${P}/profiles/${fixture.kind}/${id(fixture.id)}/retire`,undefined,[409]);
      const settings=await this.call('a','GET',`${P}/settings`);
      check(settings.settings.defaults[fixture.kind+'_profile'].id===fixture.id,'protected_default_changed');
    });
  }
  async paginate(iteration,fixture) {
    check(Array.isArray(fixture?.expected_ids) && fixture.expected_ids.length>=3 && ['sandboxes','profiles','activity'].includes(fixture.collection),'multi_page_fixture_required');
    const am=iteration%2?'a':'b',peer=other(am);
    await this.runCase('pagination_'+fixture.collection,iteration,async()=>{
      const seen=new Set();let cursor;let pageCount=0;
      do {
        const path=`${P}/${fixture.collection}?limit=2${cursor?'&cursor='+encodeURIComponent(cursor):''}${fixture.query?'&'+fixture.query:''}`;
        const result=await this.call(pageCount%2?peer:am,'GET',path);
        check(Array.isArray(result.items),'pagination_shape');
        for(const row of result.items) {const key=row[fixture.id_field??'id'];check(typeof key==='string'&&!seen.has(key),'pagination_duplicate_or_missing_identity');seen.add(key);}
        cursor=result.next;pageCount++;check(pageCount<=100,'pagination_bound');
      } while(cursor);
      check(pageCount>=2 && fixture.expected_ids.every(x=>seen.has(x)),'pagination_fixture_rows_missing');
    });
  }
  async usageOptions(iteration,fixture) {
    check(fixture?.expected_values,'known_usage_filter_values_required');
    await this.runCase('usage_filter_options',iteration,async()=>{
      for(const kind of ['owner','sandbox','runtime'])for(const am of ['a','b']) {
        const value=await this.call(am,'POST',`${P}/usage/metrics/options`,{kind,limit:50});
        check(Array.isArray(value.items),'usage_filter_shape');
        check(fixture.expected_values[kind]?.length>0 && fixture.expected_values[kind].every(x=>value.items.some(row=>row.value===x)),'usage_filter_expected_values_missing');
      }
    });
  }
  async cloudObservation(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && fixture.metering_enabled===true,'isolated_cloud_ledger_required');
    const am=iteration%2?'a':'b',peer=other(am);
    await this.runCase('cloud_spend_observation',iteration,async()=>{
      const before=await this.call(am,'GET',`${P}/spend`);check(before.metering_enabled===true,'cloud_metering_disabled');
      const observed=fixture.observed_through;
      check(Number.isFinite(Date.parse(observed))&&Date.parse(observed)<=Date.now(),'cloud_observation_time_invalid');
      check(Number.isSafeInteger(fixture.units)&&fixture.units>=0,'cloud_observation_units_invalid');
      const result=await this.call(am,'POST',`${P}/spend/observations`,{source:'cloud',observed_through:observed,units:fixture.units});
      const seen=await this.call(peer,'GET',`${P}/spend`);
      check(seen.ledger?.cloud?.units===fixture.units && seen.ledger.cloud.observed_through===observed,'cloud_observation_not_visible');
      check(equal(seen.ledger.cloud,result.ledger.cloud),'cloud_observation_identity');
    });
  }
  async admissionEnforcement(iteration,fixture) {
    check(fixture?.created_by_run===this.fixture.run_id && ['probeCreate','probeSend','probeCleanup'].every(x=>typeof fixture[x]==='function'),'real_admission_probes_required');
    const am=iteration%2?'a':'b',peer=other(am),path=`${P}/admission`,reason=`matrix-enforcement-${this.fixture.run_id}-${iteration}`;
    await this.runCase('admission_enforcement_and_recovery',iteration,async()=>{
      const initial=await this.call(am,'GET',path);check(initial.writes_available&&initial.operator_stop===null,'admission_fixture_not_open');
      try {
        await this.call(am,'POST',`${path}/stop`,{expected_revision:initial.revision,reason});
        for(const target of ['a','b']) {
          for(const name of ['probeCreate','probeSend']) {
            const result=await fixture[name](target);
            check(result?.status===503 && result.body?.code==='admission_paused','fresh_work_not_rejected_'+name);
          }
          const cleanup=await fixture.probeCleanup(target);
          check(cleanup?.actual_terminal===true && cleanup.am===target,'cleanup_blocked_by_admission');
        }
      } finally {
        const now=await this.call(peer,'GET',path);
        if(now.operator_stop!==null) {
          check(now.operator_stop.reason===reason,'admission_foreign_stop');
          await this.call(peer,'POST',`${path}/resume`,{expected_revision:now.revision,reason});
        }
        check((await this.call(am,'GET',path)).operator_stop===null,'admission_restore_failed');
      }
    });
  }
  async savedUsageView(iteration,fixture) {
    check(fixture?.listed_scope===true && typeof fixture.verifyUnlistedUiFallback==='function','listed_view_and_real_ui_fallback_required');
    await this.runCase('usage_saved_view_and_unlisted_fallback',iteration,async()=>{
      for(const am of ['a','b']) {
        const until=performance.now()+120000;let reportId;
        for(;;) {
          const view=await this.call(am,'GET',`${P}/usage/metrics/view?window=24h`,undefined,[200,202]);
          check(view.report?.scope?.admin===true,'usage_view_admin_identity');
          if(view.report.status==='ready') {check(view.report.report_complete===true,'usage_view_incomplete');reportId=view.report.report_id;break;}
          check(view.report.status==='pending'&&performance.now()<until,'usage_view_timeout_or_failure');await sleep(1000);
        }
        id(reportId);
      }
      const fallback=await fixture.verifyUnlistedUiFallback();
      check(fallback?.real_ui===true && fallback.report_complete===true && fallback.unlisted_scope===true,'unlisted_ui_report_not_proven');
    });
  }

  async runExtended({iterations=3,groups=[],fixtures={}}={}) {
    const methods={replace:'fullReplace',inactivity:'inactivity',defaults:'defaults',budget:'budget',reset:'reset',idle_pause:'idlePause',protected_profile:'protectedProfile',pagination:'paginate',usage_options:'usageOptions',cloud:'cloudObservation',admission_enforcement:'admissionEnforcement',usage_view:'savedUsageView'};
    check(Number.isInteger(iterations)&&iterations>=3&&iterations<=10,'minimum_three_bounded_iterations');
    check(groups.length>0&&groups.every(x=>Object.hasOwn(methods,x)),'explicit_extended_groups_required');
    for(let i=1;i<=iterations;i++) {
      await this.onBoundary({phase:'extended_iteration_start',iteration:i});
      for(const group of groups)await this[methods[group]](i,Array.isArray(fixtures[group])?fixtures[group][i-1]:fixtures[group]);
      await this.onBoundary({phase:'extended_iteration_complete',iteration:i});
    }
    return {verdict:'PASS',scope:'SELECTED_LOCAL_EXTENDED_API_CASES_ONLY',iterations,groups,case_count:this.rows.length,
      ui_verification:false,rolling_restart_verified:false,hpa_verified:false};
  }

}
