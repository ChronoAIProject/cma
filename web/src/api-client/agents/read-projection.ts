import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { read, segment } from '../resources.js';
import type { Parser } from '../schema.js';

export function readAgentProjection<Value extends { readonly agent_id: string }>(
  config: ResolvedConfig,
  id: string,
  name: string,
  parse: Parser<Value>,
  options: OperationOptions,
) {
  return read(
    config,
    `/api/v1/agents/${segment(id)}/${name}`,
    (value) => {
      const projection = parse(value);
      if (projection.agent_id !== id)
        throw new ProtocolError('projection Agent identity did not match the request');
      return projection;
    },
    options,
  );
}
