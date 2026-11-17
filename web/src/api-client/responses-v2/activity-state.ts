import { ProtocolError } from '../errors.js';
import type { V2ActivityItem, V2ResponseEvent } from './types.js';

/** Validate the activity-only lifecycle, including replay beginning inside a call. */
export class ActivityState {
  private snapshot?: V2ActivityItem;
  private stage?: string;
  private status?: V2ActivityItem['status'];
  private done = false;
  constructor(private readonly replay: boolean) {}

  accept(event: V2ResponseEvent) {
    const invalid = () => new ProtocolError('Responses activity lifecycle is inconsistent');
    const requireStage = (stage: string) => {
      if (this.stage !== stage && !(this.replay && this.stage === undefined)) throw invalid();
    };
    if (this.done) throw invalid();
    if ('item' in event && event.item.type === 'mcp_call') {
      const output = event.item;
      if (this.snapshot && !sameCall(this.snapshot, output)) throw invalid();
      if (event.type === 'response.output_item.added') {
        if (this.stage !== undefined || output.status !== 'in_progress') throw invalid();
        this.stage = 'added';
      } else {
        if (output.status === 'in_progress') throw invalid();
        requireStage(output.status);
        this.done = true;
      }
      this.snapshot = output;
      this.status = output.status;
    } else if (event.type === 'response.mcp_call.in_progress') {
      requireStage('added');
      this.stage = 'in_progress';
      this.status = 'in_progress';
    } else if (
      event.type === 'response.mcp_call.completed' ||
      event.type === 'response.mcp_call.failed'
    ) {
      requireStage('in_progress');
      this.status = event.type === 'response.mcp_call.completed' ? 'completed' : 'failed';
      this.stage = this.status;
    } else throw invalid();
  }

  matches(output: V2ActivityItem): boolean {
    return (
      (this.done || this.stage === 'in_progress') &&
      output.status === this.status &&
      (this.snapshot === undefined || sameCall(this.snapshot, output))
    );
  }
}
function sameCall(a: V2ActivityItem, b: V2ActivityItem): boolean {
  return a.id === b.id && a.name === b.name && a.server_label === b.server_label;
}
