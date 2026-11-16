import {check, required, equal} from './common.mjs';

const unsigned = value => Number.isSafeInteger(value) && value >= 0;
export const cloudTime = value => {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/.test(value)) return NaN;
  const time = Date.parse(value);
  return Number.isFinite(time) && time >= 0 && new Date(time).toISOString().replace('.000Z', 'Z') === value ? time : NaN;
};
const instant = value => Number.isFinite(cloudTime(value));
const cloud = value => value && unsigned(value.units) && unsigned(value.settings_revision)
  && instant(value.observed_through) && instant(value.recorded_at)
  && cloudTime(value.recorded_at) >= cloudTime(value.observed_through);
const ledger = value => value && instant(value.period_start) && instant(value.period_end)
  && cloudTime(value.period_start) < cloudTime(value.period_end)
  && ['settings_revision', 'spent', 'reserved', 'adjustments'].every(k => unsigned(value[k]))
  && (!Object.hasOwn(value, 'cloud') || (cloud(value.cloud)
    && cloudTime(value.cloud.observed_through) >= cloudTime(value.period_start)
    && cloudTime(value.cloud.observed_through) <= cloudTime(value.period_end)))
  && (!Object.hasOwn(value, 'evaluated_at') || instant(value.evaluated_at))
  && Array.isArray(value.closures) && value.closures.every(c => c
    && ['spend_warning', 'spend_stop', 'metering_unavailable'].includes(c.reason)
    && instant(c.since) && unsigned(c.settings_revision) && unsigned(c.committed_at_entry));

export function validateObservationSpend(value) {
  if (!value || typeof value.metering_enabled !== 'boolean' || !unsigned(value.settings_revision)) return false;
  if (!value.metering_enabled) return ['levels', 'committed', 'ledger'].every(k => !Object.hasOwn(value, k));
  return !!value.levels && ['warning', 'stop', 'disposable'].every(k => unsigned(value.levels[k]))
    && (!Object.hasOwn(value, 'committed') || unsigned(value.committed))
    && (!Object.hasOwn(value, 'ledger') || !!ledger(value.ledger));
}

// committed includes a projection to the current wall time. Everything else is
// compared unchanged; all raw responses, including committed, stay in evidence.
export function durableSpend(value) {const {committed, ...state} = value; return state;}
export function settingsControl(value) {const {platform_spend_writes_available, ...state} = value; return state;}

export function requireObservation(f, settings, state) {
  const through = cloudTime(f.observedThrough), platform = settings.settings?.model_budget?.platform;
  required(f.exclusiveSettings === true && typeof f.setWritesAvailable === 'function'
    && Number.isSafeInteger(f.units) && f.units > 0 && Number.isFinite(through) && through <= Date.now(),
  'admin_cloud_observation_fixture_missing');
  required(state.metering_enabled === true && settings.platform_spend_writes_available === true,
    'admin_cloud_writable_metering_fixture_missing');
  required(platform && Number.isFinite(Date.parse(platform.period_start)) && Number.isFinite(Date.parse(platform.period_end))
    && through >= Date.parse(platform.period_start) && through <= Date.parse(platform.period_end)
    && Date.parse(platform.period_start) < Date.parse(platform.period_end)
    && state.settings_revision === settings.revision, 'admin_cloud_period_fixture_missing');
  required(!state.ledger?.cloud || through > cloudTime(state.ledger.cloud.observed_through),
    'admin_cloud_fresh_observation_required');
}

export function acceptedObservation(initial, settings, body, response) {
  check(validateObservationSpend(response), 'admin_cloud_spend_shape');
  const actual = response.ledger?.cloud;
  check(actual && actual.units === body.units && actual.observed_through === body.observed_through
    && actual.settings_revision === settings.revision, 'admin_cloud_observation_not_persisted');
  const platform = settings.settings.model_budget.platform;
  const stamp = value => new Date(value).toISOString().replace('.000Z', 'Z');
  const previous = initial.ledger ?? {period_start: stamp(platform.period_start), period_end: stamp(platform.period_end),
    settings_revision: settings.revision, spent: 0, reserved: 0, adjustments: 0, closures: []};
  check(equal(durableSpend(response), durableSpend({...initial, ledger: {...previous, cloud: actual}})),
    'admin_cloud_success_changed_unrelated_state');
  return structuredClone(response);
}

export function invalidCloudObservations(body, periodStart) {
  return [
    {...body, units: Number.MAX_SAFE_INTEGER + 1}, {...body, units: -1}, {...body, units: 1.5},
    {...body, observed_through: new Date(Math.floor(Date.now() / 1000) * 1000 + 86400000).toISOString().replace('.000Z', 'Z')},
    {...body, observed_through: new Date(Date.parse(periodStart) - 1000).toISOString().replace('.000Z', 'Z')},
    {...body, observed_through: 'yesterday'}, {...body, source: 'invalid'}, {...body, extra: true},
  ];
}
