import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { createClient, type AnswerDeliverable } from '@chronoai/cma-sdk';
import { SharedAgent } from '../resources/SharedAgent';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_TRANSCRIPT } from '../test/agent';
import { DeliverablePreviewFrame } from './DeliverablePreviewFrame';

it('lets a history-only reader preview an independently authorized document but denies private files', async () => {
  const document: AnswerDeliverable = {
    id: `delivery_${'a'.repeat(64)}`,
    reference: {
      agent_id: 'agt_1',
      resource: { kind: 'document', id: `doc_${'a'.repeat(32)}`, revision: 1 },
      name: 'shared.md',
      media_type: 'text/markdown',
      size_bytes: 3,
      sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
    },
    status: 'ready',
  };
  const privateFile: AnswerDeliverable = {
    ...document,
    id: `delivery_${'b'.repeat(64)}`,
    reference: {
      ...document.reference,
      resource: { kind: 'live_file', path: 'private.txt' },
      name: 'private.txt',
      media_type: 'application/octet-stream',
    },
    status: 'denied',
  };
  let revoked = false;
  const sdk = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      if (path.includes('/resource-access/'))
        return Response.json({
          capabilities: ['view_details', 'view_history'],
          policy_revision: 1,
          revalidate_at: new Date(Date.now() + 50_000).toISOString(),
        });
      if (path.endsWith('/provenance'))
        return Response.json({
          agent_id: 'agt_1',
          created_at: '2026-11-03T00:00:00Z',
          creation_origin: 'manual',
        });
      const identity = { agent_id: 'agt_1', answer_id: 'event:3', turn_sequence: 1 };
      if (path.endsWith('/deliverables'))
        return Response.json({ ...identity, items: [document, privateFile] });
      if (path.endsWith('/download'))
        return new Response('abc', { headers: { 'content-type': 'application/octet-stream' } });
      if (path.includes('/answers/'))
        return Response.json({
          ...identity,
          item: path.endsWith(privateFile.id)
            ? privateFile
            : revoked
              ? { ...document, status: 'denied' }
              : document,
        });
      throw new Error(`Unexpected public route ${path}`);
    },
  });
  const resources = {
    agents: sdk.agents,
    resourceAccess: sdk.resourceAccess,
    sandboxes: sdk.sandboxes,
    workspaces: sdk.workspaces,
  };
  const marker = [document, privateFile]
    .map(
      (item) =>
        `<!--cma-deliverable:v1:${btoa(JSON.stringify(item.reference)).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')}-->`,
    )
    .join('\n');
  const web = agentClient({
    resourceViews: resources,
    deliverables: sdk.agents,
    readAgentTranscript: async () => ({
      ...AGENT_TRANSCRIPT,
      turns: AGENT_TRANSCRIPT.turns.map((turn) => ({
        ...turn,
        messages: turn.messages.map((message) =>
          message.role === 'assistant' ? { ...message, content: marker } : message,
        ),
      })),
    }),
  });
  render(
    <CmaClientProvider client={web}>
      <DeliverablePreviewFrame
        detailsOpen={false}
        onPreviewOpen={() => {}}
        sidebar={null}
        center={
          <SharedAgent
            viewerId="guest"
            client={resources}
            web={web}
            detail={{
              access: 'shared',
              id: 'agt_1',
              title: 'Shared answer',
              status: 'ready',
              created_at: '2026-11-03T00:00:00Z',
              updated_at: '2026-11-03T00:00:00Z',
            }}
            onAccessChanged={() => {}}
            detailsOpen={false}
            onDetailsOpenChange={() => {}}
          />
        }
      />
    </CmaClientProvider>,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Preview shared.md' }));
  let preview = within(screen.getByRole('complementary', { name: 'Deliverable preview' }));
  expect(await preview.findByText('abc')).toBeVisible();
  expect(preview.queryByRole('button', { name: 'Save' })).toBeNull();
  expect(screen.queryByRole('textbox', { name: 'Message' })).toBeNull();
  revoked = true;
  fireEvent.focus(window);
  expect(await preview.findByText('Access unavailable')).toBeVisible();
  expect(preview.queryByText('abc')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Preview private.txt' }));
  preview = within(screen.getByRole('complementary', { name: 'Deliverable preview' }));
  expect(await preview.findByText('Access unavailable')).toBeVisible();
  expect(preview.getByRole('button', { name: 'Download private.txt' })).toBeDisabled();
});
