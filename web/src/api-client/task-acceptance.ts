import { deliverableResource, type DeliverableResource } from './agents/delivery-resource.js';
import {
  array,
  enumeration,
  object,
  positiveInteger,
  tagged,
  text,
  type Output,
} from './schema.js';

const reason = enumeration(
  'required_steps_incomplete',
  'required_deliverables_missing',
  'interrupted',
  'execution_failed',
  'history_incomplete',
);

export const taskAcceptance = object({
  task_id: text,
  turn_sequence: positiveInteger,
  verdict: tagged(
    {
      in_progress: object({ status: enumeration('in_progress') }),
      succeeded: object({ status: enumeration('succeeded') }),
      needs_attention: object({ status: enumeration('needs_attention'), reason }),
      indeterminate: object({ status: enumeration('indeterminate'), reason }),
    },
    'status',
  ),
  unconfirmed_outputs: array(deliverableResource),
});

export type TaskAcceptance = Output<typeof taskAcceptance>;

interface TaskRequirement {
  readonly id: string;
  readonly required: boolean;
  readonly evidence:
    | { readonly kind: 'plan_step'; readonly title: string }
    | { readonly kind: 'deliverable'; readonly resource: DeliverableResource };
}

export interface TaskAcceptanceIntent {
  readonly version: 1;
  readonly action:
    | { readonly type: 'declare'; readonly requirements: readonly TaskRequirement[] }
    | { readonly type: 'retry'; readonly task_id: string; readonly after_turn_sequence: number };
}
