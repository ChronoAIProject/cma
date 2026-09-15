import { useMemo } from 'react';
import type { BridgePlatform } from '@chronoai/cma-sdk';
import type { BridgeViewsClient } from './client';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import { Button } from '../ui/Button';

export function BridgeCapabilities({
  client,
  platform,
  organization,
}: {
  readonly client: BridgeViewsClient;
  readonly platform: BridgePlatform;
  readonly organization?: string;
}) {
  const read = useMemo(
    () => async (signal: AbortSignal) => ({
      value: await client.bridges.capabilities({ organization, signal }),
    }),
    [client, organization],
  );
  const current = useAuthorizedRead(read);
  const catalog = current.state.status === 'ready' ? current.state.value : undefined;
  const capability = catalog?.connectors.find((item) => item.platform === platform);
  return (
    <section aria-label="Connector capabilities">
      <h3>What this connection supports</h3>
      {current.state.status === 'loading' ? (
        <p role="status">Checking connector capabilities</p>
      ) : null}
      {current.state.status === 'error' || (catalog !== undefined && capability === undefined) ? (
        <p role="status">
          Connector capabilities are unavailable.{' '}
          <Button onClick={current.reload}>Check again</Button>
        </p>
      ) : null}
      {capability === undefined ? null : (
        <>
          <p>
            {capability.inbound_text
              ? 'Text messages are supported.'
              : 'Text messages are unavailable.'}{' '}
            {capability.output_text
              ? 'In conversations created by this channel, the agent can use channel reply tools to send text.'
              : 'Channel replies are unavailable.'}
          </p>
          <p>
            {capability.attachment_input === 'download_required'
              ? 'Messages with attachments need a download capability that is not available. Send text separately.'
              : 'Attachments are unavailable.'}{' '}
            {capability.output_edit ? 'Reply edits are supported.' : 'Reply edits are unavailable.'}
          </p>
          <p>
            {capability.live_verified
              ? 'Live platform delivery has been verified.'
              : 'Live platform delivery has not been verified.'}{' '}
            {capability.evidence === 'contract_tested'
              ? 'Supported message shapes have passed protocol fixtures.'
              : 'This connector is not available for CMA bot conversations.'}
          </p>
          <dl>
            <dt>Message intake</dt>
            <dd>{capability.stages.ingress ? 'Enabled' : 'Disabled'}</dd>
            <dt>Agent execution</dt>
            <dd>{capability.stages.execution ? 'Enabled' : 'Disabled'}</dd>
            <dt>Channel replies</dt>
            <dd>{capability.stages.outbound ? 'Enabled' : 'Disabled'}</dd>
            <dt>Connection verification</dt>
            <dd>{catalog?.connection_validation_available ? 'Available' : 'Unavailable'}</dd>
          </dl>
          <p>
            Deployment settings are shown above. Processing and replies also require a valid
            connection, current access and any required consent.
          </p>
        </>
      )}
    </section>
  );
}
