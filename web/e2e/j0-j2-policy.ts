import { settings } from './environment';

export const journeySlice = {
  scope: 'j0_j2_partial',
  fullCampaignGatesAdvanced: false,
  deferredJourneys: ['J3', 'J4', 'J5', 'J6', 'J7'],
  samplingTargetsSatisfied: false,
} as const;

export function requireJourneyMutation(env = process.env) {
  if (env.CMA_E2E_MODE !== 'mutation')
    throw new Error('Required E2E J0-J2 mode: explicit mutation authorization');
  return settings(env);
}

export function assertJourneySelection(titles: string[]) {
  const ids = titles.map((title) => title.split(' ')[0]);
  if (ids.length !== 3 || ['J0', 'J1', 'J2'].some((id) => ids.filter((v) => v === id).length !== 1))
    throw new Error('Invalid E2E J0-J2 selection: exactly one case for each journey is required');
}

export interface JourneyResult {
  case: string;
  status: string;
  retry: number;
  durationMs: number;
}

export function journeyResult(status: string, cases: JourneyResult[]) {
  let complete = false;
  try {
    assertJourneySelection(cases.map((row) => row.case));
    complete = cases.every((row) => row.status === 'passed' && row.retry === 0);
  } catch {
    // An interrupted or filtered execution cannot be a completed slice.
  }
  return {
    ...journeySlice,
    status: status === 'passed' && complete ? 'passed' : 'failed',
    attempted: cases.filter((row) => row.status !== 'skipped').length,
    passed: cases.filter((row) => row.status === 'passed' && row.retry === 0).length,
    cases,
  };
}
