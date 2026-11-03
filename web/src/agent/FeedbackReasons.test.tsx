import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { messageFeedbackClient } from '../api/rest/messageFeedback';
import { agentClient } from '../test/agent';
import { MessageFooter } from './MessageFooter';
import { useMessageFeedback } from './useMessageFeedback';
import type { FeedbackDetails, FeedbackReasonRequest } from '../api/feedbackTypes';

function Surface({ viewer = 'reason-owner', agent = 'reason-agent', enabled = true }) {
  const feedback = useMessageFeedback(agent, viewer, enabled);
  return (
    <>
      <p>The recorded final answer.</p>
      <MessageFooter
        id="event:5"
        content="The recorded final answer."
        committed
        canVote
        feedback={feedback}
      />
      <input aria-label="Message" defaultValue="Unsent question" />
    </>
  );
}
const client = agentClient(messageFeedbackClient());
function draw(props = {}) {
  return (
    <CmaClientProvider client={client}>
      <Surface {...props} />
    </CmaClientProvider>
  );
}
afterEach(() => vi.unstubAllGlobals());

it('opens immediately but waits for confirmed down, and stays closed when the vote finishes late', async () => {
  let finish!: () => void;
  let vote: 'down' | null = null;
  vi.stubGlobal('fetch', async (path: string, options?: RequestInit) => {
    if (path.includes('/message-feedback?')) return Response.json({ items: [], next: null });
    if (options?.method === 'PUT') {
      await new Promise<void>((resolve) => {
        finish = resolve;
      });
      vote = 'down';
      return Response.json({ message_id: 'event:5', vote });
    }
    return Response.json({
      viewer_id: 'pending-owner',
      agent_id: 'reason-agent',
      message_id: 'event:5',
      vote,
      version: 1,
      reasons: [],
      comment: '',
      updated_at: null,
      request_id: null,
    });
  });
  render(draw({ viewer: 'pending-owner' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Needs improvement' })).toBeEnabled(),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Needs improvement' }));
  expect(screen.getByRole('button', { name: 'Save reasons' })).toBeDisabled();
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'Draft before confirmation',
  );
  await userEvent.click(screen.getByRole('button', { name: 'Close' }));
  await act(async () => finish());
  await screen.findByRole('button', { name: 'Edit reasons' });
  expect(screen.queryByRole('region', { name: 'Reasons for improvement' })).toBeNull();
  await openReasons();
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'Draft before confirmation',
  );
});

function boundary(viewer: string) {
  const states = new Map<string, FeedbackDetails>();
  const writes: FeedbackReasonRequest[] = [];
  let mode: 'ok' | 'fail' | 'lost' | 'delay' | 'denied' = 'ok';
  let finish: (() => void) | undefined;
  function state(agent = 'reason-agent'): FeedbackDetails {
    return (
      states.get(agent) ?? {
        viewer_id: viewer,
        agent_id: agent,
        message_id: 'event:5',
        vote: 'down',
        version: 1,
        reasons: [],
        comment: '',
        updated_at: null,
        request_id: null,
      }
    );
  }
  vi.stubGlobal('fetch', async (path: string, options?: RequestInit) => {
    const agent = decodeURIComponent(path.split('/agents/')[1]!.split('/')[0]!);
    if (path.includes('/message-feedback?'))
      return Response.json({
        items: state(agent).vote ? [{ message_id: 'event:5', vote: state(agent).vote }] : [],
        next: null,
      });
    if (options?.method !== 'PUT') return Response.json(state(agent));
    const body = JSON.parse(String(options.body));
    if (!path.endsWith('/reasons')) {
      states.set(agent, {
        ...state(agent),
        vote: body.vote,
        version: state(agent).version + 1,
        reasons: [],
        comment: '',
        request_id: null,
      });
      return Response.json({ message_id: 'event:5', vote: body.vote });
    }
    writes.push(body);
    if (mode === 'denied') return Response.json({}, { status: 403 });
    if (mode === 'fail') throw new TypeError('Network unavailable');
    if (mode === 'delay')
      await new Promise<void>((resolve) => {
        finish = resolve;
      });
    const current = state(agent);
    if (current.request_id !== body.request_id) {
      if (current.vote !== 'down' || current.version !== body.expected_version)
        return Response.json({}, { status: 409 });
      states.set(agent, {
        ...current,
        reasons: body.reasons,
        comment: body.comment,
        request_id: body.request_id,
        version: current.version + 1,
        updated_at: '2026-11-03T01:00:00Z',
      });
    }
    if (mode === 'lost') throw new TypeError('Receipt lost');
    return Response.json(state(agent));
  });
  return {
    writes,
    state,
    advance: () =>
      states.set('reason-agent', {
        ...state(),
        version: state().version + 1,
        comment: 'Another saved edit',
        request_id: 'another-request',
      }),
    mode: (value: typeof mode) => {
      mode = value;
    },
    finish: () => finish!(),
  };
}

