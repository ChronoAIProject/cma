import { useState } from 'react';
import type { Bridge, BridgeAcceptedWork, BridgeControl } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Picker } from '../ui/Picker';
import { RiskConfirmation } from '../ui/RiskConfirmation';

export function BridgeLifecycle({
  bridge,
  disabled,
  connectionVerified,
  onControl,
}: {
  readonly bridge: Bridge;
  readonly disabled: boolean;
  readonly connectionVerified: boolean;
  readonly onControl: (command: BridgeControl) => void;
}) {
  const [work, setWork] = useState<BridgeAcceptedWork>('suspend_original');
  const [retire, setRetire] = useState(false);
  const controls = bridge.emergency;
  return (
    <section className="bridges__fields" aria-labelledby="bridge-lifecycle-title">
      <h2 id="bridge-lifecycle-title">Bridge controls</h2>
      <p>Current state: {bridge.state.kind}</p>
      {bridge.state.kind === 'retired' ? (
        <p>
          This bridge is retired. Retained resources and conversation history remain available under
          their current permissions.
        </p>
      ) : (
        <>
          <Picker
            label="Already accepted work"
            value={work}
            disabled={disabled}
            options={[
              { value: 'suspend_original', label: 'Suspend already accepted work' },
              { value: 'drain_original', label: 'Finish accepted work with its original settings' },
            ]}
            onChange={setWork}
          />
          <div className="bridges__actions">
            <Button
              disabled={disabled || !connectionVerified || bridge.state.kind === 'enabled'}
              onClick={() => onControl({ kind: 'enable' })}
            >
              Enable bridge
            </Button>
            <Button
              disabled={disabled || bridge.state.kind !== 'enabled'}
              onClick={() => onControl({ kind: 'disable', accepted_work: work })}
            >
              Disable bridge
            </Button>
            <Button disabled={disabled} onClick={() => setRetire(true)}>
              Retire bridge
            </Button>
          </div>
          {!connectionVerified ? (
            <p>
              Validate the current connection before enabling. The server also checks current
              authority and target requirements.
            </p>
          ) : null}
          <div role="group" aria-label="Emergency bridge controls">
            {(
              [
                ['ingress_stopped', 'Incoming messages'],
                ['execution_stopped', 'Agent execution'],
                ['outbound_stopped', 'Outgoing replies'],
              ] as const
            ).map(([key, label]) => (
              <div className="bridges__actions" key={key}>
                <span>
                  {label}: {controls[key] ? 'Stopped' : 'Allowed by this control'}
                </span>
                <Button
                  disabled={disabled}
                  onClick={() =>
                    onControl({
                      kind: 'emergency',
                      controls: { ...controls, [key]: !controls[key] },
                    })
                  }
                >
                  {controls[key] ? 'Resume' : 'Stop'} {label.toLowerCase()}
                </Button>
              </div>
            ))}
          </div>
        </>
      )}
      <RiskConfirmation
        open={retire}
        title="Retire this bridge"
        description="This permanently stops new messages through this bridge. Existing Agents, resources and conversation history are retained. The selected policy applies to already accepted work."
        acknowledgeLabel="I understand this bridge cannot be enabled again."
        confirmLabel="Retire bridge"
        busy={disabled}
        onCancel={() => setRetire(false)}
        onConfirm={() => {
          setRetire(false);
          onControl({ kind: 'retire', accepted_work: work });
        }}
      />
    </section>
  );
}
