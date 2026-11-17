import type { ResolvedConfig } from './config.js';
import { requestVoid, type OperationOptions } from './http.js';
import { input, query, read, segment } from './resources.js';
import { integer, nullable, object, positiveInteger, text, type Output } from './schema.js';

const record = object({
  record_id: text,
  owner: text,
  blob: text,
  version: positiveInteger,
  created_at: text,
  updated_at: text,
});
const receipt = object({ record_id: text, version: positiveInteger, updated_at: text });
const put = object({
  owner: text,
  blob: text,
  expected_version: nullable(positiveInteger),
  expires_at: integer,
});
export type PutGatewayInteraction = Output<typeof put>;
const base = '/api/v1/gateway/interaction-records';
/** Gateway service authority only. CMA stores expiring ciphertext without interpreting it. */
export class GatewayInteractions {
  constructor(private readonly config: ResolvedConfig) {}
  put(id: string, body: PutGatewayInteraction, options: OperationOptions = {}) {
    return read(this.config, `${base}/${segment(id)}`, receipt, {
      ...options,
      method: 'PUT',
      body: input(put, body),
    });
  }
  get(id: string, options: OperationOptions = {}) {
    return read(this.config, `${base}/${segment(id)}`, record, options);
  }
  delete(id: string, expectedVersion: number, options: OperationOptions = {}) {
    return requestVoid(
      this.config,
      query(`${base}/${segment(id)}`, { expected_version: positiveInteger(expectedVersion) }),
      {
        ...options,
        method: 'DELETE',
        expectedStatus: 204,
      },
    );
  }
}
