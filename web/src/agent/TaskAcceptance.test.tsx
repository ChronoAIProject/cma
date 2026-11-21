import { fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { createClient, type AnswerDeliverable, type TaskAcceptance } from '@cma/client';
import { AgentHeader } from './AgentHeader';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript } from './conversation';
import { taskStateMeta, taskStatus } from '../shell/taskStatus';

const agentId = 'agt_task';
const output: AnswerDeliverable = {
  id: `delivery_${'a'.repeat(64)}`,
  reference: {
    agent_id: agentId,
    resource: { kind: 'live_file', path: 'out/report.csv' },
    name: 'report.csv',
    media_type: 'application/octet-stream',
    size_bytes: 3,
    sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
  },
  status: 'unchecked',
};

const marker = `<!--cma-deliverable:v1:${btoa(JSON.stringify(output.reference)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '')}-->`;
const conversation = {
  ...conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }),
  plans: [
    { id: 'plan:4', runId: '3', entries: [{ title: 'Write the report', status: 'completed' }] },
  ],
  turnOutcomes: { 3: 'completed' as const, 7: 'completed' as const },
  messages: [
    { id: 'event:5', runId: '3', content: `Required answer\n${marker}` },
    { id: 'event:9', runId: '7', content: `Another answer\n${marker}` },
  ].map((message) => ({
    ...message,
    role: 'assistant' as const,
    status: 'complete' as const,
    finalAnswer: true,
  })),
};

it('shows an unmet task after reopen and verifies only its exact turn and output', async () => {
  let retained: TaskAcceptance = {
    task_id: 'cmd_task',
    turn_sequence: 3,
    verdict: { status: 'needs_attention', reason: 'required_deliverables_missing' },
    unconfirmed_outputs: [output.reference.resource],
  };
  const confirm = vi.fn((path: string, init?: RequestInit) => {
    expect(path).toBe(
      `/api/v1/agents/${agentId}/answers/event:5/deliverables/${output.id}/confirmation`,
    );
    expect(JSON.parse(String(init?.body))).toEqual({ task_id: retained.task_id });
    retained = { ...retained, verdict: { status: 'succeeded' }, unconfirmed_outputs: [] };
    return Response.json(retained);
  });
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input, init) => {
      const path = decodeURIComponent(new URL(String(input)).pathname);
      if (init?.method === 'POST') return confirm(path, init);
      const earlier = path.includes('event:5');
      return Response.json({
        agent_id: agentId,
        answer_id: earlier ? 'event:5' : 'event:9',
        turn_sequence: earlier ? 3 : 7,
        items: [output],
      });
    },
  });
  function OpenTask() {
    const [task, setTask] = useState(retained);
    return (
      <>
        <AgentHeader
          title="Report"
          status="ready"
          taskAcceptance={task}
          breadcrumb={[]}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
        />
        <aside aria-label="Tasks">
          {taskStateMeta(
            taskStatus({ status: 'ready', taskStatus: 'completed', taskAcceptance: task }),
          )}
        </aside>
        <MessageStream
          conversation={conversation}
          taskAcceptance={task}
          onTaskDeliveryConfirmed={() => setTask(retained)}
          onApproval={() => {}}
          deliverables={{ agentId, viewerId: 'owner', client: client.agents }}
        />
      </>
    );
  }
  const first = render(<OpenTask />);
  first.unmount();
  render(<OpenTask />);
  const sidebar = screen.getByRole('complementary', { name: 'Tasks' });
  const plan = screen.getByRole('region', { name: 'Plan' });
  expect(within(sidebar).getByText('Needs attention')).toBeVisible();
  expect(screen.getAllByText('Needs attention')).toHaveLength(2);
  expect(within(plan).getByRole('status')).toHaveTextContent(
    'Required outputs still need verification.',
  );
  const verify = await screen.findByRole('button', { name: 'Verify required output' });
  await screen.findAllByRole('list', { name: 'Answer deliverables' });
  expect(screen.getAllByRole('button', { name: 'Verify required output' })).toHaveLength(1);
  fireEvent.click(verify);
  expect(await within(sidebar).findByText('Task complete')).toBeVisible();
  expect(within(plan).getByRole('status')).toHaveTextContent('Task complete');
  expect(confirm).toHaveBeenCalledOnce();
  expect(screen.queryByRole('button', { name: 'Verify required output' })).toBeNull();
});
