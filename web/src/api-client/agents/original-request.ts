import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { ProtocolError } from '../errors.js';
import { read, segment } from '../resources.js';
import {
  enumeration,
  nullable,
  object,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';

const originalRequest = tagged(
  {
    ready: object({
      status: enumeration('ready'),
      agent_id: text,
      original_request: object({
        text,
        source: enumeration('admitted_first_task'),
        turn_sequence: nullable(positiveInteger),
      }),
    }),
    unavailable: object({
      status: enumeration('unavailable'),
      agent_id: text,
      original_request: (value: unknown): null => {
        if (value !== null) throw new ProtocolError('unavailable original request must be absent');
        return null;
      },
    }),
  },
  'status',
);
export type AgentOriginalRequest = Output<typeof originalRequest>;

export function readOriginalRequest(
  config: ResolvedConfig,
  id: string,
  options: OperationOptions = {},
) {
  return read(
    config,
    `/api/v1/agents/${segment(id)}/original-request`,
    (value) => {
      const result = originalRequest(value);
      if (
        result.agent_id !== id ||
        (result.status === 'ready' &&
          new TextEncoder().encode(result.original_request.text).length > 256 * 1024)
      )
        throw new ProtocolError('Original request has invalid identity or exceeds the text bound');
      return result;
    },
    { ...options, cache: 'no-store' },
  );
}
