import { deliverableResource } from './agents/delivery-resource.js';
import { ProtocolError } from './errors.js';
import {
  array,
  boolean,
  enumeration,
  object,
  positiveInteger,
  record,
  tagged,
  text,
  type Output,
  type Parser,
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

function exact<Value extends object>(parse: Parser<Value>): Parser<Value> {
  return (input) => {
    const source = record(input);
    const value = parse(source);
    if (Object.keys(source).some((key) => !Object.hasOwn(value, key)))
      throw new ProtocolError('Task acceptance field is unsupported');
    return value;
  };
}

const taskId = (input: unknown): string => {
  const value = text(input);
  if (!/^[A-Za-z0-9_-]{1,80}$/.test(value)) throw new ProtocolError('Invalid task acceptance ID');
  return value;
};

const requirement = exact(
  object({
    id: taskId,
    required: boolean,
    evidence: tagged(
      {
        plan_step: exact(object({ kind: enumeration('plan_step'), title: text })),
        deliverable: exact(
          object({ kind: enumeration('deliverable'), resource: exact(deliverableResource) }),
        ),
      },
      'kind',
    ),
  }),
);
const requirements = (input: unknown) => {
  if (!Array.isArray(input) || input.length < 1 || input.length > 16)
    throw new ProtocolError('Expected 1 to 16 task acceptance requirements');
  return array(requirement)(input);
};

export const taskAcceptanceIntent = exact(
  object({
    version: (input: unknown): 1 => {
      if (input !== 1) throw new ProtocolError('Unsupported task acceptance version');
      return 1;
    },
    action: tagged({
      declare: exact(object({ type: enumeration('declare'), requirements })),
      retry: exact(
        object({
          type: enumeration('retry'),
          task_id: taskId,
          after_turn_sequence: positiveInteger,
        }),
      ),
    }),
  }),
);

export type TaskAcceptanceIntent = Output<typeof taskAcceptanceIntent>;
