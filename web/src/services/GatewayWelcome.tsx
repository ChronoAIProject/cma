import { Button } from '../ui/Button';
import { DeliveryFlow } from './DeliveryFlow';
import type { ChannelNode } from './channelTopology';
import './ChannelMap.css';

const illustration: readonly (readonly ChannelNode[])[] = [
  [
    {
      key: 'app',
      selection: { kind: 'producer', index: 0 },
      title: 'Your application',
      subtitle: 'Custom events',
      detail: 'Bring your own event source',
      identity: 'custom',
    },
    {
      key: 'chat',
      selection: { kind: 'producer', index: 1 },
      title: 'Your conversations',
      subtitle: 'Messaging platforms',
      detail: 'Connect the places you work',
      identity: 'telegram',
    },
  ],
  [
    {
      key: 'channel',
      selection: { kind: 'channel' },
      title: 'Event channel',
      subtitle: 'One connected journey',
      detail: 'Route each event to its agent',
    },
  ],
  [
    {
      key: 'agent',
      selection: { kind: 'consumer', id: 'illustration' },
      title: 'Chrono Agent',
      subtitle: 'Your managed agent',
      detail: 'Put your agents to work',
      identity: 'cma',
    },
  ],
];
export function GatewayWelcome({
  kind,
  onCreate,
  loading = false,
}: {
  readonly kind: 'channel' | 'consumer';
  readonly onCreate?: () => void;
  readonly loading?: boolean;
}) {
  return (
    <section
      className="gateway-welcome"
      aria-label={`${kind === 'channel' ? 'Channel' : 'Consumer'} workspace`}
    >
      <div className="gateway-welcome__copy">
        <p className="gateway-welcome__eyebrow">Event Gateway</p>
        <h1>{kind === 'channel' ? 'Connect events to action' : 'An agent for every event'}</h1>
        <p>
          {kind === 'channel'
            ? 'Bring your sources and agents together. Choose a channel in the sidebar to explore its delivery map.'
            : 'Choose a consumer in the sidebar, or connect a new agent to the conversations that matter.'}
        </p>
        {onCreate ? (
          <Button variant="primary" onClick={onCreate}>
            New {kind}
          </Button>
        ) : null}
        {loading ? <p role="status">Loading {kind}s…</p> : null}
      </div>
      <div className="gateway-welcome__diagram" aria-label="Illustrative event journey">
        <DeliveryFlow groups={illustration} illustrative />
        <p>From your sources to your agents · An example of what you can connect</p>
      </div>
    </section>
  );
}
