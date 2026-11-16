import {coverage} from './coverage.mjs';

const requireValue = (value, message) => { if (!value) throw Error(message); };
const ids = Array.from({length: 13}, (_, index) => 'SW' + String(index + 1).padStart(2, '0'));

// The registry is reviewed source, never a result packet that can mark itself
// complete. Native fixtures and image/browser observations must be mapped before
// clearing a group's INCOMPLETE status. Missing groups cannot shrink the gate.
export function writerCoverage(registry, results, journey) {
  requireValue(registry.schema === 'multi-am-writer-regressions-v2', 'writer_registry_schema');
  requireValue(Array.isArray(registry.cases) && registry.cases.length === ids.length
    && registry.cases.map(row => row.id).sort().join(',') === ids.join(','), 'writer_registry_ids');
  const groups = registry.cases.map(group => {
    requireValue(['COMPLETE', 'INCOMPLETE_NOT_EXECUTION_PASS'].includes(group.status)
      && Array.isArray(group.required_assertions), 'writer_registry_status');
    requireValue(new Set(group.required_assertions.map(row => row.case)).size
      === group.required_assertions.length, 'writer_assertion_duplicate');
    const assertions = group.required_assertions.map(assertion => {
      requireValue(typeof assertion.case === 'string' && assertion.case.length > 0
        && Number.isSafeInteger(assertion.minimum_runs) && assertion.minimum_runs > 0
        && Array.isArray(assertion.phases)
        && ['non_owner', 'both', 'journey'].includes(assertion.routing), 'writer_assertion_contract');
      if (assertion.routing === 'journey') {
        const steps = journey?.steps?.filter(step => step.name === assertion.case) ?? [];
        return {case: assertion.case, verdict: !steps.length ? 'NOT_RUN'
          : journey.verdict === 'LOCAL_TWO_AM_JOURNEY_PASS' && steps.length === 1
            && steps[0].verdict === 'PASS' ? 'PASS' : 'FAIL'};
      }
      const mapping = {...assertion, cases: [assertion.case]};
      if (!mapping.phases.length) delete mapping.phases;
      const value = coverage([{id: assertion.case, role: 'writer'}], {[assertion.case]: mapping}, results);
      return {case: assertion.case, verdict: value.features[0].verdict};
    });
    const incomplete = group.status !== 'COMPLETE' || !assertions.length;
    const verdict = assertions.some(row => row.verdict === 'FAIL') ? 'FAIL'
      : incomplete ? 'BLOCKED' : assertions.some(row => row.verdict !== 'PASS') ? 'NOT_RUN' : 'PASS';
    return {id: group.id, verdict, assertions,
      ...(incomplete ? {remaining: group.remaining || 'required_evidence_mapping_incomplete'} : {})};
  });
  const counts = Object.fromEntries(['PASS', 'FAIL', 'BLOCKED', 'NOT_RUN']
    .map(verdict => [verdict, groups.filter(row => row.verdict === verdict).length]));
  return {schema: 'multi-am-writer-coverage-v1', requirements_sha256: registry.requirements_sha256,
    verdict: counts.PASS === ids.length ? 'PASS' : 'FAIL', total: ids.length, counts, groups};
}
