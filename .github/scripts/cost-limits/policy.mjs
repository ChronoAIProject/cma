// M-SUST-05 defaults are checked independently from configured-policy mechanics.
// No unpriced accounting unit can silently become a dollar of verified coverage.
export function assessDefaults(observation, valuation) {
  if (observation?.schema !== 'cma-cost-defaults-v1') throw Error('cost_defaults_schema');
  const budget = observation.model_budget;
  const policy = budget?.default_policy;
  const checks = [
    {id: 'default_two_sandboxes', status: observation.platform_limits?.max_sandboxes_per_user === 2
      && observation.sandbox_owner_limit === 2 ? 'PASS' : 'FAIL',
    observed: {platform: observation.platform_limits?.max_sandboxes_per_user ?? null,
      service: observation.sandbox_owner_limit ?? null}, required: 2},
    {id: 'default_owner_budget_enabled', status: budget?.enabled === true && policy ? 'PASS' : 'FAIL'},
  ];
  if (budget?.enabled !== true || !policy) {
    checks.push({id: 'default_usd_five_per_utc_day', status: 'FAIL', reason: 'default_budget_absent_or_disabled'});
  } else if (valuation?.currency !== 'USD' || !Number.isSafeInteger(valuation.units_per_usd)
      || valuation.units_per_usd <= 0 || !/^[a-f0-9]{64}$/.test(valuation.approved_rate_sha256 ?? '')) {
    checks.push({id: 'default_usd_five_per_utc_day', status: 'BLOCKED', reason: 'approved_usd_valuation_missing'});
  } else {
    const exact = Number.isSafeInteger(policy.owner_day_limit)
      && BigInt(policy.owner_day_limit) === 5n * BigInt(valuation.units_per_usd);
    checks.push({id: 'default_usd_five_per_utc_day', status: exact && policy.day_timezone === 'UTC' ? 'PASS' : 'FAIL',
      approved_rate_sha256: valuation.approved_rate_sha256});
  }
  return {schema: 'cma-cost-defaults-assessment-v1', checks,
    status: checks.some(row => row.status === 'FAIL') ? 'FAIL'
      : checks.some(row => row.status === 'BLOCKED') ? 'BLOCKED' : 'PASS',
    scope: observation.scope, runtime_acceptance: false};
}

export function assessRefusal(response, code) {
  return response?.status === 429 && response.body?.status === 429
    && response.body?.code === code && typeof response.body?.type === 'string'
    && response.body.type.length > 0;
}

// Runtime qualification is intentionally absent. Before/after allocation lists
// cannot exclude transient allocation. A cancellation acknowledgement cannot
// establish terminal time. Those claims need an independently collected kind
// trace, not caller-supplied "complete" or "same_turn" booleans.
