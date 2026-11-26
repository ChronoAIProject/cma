import { fireEvent, render, screen, within } from '@testing-library/react';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { createClient, type AnswerDeliverable, type TaskAcceptance } from '@cma/client';
import { AgentHeader } from './AgentHeader';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript } from './conversation';
import { observedTaskStatus, taskStateMeta, taskStatus } from '../shell/taskStatus';
import { AGENT_DETAIL } from '../test/agent';

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
  latestTurnId: '3',
  plans: [
    { id: 'plan:4', runId: '3', entries: [{ title: 'Write the report', status: 'completed' }] },
  ],
  turnOutcomes: { 1: 'completed' as const, 3: 'completed' as const },
  messages: [
    { id: 'event:2', runId: '1', content: `Another answer\n${marker}` },
    { id: 'event:5', runId: '3', content: `Required answer\n${marker}` },
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
      const required = path.includes('event:5');
      return Response.json({
        agent_id: agentId,
        answer_id: required ? 'event:5' : 'event:2',
        turn_sequence: required ? 3 : 1,
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
          turnId={conversation.latestTurnId}
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

it.each([
  { outcome: 'failed', activity: 'failed', label: 'Failed' },
  { outcome: 'interrupted', activity: 'interrupted', label: 'Response interrupted' },
  { outcome: 'completed', activity: 'completed', label: 'Turn ended' },
] as const)(
  'keeps an earlier accepted verdict on its turn after a later $outcome turn and reopen',
  ({ outcome, activity, label }) => {
    const task: TaskAcceptance = {
      task_id: 'cmd_report',
      turn_sequence: 3,
      verdict: { status: 'succeeded' },
      unconfirmed_outputs: [],
    };
    const detail = { ...AGENT_DETAIL, id: agentId, taskAcceptance: task };
    const reopened = {
      ...conversation,
      latestTurnId: '7',
      turnOutcomes: { ...conversation.turnOutcomes, 7: outcome },
    };
    function OpenAgent({ later }: { later: boolean }) {
      const state = later ? reopened : conversation;
      const taskStatus = later ? activity : 'completed';
      const turnId = state.latestTurnId;
      return (
        <>
          <AgentHeader
            title="Report"
            status="ready"
            activityStatus={taskStatus}
            turnId={turnId}
            taskAcceptance={task}
            breadcrumb={[]}
            detailsOpen={false}
            onDetailsOpenChange={() => {}}
          />
          <aside aria-label="Tasks">
            {taskStateMeta(
              observedTaskStatus(
                {
                  ...detail,
                  taskStatus: taskStatus === 'interrupted' ? 'stopped' : taskStatus,
                  sandboxId: detail.sandbox.id,
                  workspaceId: detail.workspace.id,
                  worktreeId: detail.worktree.id,
                },
                undefined,
                { agent: detail, status: taskStatus, turnId },
              ),
            )}
          </aside>
          <MessageStream conversation={state} taskAcceptance={task} onApproval={() => {}} />
        </>
      );
    }
    const first = render(<OpenAgent later={false} />);
    expect(within(screen.getByRole('banner')).getByText('Task complete')).toBeVisible();
    first.unmount();
    render(<OpenAgent later />);
    const sidebar = screen.getByRole('complementary', { name: 'Tasks' });
    expect(within(sidebar).getByText(label)).toBeVisible();
    expect(within(screen.getByRole('banner')).queryByText('Task complete')).toBeNull();
    expect(
      within(screen.getByRole('region', { name: 'Plan' })).getByRole('status'),
    ).toHaveTextContent('Task complete');
  },
);

it('shows the retained verdict on the exact final answer when there is no plan', () => {
  render(
    <MessageStream
      conversation={{ ...conversation, latestTurnId: '7', plans: [] }}
      taskAcceptance={{
        task_id: 'cmd_report',
        turn_sequence: 3,
        verdict: { status: 'succeeded' },
        unconfirmed_outputs: [],
      }}
      onApproval={() => {}}
    />,
  );
  expect(screen.getByText('Task complete').closest('article')).toHaveTextContent('Required answer');
});

it('does not attach an earlier verdict to the selected row while its task list refreshes', () => {
  const taskAcceptance: TaskAcceptance = {
    task_id: 'cmd_earlier',
    turn_sequence: 3,
    verdict: { status: 'succeeded' },
    unconfirmed_outputs: [],
  };
  const detail = { ...AGENT_DETAIL, taskAcceptance };
  const row = {
    ...detail,
    taskStatus: 'completed' as const,
    sandboxId: detail.sandbox.id,
    workspaceId: detail.workspace.id,
    worktreeId: detail.worktree.id,
  };
  expect(observedTaskStatus(row, undefined, { agent: detail, turnId: '3' })).toBe('task_accepted');
  expect(observedTaskStatus(row, undefined, { agent: detail, turnId: '7' })).toBe('completed');
});