async function openReasons() {
  await userEvent.click(await screen.findByRole('button', { name: 'Edit reasons' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save reasons' })).toBeEnabled());
}

it('keeps a conflicting draft and requires reread before a new conditional save', async () => {
  const server = boundary('conflict-owner');
  render(draw({ viewer: 'conflict-owner' }));
  await openReasons();
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'My draft',
  );
  server.advance();
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Feedback changed. Reload feedback before saving this draft.');
  expect(screen.getByRole('button', { name: 'Retry reasons' })).toBeDisabled();
  expect(server.state().comment).toBe('Another saved edit');
  await userEvent.click(screen.getByRole('button', { name: 'Reload feedback' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save reasons' })).toBeEnabled());
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'My draft',
  );
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Reasons saved');
  expect(server.writes.map((request) => request.expected_version)).toEqual([1, 2]);
  expect(server.writes[1]!.request_id).not.toBe(server.writes[0]!.request_id);
  expect(server.state().comment).toBe('My draft');
});

it('keeps previously saved reasons until an edited replacement is confirmed', async () => {
  const server = boundary('edit-owner');
  render(draw({ viewer: 'edit-owner' }));
  await openReasons();
  await userEvent.click(screen.getByRole('checkbox', { name: 'Inaccurate content' }));
  const details = screen.getByRole('textbox', { name: 'Additional details (optional)' });
  await userEvent.type(details, 'First saved');
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Reasons saved');
  await userEvent.click(screen.getByRole('checkbox', { name: 'Inaccurate content' }));
  await userEvent.click(screen.getByRole('checkbox', { name: 'Unclear wording' }));
  fireEvent.change(details, { target: { value: 'Edited draft' } });
  server.mode('fail');
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Reasons could not be confirmed. Retry the same reasons.');
  expect(server.state().comment).toBe('First saved');
  expect(server.state().reasons).toEqual(['inaccurate']);
  await userEvent.click(screen.getByRole('button', { name: 'Close' }));
  await userEvent.click(screen.getByRole('button', { name: 'Edit reasons' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Retry reasons' })).toBeEnabled());
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'Edited draft',
  );
  server.mode('ok');
  await userEvent.click(screen.getByRole('button', { name: 'Retry reasons' }));
  await screen.findByText('Reasons saved');
  expect(server.state().version).toBe(3);
  expect(server.state().comment).toBe('Edited draft');
  expect(server.state().reasons).toEqual(['unclear']);
});

it('retries only the exact failed reasons while preserving the down vote and draft', async () => {
  const server = boundary('retry-owner');
  render(draw({ viewer: 'retry-owner' }));
  await openReasons();
  await userEvent.click(screen.getByRole('checkbox', { name: 'Other' }));
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'Keep this draft',
  );
  server.mode('fail');
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Reasons could not be confirmed. Retry the same reasons.');
  expect(screen.getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'Keep this draft',
  );
  server.mode('ok');
  await userEvent.click(screen.getByRole('button', { name: 'Retry reasons' }));
  await screen.findByText('Reasons saved');
  expect(server.writes).toHaveLength(2);
  expect(server.writes[1]).toEqual(server.writes[0]);
  expect(server.state().version).toBe(2);
});

it('recovers an accepted write with a lost receipt through public reread after navigation', async () => {
  const server = boundary('lost-owner');
  const view = render(draw({ viewer: 'lost-owner' }));
  await openReasons();
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'Confirmed later',
  );
  server.mode('lost');
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Reasons could not be confirmed. Retry the same reasons.');
  view.unmount();
  render(draw({ viewer: 'lost-owner' }));
  await openReasons();
  await screen.findByText('Reasons saved');
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'Confirmed later',
  );
  expect(server.writes).toHaveLength(1);
});

it.each(['Helpful', 'Needs improvement'])(
  'never restores delayed reasons after selecting %s',
  async (label) => {
    const viewer = `late-${label}`;
    const server = boundary(viewer);
    const view = render(draw({ viewer }));
    await openReasons();
    await userEvent.click(screen.getByRole('checkbox', { name: 'Inaccurate content' }));
    server.mode('delay');
    await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
    await screen.findByText('Saving reasons…');
    await userEvent.click(screen.getByRole('button', { name: label }));
    await waitFor(() =>
      expect(screen.queryByRole('region', { name: 'Reasons for improvement' })).toBeNull(),
    );
    await act(async () => server.finish());
    expect(server.state().vote).toBe(label === 'Helpful' ? 'up' : null);
    expect(server.state().reasons).toEqual([]);
    view.unmount();
    render(draw({ viewer }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Needs improvement' })).toHaveAttribute(
        'aria-pressed',
        'false',
      ),
    );
    await userEvent.click(screen.getByRole('button', { name: 'Needs improvement' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save reasons' })).toBeEnabled());
    expect(screen.getByRole('checkbox', { name: 'Inaccurate content' })).not.toBeChecked();
  },
);

it('retains drafts by Agent, but clears them when the viewer or authority changes', async () => {
  boundary('scope-owner');
  const view = render(draw({ viewer: 'scope-owner' }));
  await openReasons();
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'Private draft',
  );
  view.rerender(draw({ viewer: 'scope-owner', agent: 'other-agent' }));
  await openReasons();
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue('');
  view.rerender(draw({ viewer: 'scope-owner' }));
  await openReasons();
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'Private draft',
  );
  view.rerender(draw({ viewer: 'scope-owner', enabled: false }));
  expect(screen.queryByRole('button', { name: 'Save reasons' })).toBeNull();
  view.rerender(draw({ viewer: 'scope-owner' }));
  await openReasons();
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue('');
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'Account draft',
  );
  view.rerender(draw({ viewer: 'other-owner' }));
  expect(screen.queryByDisplayValue('Account draft')).toBeNull();
  view.rerender(draw({ viewer: 'scope-owner' }));
  await openReasons();
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue('');
});

it('counts Unicode code points without truncating and clears drafts on a permission refusal', async () => {
  const server = boundary('unicode-owner');
  const view = render(draw({ viewer: 'unicode-owner' }));
  await openReasons();
  const details = screen.getByRole('textbox', { name: 'Additional details (optional)' });
  fireEvent.change(details, { target: { value: '🦀'.repeat(500) } });
  expect(screen.getByText('500 / 500')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Save reasons' })).toBeEnabled();
  fireEvent.change(details, { target: { value: '🦀'.repeat(501) } });
  expect(details).toHaveValue('🦀'.repeat(501));
  expect(details).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByRole('button', { name: 'Save reasons' })).toBeDisabled();
  fireEvent.change(details, { target: { value: 'Private text' } });
  server.mode('denied');
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Feedback is unavailable for this account.');
  expect(screen.queryByDisplayValue('Private text')).toBeNull();
  view.unmount();
  render(draw({ viewer: 'unicode-owner' }));
  await openReasons();
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue('');
});

it('saves down immediately, retains a closed draft, then independently confirms and rereads reasons', async () => {
  const writes: unknown[] = [];
  let state = {
    viewer_id: 'reason-owner',
    agent_id: 'reason-agent',
    message_id: 'event:5',
    vote: null as 'down' | null,
    version: 0,
    reasons: [] as string[],
    comment: '',
    updated_at: null as string | null,
    request_id: null as string | null,
  };
  vi.stubGlobal('fetch', async (path: string, options?: RequestInit) => {
    if (path.includes('/message-feedback?'))
      return Response.json({
        items: state.vote ? [{ message_id: 'event:5', vote: state.vote }] : [],
        next: null,
      });
    if (options?.method !== 'PUT') return Response.json(state);
    const body = JSON.parse(String(options.body));
    writes.push(body);
    if (path.endsWith('/reasons')) {
      expect(body.expected_version).toBe(1);
      state = { ...state, ...body, version: 2, updated_at: '2026-11-03T01:00:00Z' };
    } else state = { ...state, vote: body.vote, version: 1 };
    return Response.json(
      path.endsWith('/reasons') ? state : { message_id: 'event:5', vote: state.vote },
    );
  });
  const view = render(draw());
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Needs improvement' })).toBeEnabled(),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Needs improvement' }));
  expect(writes).toEqual([{ vote: 'down' }]);
  await userEvent.click(await screen.findByRole('checkbox', { name: 'Inaccurate content' }));
  await userEvent.type(
    screen.getByRole('textbox', { name: 'Additional details (optional)' }),
    'Please check the source.',
  );
  await userEvent.click(screen.getByRole('button', { name: 'Close' }));
  await userEvent.click(screen.getByRole('button', { name: 'Edit reasons' }));
  expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
    'Please check the source.',
  );
  await userEvent.click(screen.getByRole('button', { name: 'Save reasons' }));
  await screen.findByText('Reasons saved');
  expect(writes).toHaveLength(2);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Unsent question');
  view.unmount();
  render(draw());
  await userEvent.click(await screen.findByRole('button', { name: 'Edit reasons' }));
  await waitFor(() =>
    expect(screen.getByRole('textbox', { name: 'Additional details (optional)' })).toHaveValue(
      'Please check the source.',
    ),
  );
  expect(screen.getByRole('checkbox', { name: 'Inaccurate content' })).toBeChecked();
});
