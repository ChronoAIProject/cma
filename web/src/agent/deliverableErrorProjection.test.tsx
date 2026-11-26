import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { ToolCallRow } from './ToolCallRows';

const agentId = 'fixture-agent';
const title = 'mcp.cma_access.describe_deliverable';
const at = '2026-11-05T14:33:00.000Z';
const empty = () => conversationFromTranscript({ agentId, throughSeq: 0, turns: [] });

it.each([
  'This deliverable could not be verified (too_large). Retryable: false. Produce a smaller file.',
  'This deliverable could not be verified (runtime_not_ready). Retryable: true. Wait until the runtime is ready.',
  'MCP call failed before a typed deliverable reply (mcp_transport_error). Retryable: unknown. Check the runtime and MCP connection before retrying the same resource.',
])('preserves the failure in live and replayed operation details: %s', (text) => {
  const output = { text, truncated: false };
  const base = { threadId: agentId, runId: '1', toolCallId: 'delivery' };
  const streamed = foldLiveEvent(
    foldLiveEvent(
      empty(),
      { ...base, type: 'TOOL_CALL_START', toolCallName: title, 'cma:toolKind': 'execute' },
      agentId,
    ),
    { ...base, type: 'TOOL_CALL_END', 'cma:status': 'failed', 'cma:output': output },
    agentId,
  );
  const restored = [
    {
      seq: 1,
      appendedAt: at,
      kind: {
        type: 'tool_call_started',
        call_id: 'delivery',
        title,
        kind: 'execute',
        status: 'in_progress',
      },
    },
    {
      seq: 2,
      appendedAt: at,
      kind: { type: 'tool_call_updated', call_id: 'delivery', status: 'failed', output },
    },
  ].reduce(foldAgentEvent, empty());
  for (const state of [streamed, restored]) {
    const tool = state.toolCalls[0]!;
    expect(tool).toMatchObject({ status: 'failed', output });
    const view = render(<ToolCallRow tool={tool} />);
    fireEvent.click(screen.getByRole('button', { name: /Ran 1 command/ }));
    const row = screen.getByRole('article', { name: `Tool call: ${title}` });
    expect(row).toHaveTextContent(text);
    fireEvent.click(
      within(row).getByRole('button', { name: /mcp\.cma_access\.describe_deliverable/ }),
    );
    expect(within(row).getByLabelText('Recorded output')).toHaveTextContent(text);
    view.unmount();
  }
});

it('preserves a historical failure sent as a terminal start and output event', () => {
  const base = { threadId: agentId, runId: '1', toolCallId: 'historical' };
  const output = {
    text: 'Deliverable result unavailable (result_projection_unavailable). Retryable: unknown.',
    truncated: false,
  };
  const state = foldLiveEvent(
    foldLiveEvent(
      empty(),
      { ...base, type: 'TOOL_CALL_START', toolCallName: title, 'cma:status': 'failed' },
      agentId,
    ),
    {
      ...base,
      type: 'CUSTOM',
      name: 'cma.tool_output_reported',
      value: { toolCallId: 'historical', output },
    },
    agentId,
  );
  expect(state.toolCalls[0]).toMatchObject({ status: 'failed', output });
});
