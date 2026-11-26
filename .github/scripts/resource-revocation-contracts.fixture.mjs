import { createHash } from 'node:crypto';
import { releaseSchedules } from './resource-revocation-contracts.mjs';

export const hash = value => createHash('sha256').update(value).digest('hex');
export const binding = {
  source_revision: 'a'.repeat(40),
  api_image_sha256: hash('synthetic-api'),
  runtime_image_sha256: hash('synthetic-runtime'),
  web_bundle_sha256: hash('synthetic-web'),
  configuration_sha256: hash('synthetic-config'),
  observer_sha256: hash('synthetic-observer'),
  route: 'retained_output',
};
export const schedules = releaseSchedules();
// These are fabricated observer reports for checker tests, not candidate runs.
export function observation(schedule, origin = 'synthetic') {
  let shared = 0;
  const chunks = [0, 0];
  let closed = false;
  const registered = schedule.actions.indexOf('register_body_attempt') < schedule.actions.indexOf('freeze_commit');
  const authorized = registered && schedule.actions.indexOf('final_check_attempt') < schedule.actions.indexOf('freeze_commit');
  return {
    schema: 1, schedule_id: schedule.id, origin, binding: { ...binding },
    execution: 'observed', boundary: 'server_transport_handoff',
    identity: Object.fromEntries(['operation', 'grant', 'release', 'api_boot'].map(key => [key, hash(key + schedule.id)])),
    events: [...schedule.actions, 'observation_complete'].map((action, index) => {
      if (action === 'close_body' || action === 'close_release_gate') closed = true;
      if (action.includes('handoff_attempt') && authorized && !closed) {
        chunks[action === 'chunk_0_handoff_attempt' ? 0 : 1] += 8;
        shared += 8;
      }
      return {
        action, sequence: index + 1, elapsed_ms: index * 10,
        private_bytes_total: shared,
        private_bytes_by_chunk: [...chunks],
        owner_bytes_total: action === 'observation_complete' ? 12 : 0,
        buffered_private_bytes: 0,
        ...(action === 'read_source_attempt' ? { source_private_bytes: registered ? 16 : 0 } : {}),
        ...(action === 'final_check_attempt' ? { authorized } : {}),
        ...(['durable_release_ack', 'effective_commit'].includes(action)
          ? { receipt_sha256: hash(action + schedule.id) } : {}),
      };
    }),
  };
}
