import type { ResolvedConfig } from './config.js';
import { requestVoid, type OperationOptions } from './http.js';
import { input, query, read, segment } from './resources.js';
import { array, nullable, object, positiveInteger, text, type Output } from './schema.js';

const recordHead = object({
  record_id: text,
  owner: text,
  version: positiveInteger,
  updated_at: text,
});
const record = object({
  record_id: text,
  owner: text,
  blob: text,
  version: positiveInteger,
  created_at: text,
  updated_at: text,
});
const putRecord = object({ owner: text, blob: text, expected_version: nullable(positiveInteger) });
const receipt = object({ record_id: text, version: positiveInteger, updated_at: text });
export type GatewayRecord = Output<typeof record>;
export type GatewayRecordHead = Output<typeof recordHead>;
export type PutGatewayRecord = Output<typeof putRecord>;
export type GatewayRecordReceipt = Output<typeof receipt>;
export interface GatewayRecordListOptions extends OperationOptions {
  readonly owner?: string;
  readonly after?: string;
  readonly limit?: number;
}
/** Management-only encrypted record storage. Blobs contain base64 ciphertext. */
export class GatewayRecords {
  private readonly base: string;

  constructor(
    private readonly config: ResolvedConfig,
    namespace: 'channel-records' | 'connection-records' = 'channel-records',
  ) {
    this.base = `/api/v1/gateway/${namespace}`;
  }
  put(id: string, body: PutGatewayRecord, options: OperationOptions = {}) {
    return read(this.config, `${this.base}/${segment(id)}`, receipt, {
      ...options,
      method: 'PUT',
      body: input(putRecord, body),
    });
  }
  get(id: string, options: OperationOptions = {}) {
    return read(this.config, `${this.base}/${segment(id)}`, record, options);
  }
  list(options: GatewayRecordListOptions = {}) {
    return read(
      this.config,
      query(this.base, { owner: options.owner, after: options.after, limit: options.limit }),
      object({ items: array(recordHead), next_after: nullable(text) }),
      options,
    );
  }
  delete(id: string, expectedVersion?: number, options: OperationOptions = {}) {
    return requestVoid(
      this.config,
      query(`${this.base}/${segment(id)}`, { expected_version: expectedVersion }),
      {
        ...options,
        method: 'DELETE',
        expectedStatus: 204,
      },
    );
  }
}
