import { useState } from 'react';
import type { BridgePartition } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import type { PartitionAction, PartitionCommandController } from './usePartitionCommand';

export function PartitionCommands({
  partition,
  revision,
  allowed,
  command,
}: {
  readonly partition?: BridgePartition;
  readonly revision?: number;
  readonly allowed: boolean;
  readonly command: PartitionCommandController;
}) {
  const [review, setReview] = useState<{
    partition: BridgePartition;
    revision: number;
    action: PartitionAction;
  }>();
  const alreadyAccepted =
    command.accepted?.allocation_id === partition?.id &&
    command.accepted?.previous_revision === partition?.revision;
  const available =
    !alreadyAccepted &&
    allowed &&
    partition?.current === true &&
    partition.target.status !== 'redacted' &&
    revision !== undefined &&
    !command.busy &&
    command.pending === undefined;
  const canDetach = available && partition.state !== 'retired';
  const reviewed =
    review !== undefined &&
    available &&
    partition.id === review.partition.id &&
    partition.revision === review.partition.revision &&
    partition.generation === review.partition.generation &&
    revision === review.revision &&
    JSON.stringify(partition.target) === JSON.stringify(review.partition.target) &&
    (review.action !== 'detach' || canDetach);
  const accepted = command.accepted;
  if (!allowed) return null;
  function open(action: PartitionAction) {
    if (available) setReview({ partition: structuredClone(partition), action, revision });
  }
  const target = review?.partition.target;
  const exact =
    target === undefined || target.status === 'redacted'
      ? ''
      : `Agent ${target.ids.agent_id}, generation ${review!.partition.generation}. `;
  const description =
    review?.action === 'detach'
      ? `${exact}Stop new Channel input for this partition. The existing Agent conversation and history remain available. Accepted work retains its original identity. This does not remove resources.`
      : review?.partition.placement === 'existing_agent'
        ? `${exact}Create the next binding generation for this same retained Agent. Its existing conversation and context continue unchanged. This does not clear conversation history.`
        : `${exact}Create the next partition generation using the original configuration. The previous Agent, conversation and history remain available. New resources may take time to become ready.`;
  return (
    <div className="bridges__fields">
      <h4>Partition management</h4>
      {partition?.current === false ? (
        <p>This is a previous generation. Select the current partition to reset or detach.</p>
      ) : null}
      {partition?.current === true && partition.state === 'retired' ? (
        <p>This current generation is retired. An explicit reset can create a new generation.</p>
      ) : null}
      <div className="bridges__actions">
        <Button disabled={!available} onClick={() => open('reset')}>
          Reset partition generation
        </Button>
        <Button disabled={!canDetach} onClick={() => open('detach')}>
          Detach partition
        </Button>
      </div>
      {command.busy ? (
        <p role="status">Checking acceptance of the original partition request</p>
      ) : null}
      {command.message === undefined ? null : <p role="status">{command.message}</p>}
      {command.pending === undefined ? null : (
        <Button disabled={command.busy || revision === undefined} onClick={command.recover}>
          Recover original partition request
        </Button>
      )}
      {accepted === undefined ? null : (
        <p role="status">
          {accepted.action === 'detach'
            ? `Partition generation ${accepted.generation} detached. The Agent conversation and history are retained.`
            : `Reset accepted. Generation ${accepted.successor?.generation} is ${accepted.successor?.state.replaceAll('_', ' ')}. Refresh partitions to inspect it; the previous conversation remains available.`}
        </p>
      )}
      <RiskConfirmation
        open={review !== undefined}
        title={
          review?.action === 'detach'
            ? 'Detach this partition?'
            : 'Reset this partition generation?'
        }
        description={description}
        acknowledgeLabel={
          review?.action === 'detach'
            ? 'I understand that new Channel input will stop for this partition.'
            : 'I understand that this creates a new binding generation and retains the existing history.'
        }
        confirmLabel={review?.action === 'detach' ? 'Confirm detach' : 'Confirm generation reset'}
        busy={command.busy}
        confirmDisabled={!reviewed}
        status={
          review !== undefined && !reviewed
            ? 'Current access or revisions changed. Cancel and refresh before reviewing this partition again.'
            : undefined
        }
        onCancel={() => setReview(undefined)}
        onConfirm={() => {
          if (reviewed) {
            command.submit(review.partition, review.action, review.revision);
            setReview(undefined);
          }
        }}
      />
    </div>
  );
}
