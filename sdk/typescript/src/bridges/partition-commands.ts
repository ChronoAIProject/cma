import { ProtocolError } from '../errors.js';
import { command, input, read, segment, type CommandOptions } from '../resources.js';
import { boundedId } from './invoke-schema.js';
import { allocationId } from './cleanup-schema.js';
import { BridgePartitions } from './partitions.js';
import * as schema from './partition-commands-schema.js';

/** Explicit generation changes preserve the old Agent conversation and history. */
export class BridgePartitionCommands extends BridgePartitions {
  /** Create a successor binding. An existing-Agent target keeps its native conversation. */
  resetPartition(
    id: string,
    allocation: string,
    body: schema.PartitionCommandRequest,
    options: CommandOptions,
  ) {
    return this.partitionCommand(id, allocation, 'reset', body, options);
  }
  /** Stop new partition admission and retain accepted work, resources and history. */
  detachPartition(
    id: string,
    allocation: string,
    body: schema.PartitionCommandRequest,
    options: CommandOptions,
  ) {
    return this.partitionCommand(id, allocation, 'detach', body, options);
  }
  private partitionCommand(
    id: string,
    allocation: string,
    action: schema.PartitionCommandAction,
    body: schema.PartitionCommandRequest,
    options: CommandOptions,
  ) {
    input(boundedId, id);
    input(allocationId, allocation);
    const captured = schema.request(input(schema.request, body));
    const capturedOptions = command(options);
    return read(
      this.config,
      `/api/v1/bridges/${segment(id)}/partitions/${segment(allocation)}/${action}`,
      (value) => {
        const receipt = schema.receipt(value);
        if (
          receipt.bridge_id !== id ||
          receipt.allocation_id !== allocation ||
          receipt.action !== action ||
          receipt.expected_bridge_revision !== captured.expected_bridge_revision ||
          receipt.previous_revision !== captured.expected_revision
        )
          throw new ProtocolError('partition command receipt changed its exact original request');
        return receipt;
      },
      { ...capturedOptions, method: 'POST', body: captured, expectedStatus: 202 },
    );
  }
}
