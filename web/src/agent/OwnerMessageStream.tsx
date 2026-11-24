import type { ComponentProps } from 'react';
import type { CmaWebClient } from '../api/client';
import { MessageStream } from './MessageStream';
import { conversationMedia } from './mediaSources';

interface OwnerMessageStreamProps extends Omit<
  ComponentProps<typeof MessageStream>,
  'retainedOutput' | 'deliverables' | 'media'
> {
  readonly client?: CmaWebClient;
  readonly agentId: string;
  readonly viewerId?: string;
  readonly authorized: boolean;
}

/** Keep owner conversation reads scoped to the currently authorized Agent. */
export function OwnerMessageStream({
  client,
  agentId,
  viewerId,
  authorized,
  ...stream
}: OwnerMessageStreamProps) {
  return (
    <MessageStream
      {...stream}
      retainedOutput={client && viewerId && authorized ? { client, agentId, viewerId } : undefined}
      deliverables={
        client?.deliverables && viewerId && authorized
          ? { agentId, viewerId, client: client.deliverables }
          : undefined
      }
      media={conversationMedia(
        agentId,
        client?.files,
        authorized && !stream.retained,
        stream.conversation.runtimeMetadata?.workingDirectory,
      )}
    />
  );
}
