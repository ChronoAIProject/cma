import {runJourney, STEPS} from './journey.mjs';

export const journey = Object.freeze({
  id: 'M-FUNC-01:J0-J7',
  scope: 'agent',
  steps: [...STEPS],
  fixtures: ['fresh-identity', 'registration-evidence', 'browser', 'runtime-controls', 'lifecycle-evidence'],
  run: runJourney,
});

export {runJourney, STEPS};
export {validateLocalKindBinding} from './binding.mjs';
export {invokeJourney} from './invoke.mjs';
