// Integrator: merge these definitions/setups into prod-verifier-03's contribution.
// Do not register this nested module as the developer-17 USER assignment.
import { cases as channels } from './channels.mjs';
import { cases as events } from './events.mjs';
import { cases as connections } from './connections.mjs';
import { cases as consumers } from './consumers.mjs';
import { cases as consoleCases } from './console.mjs';
export { setups } from './setup.mjs';
export const cases = [...channels, ...events, ...connections, ...consumers, ...consoleCases].sort(
  (a, b) => a.id.localeCompare(b.id),
);
