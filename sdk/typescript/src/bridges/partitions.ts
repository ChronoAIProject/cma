import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { input, query, read, segment, type PageOptions } from '../resources.js';
import { positiveInteger } from '../schema.js';
import { allocationId } from './cleanup-schema.js';
import { boundedId } from './invoke-schema.js';
import { BridgeSourceControls } from './source-controls.js';
import * as schema from './partitions-schema.js';

function path(id: string) {
  input(boundedId, id);
  return `/api/v1/bridges/${segment(id)}/partitions`;
}
/** Historical partition generations and exact Channel response status. Reads admit no work. */
export class BridgePartitions extends BridgeSourceControls {
  listPartitions(id: string, options: PageOptions = {}) {
    const endpoint = path(id);
    const limit = options.limit ?? 25;
    if (input(positiveInteger, limit) > 25) throw new ValidationError('partition limit exceeds 25');
    const after =
      options.cursor === undefined
        ? undefined
        : input((value) => schema.cursorAfter(String(value), id), options.cursor);
    return read(
      this.config,
      query(endpoint, { cursor: options.cursor, limit: options.limit }),
      (value) => {
        const result = schema.page(value);
        if (
          result.items.length > limit ||
          result.items.some(
            (item, i) =>
              item.bridge_id !== id ||
              item.id <= (i === 0 ? (after ?? '') : result.items[i - 1]!.id),
          )
        )
          throw new ProtocolError('partition page changed Bridge, ordering or limit');
        if (
          result.next_cursor !== null &&
          (!result.items.length ||
            schema.cursorAfter(result.next_cursor, id) !== result.items.at(-1)!.id)
        )
          throw new ProtocolError('partition continuation does not match its page');
        return result;
      },
      options,
    );
  }
  readPartition(id: string, allocation: string, options: OperationOptions = {}) {
    input(allocationId, allocation);
    return read(
      this.config,
      `${path(id)}/${segment(allocation)}`,
      (value) => {
        const result = schema.detail(value);
        if (result.partition.bridge_id !== id || result.partition.id !== allocation)
          throw new ProtocolError('partition detail changed Bridge or allocation');
        return result;
      },
      options,
    );
  }
}
