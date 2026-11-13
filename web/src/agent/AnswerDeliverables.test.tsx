import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, type AnswerDeliverable } from '@chronoai/cma-sdk';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript } from './conversation';
import { answerProse, streamingSafePrefix } from './answerDeliveries';

const first: AnswerDeliverable = {
  id: `delivery_${'a'.repeat(64)}`,
  reference: {
    agent_id: 'agent-1',
    resource: { kind: 'live_file', path: 'first.txt' },
    name: 'first.txt',
    media_type: 'application/octet-stream',
    size_bytes: 3,
    sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
  },
  status: 'unchecked',
};
const second: AnswerDeliverable = {
  ...first,
  id: `delivery_${'b'.repeat(64)}`,
  reference: {
    ...first.reference,
    resource: { kind: 'live_file', path: 'second.txt' },
    name: 'second.txt',
  },
};
const marker = (item: AnswerDeliverable) =>
  `<!--cma-deliverable:v1:${btoa(JSON.stringify(item.reference)).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')}-->`;
const json = (value: unknown) =>
  new Response(JSON.stringify(value), { headers: { 'content-type': 'application/json' } });
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it.each([
  ['CJK text', '你好，世界。', '你好，世界。'],
  ['an unclosed link', 'Read [the report](https://example.test/report', 'Read '],
  ['an unfinished link label', 'Read [the report', 'Read '],
  [
    'a complete link',
    'Read [the report](https://example.test/report)',
    'Read [the report](https://example.test/report)',
  ],
  ['a link label awaiting its URL', 'Read [the report]', 'Read '],
  ['an unpaired emphasis delimiter', 'Before **the answer', 'Before '],
  ['an unpaired inline code delimiter', 'Before `the answer', 'Before '],
  ['emphasis-looking inline code', 'Use `**kwargs` now', 'Use `**kwargs` now'],
  ['mathematical powers', 'Compute 2 ** 3 now', 'Compute 2 ** 3 now'],
  [
    'independent emphasis delimiters',
    'A **bold** and __underlined__ answer',
    'A **bold** and __underlined__ answer',
  ],
  ['CJK emphasis', '你好 **世界**', '你好 **世界**'],
  ['an open fenced code block', '```\n** [literal](url) `\n', '```\n** [literal](url) `\n'],
  ['a partial deliverable marker', 'Answer\n<!--cma-deliverable:v1:eyJ', 'Answer\n'],
])('renders a safe streaming prefix for %s', (_name, input, expected) => {
  expect(streamingSafePrefix(input)).toBe(expected);
  expect(answerProse(input, true).content).toBe(expected);
});

it('hides complete and partial deliverable markers while streaming', () => {
  const complete = '<!--cma-deliverable:v1:eyJ0ZXN0Ijp0cnVlfQ-->';
  expect(answerProse(`Answer\n${complete}`, true).content).toBe('Answer\n');
  expect(answerProse(`Answer\n${complete}`, false).content).toBe('Answer');
  expect(answerProse('Answer\n<!--cma-deliverable:v1:eyJ0ZXN0', true).content).toBe('Answer\n');
});

it('keeps protocol metadata out of streaming prose without creating premature cards', () => {
  const state = transcript();
  const message = {
    ...state.messages[0]!,
    finalAnswer: false,
    status: 'streaming' as const,
    content: 'Writing the report.\n<!--cma-deliverable:v1:eyJhZ2VudF9pZCI',
  };
  const view = render(
    <MessageStream conversation={{ ...state, messages: [message] }} onApproval={vi.fn()} />,
  );
  expect(screen.getByText('Writing the report.')).toBeVisible();
  expect(view.container.textContent).not.toContain('cma-deliverable');
  expect(screen.queryByRole('list', { name: 'Answer deliverables' })).toBeNull();
});

it('shows access revocation discovered after buffering and never saves those bytes', async () => {
  const saved = vi.fn(() => 'blob:forbidden');
  vi.stubGlobal(
    'URL',
    class extends URL {
      static createObjectURL = saved;
      static revokeObjectURL() {}
    },
  );
  let metadataReads = 0;
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      const identity = { agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3 };
      if (path.endsWith('/deliverables')) return json({ ...identity, items: [first] });
      if (path.endsWith('/download'))
        return new Response('abc', { headers: { 'content-type': 'application/octet-stream' } });
      return json({
        ...identity,
        item: { ...first, status: ++metadataReads === 1 ? 'unchecked' : 'denied' },
      });
    },
  });
  const state = transcript();
  render(
    <MessageStream
      conversation={{ ...state, messages: state.messages.slice(0, 1) }}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Download first.txt' }));
  expect(await screen.findByText('Access unavailable')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Download first.txt' })).toBeDisabled();
  expect(saved).not.toHaveBeenCalled();
});

it('explains a changed live file reported by the download route and keeps retry independent', async () => {
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = new URL(String(input)).pathname;
      const identity = { agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3 };
      if (path.endsWith('/download'))
        return new Response(
          JSON.stringify({
            type: 'https://cma.test/problems/deliverable-changed',
            title: 'File changed',
            status: 409,
            detail: 'Bytes differ from the referenced version.',
            code: 'deliverable_changed',
          }),
          { status: 409, headers: { 'content-type': 'application/problem+json' } },
        );
      return json(
        path.endsWith('/deliverables')
          ? { ...identity, items: [first] }
          : { ...identity, item: first },
      );
    },
  });
  const state = transcript();
  render(
    <MessageStream
      conversation={{ ...state, messages: state.messages.slice(0, 1) }}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Download first.txt' }));
  expect(await screen.findByText('File changed since this answer')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Download first.txt' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Retry first.txt' })).toBeEnabled();
});

function transcript() {
  return {
    ...conversationFromTranscript({ agentId: 'agent-1', throughSeq: 0, turns: [] }),
    messages: [
      {
        id: 'event:5',
        role: 'assistant' as const,
        status: 'complete' as const,
        finalAnswer: true,
        content: `First answer\n${marker(first)}`,
      },
      {
        id: 'event:9',
        role: 'assistant' as const,
        status: 'complete' as const,
        finalAnswer: true,
        content: `Second answer\n${marker(second)}`,
      },
      {
        id: 'event:13',
        role: 'assistant' as const,
        status: 'complete' as const,
        finalAnswer: true,
        content: 'An ordinary `private.txt` filename.',
      },
    ],
  };
}

it('keeps verified deliveries under their own final answers after a fresh mount, without inferring filenames', async () => {
  const requests: string[] = [];
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = decodeURIComponent(new URL(String(input)).pathname);
      requests.push(path);
      const answer = path.includes('event:5') ? 'event:5' : 'event:9';
      return json({
        agent_id: 'agent-1',
        answer_id: answer,
        turn_sequence: answer === 'event:5' ? 3 : 7,
        items: [answer === 'event:5' ? first : second],
      });
    },
  });
  const draw = () => (
    <MessageStream
      conversation={transcript()}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />
  );
  const view = render(draw());
  expect(await screen.findByRole('button', { name: 'Download first.txt' })).toBeEnabled();
  expect(
    within(screen.getByText('First answer').closest('article')!).queryByText('second.txt'),
  ).toBeNull();
  expect(await screen.findByRole('button', { name: 'Download second.txt' })).toBeEnabled();
  expect(screen.getAllByRole('list', { name: 'Answer deliverables' })).toHaveLength(2);
  expect(requests).toHaveLength(2);
  expect(requests.every((path) => !path.includes('event:13'))).toBe(true);
  view.unmount();
  render(draw());
  expect(await screen.findByRole('button', { name: 'Download first.txt' })).toBeEnabled();
  expect(
    within(screen.getByText('First answer').closest('article')!).getByText('Verify on download'),
  ).toBeVisible();
});

it('saves only verified original bytes after current authorization is checked again', async () => {
  const saved: Blob[] = [];
  vi.stubGlobal(
    'URL',
    class extends URL {
      static createObjectURL(blob: Blob) {
        saved.push(blob);
        return 'blob:verified';
      }
      static revokeObjectURL() {}
    },
  );
  const clicked = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  const requests: string[] = [];
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const path = decodeURIComponent(new URL(String(input)).pathname);
      requests.push(path);
      const identity = { agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3 };
      if (path.endsWith('/download'))
        return new Response('abc', { headers: { 'content-type': 'application/octet-stream' } });
      return json(
        path.endsWith('/deliverables')
          ? { ...identity, items: [first] }
          : { ...identity, item: first },
      );
    },
  });
  const state = transcript();
  render(
    <MessageStream
      conversation={{ ...state, messages: state.messages.slice(0, 1) }}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Download first.txt' }));
  await waitFor(() => expect(clicked).toHaveBeenCalledOnce());
  expect(requests.filter((path) => path.endsWith(first.id))).toHaveLength(2);
  expect(saved[0]!.type).toBe('application/octet-stream');
  const reader = new FileReader();
  const content = new Promise((resolve) => {
    reader.onload = () => resolve(reader.result);
  });
  reader.readAsText(saved[0]!);
  expect(await content).toBe('abc');
  clicked.mockRestore();
  vi.unstubAllGlobals();
});

it('retries one resource independently and explains changed, missing, denied, unavailable and original document versions', async () => {
  const document: AnswerDeliverable = {
    ...second,
    reference: {
      ...second.reference,
      resource: { kind: 'document', id: `doc_${'c'.repeat(32)}`, revision: 1 },
      media_type: 'text/markdown',
    },
    status: 'version_changed',
  };
  let status: AnswerDeliverable['status'] = 'missing';
  const requests: string[] = [];
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input) => {
      const url = new URL(String(input));
      requests.push(url.pathname + url.search);
      const identity = { agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3 };
      return json(
        url.pathname.endsWith('/deliverables')
          ? { ...identity, items: [{ ...first, status }, document] }
          : { ...identity, item: { ...first, status } },
      );
    },
  });
  const state = transcript();
  render(
    <MessageStream
      conversation={{ ...state, messages: state.messages.slice(0, 1) }}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />,
  );
  expect(await screen.findByText('File deleted')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Download first.txt' })).toBeDisabled();
  expect(
    screen.getByText('Document has a newer revision; download keeps this revision'),
  ).toBeVisible();
  for (const [next, label] of [
    ['changed', 'File changed since this answer'],
    ['not_ready', 'Not ready'],
    ['denied', 'Access unavailable'],
    ['too_large', 'Exceeds the resource limit'],
    ['unavailable', 'Download unavailable; retry'],
    ['ready', 'Ready to download'],
  ] as const) {
    status = next;
    fireEvent.click(screen.getByRole('button', { name: 'Retry first.txt' }));
    expect(screen.getByRole('button', { name: 'Download second.txt' })).toBeEnabled();
    expect(await screen.findByText(label)).toBeVisible();
  }
  expect(requests.slice(1).every((path) => path.endsWith(`${first.id}?verify=true`))).toBe(true);
  expect(screen.getByRole('button', { name: 'Download first.txt' })).toBeEnabled();
});

it('queries only currently visible referenced answers, including after a viewer changes', async () => {
  const observers: { active: boolean; notify: (visible: boolean) => void }[] = [];
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      state: (typeof observers)[number];
      constructor(callback: IntersectionObserverCallback) {
        this.state = {
          active: true,
          notify: (visible) => {
            if (this.state.active)
              callback(
                [{ isIntersecting: visible } as IntersectionObserverEntry],
                this as unknown as IntersectionObserver,
              );
          },
        };
        observers.push(this.state);
      }
      observe() {}
      disconnect() {
        this.state.active = false;
      }
    },
  );
  let requests = 0;
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async () => {
      requests++;
      return json({ agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3, items: [first] });
    },
  });
  const state = transcript();
  const draw = (viewerId: string) => (
    <MessageStream
      conversation={{ ...state, messages: state.messages.slice(0, 1) }}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId, client: client.agents }}
    />
  );
  const view = render(draw('owner'));
  expect(requests).toBe(0);
  await act(async () => observers[0]!.notify(true));
  expect(await screen.findByRole('button', { name: 'Download first.txt' })).toBeEnabled();
  await act(async () => observers[0]!.notify(false));
  view.rerender(draw('another-viewer'));
  await act(async () => {});
  expect(requests).toBe(1);
  await act(async () => observers.at(-1)!.notify(true));
  expect(await screen.findByRole('button', { name: 'Download first.txt' })).toBeEnabled();
  expect(requests).toBe(2);
});

it('aborts and discards a late download when authorization disappears', async () => {
  const saved = vi.fn(() => 'blob:forbidden');
  vi.stubGlobal(
    'URL',
    class extends URL {
      static createObjectURL = saved;
      static revokeObjectURL() {}
    },
  );
  let deliver: ((response: Response) => void) | undefined;
  let signal: AbortSignal | null | undefined;
  const client = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input, options) => {
      const path = new URL(String(input)).pathname;
      const identity = { agent_id: 'agent-1', answer_id: 'event:5', turn_sequence: 3 };
      if (path.endsWith('/download')) {
        signal = options?.signal;
        return new Promise<Response>((resolve) => {
          deliver = resolve;
        });
      }
      return json(
        path.endsWith('/deliverables')
          ? { ...identity, items: [first] }
          : { ...identity, item: first },
      );
    },
  });
  const state = transcript();
  const conversation = { ...state, messages: state.messages.slice(0, 1) };
  const view = render(
    <MessageStream
      conversation={conversation}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />,
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Download first.txt' }));
  await waitFor(() => expect(deliver).toBeDefined());
  view.rerender(<MessageStream conversation={conversation} onApproval={vi.fn()} />);
  expect(signal?.aborted).toBe(true);
  await act(async () =>
    deliver!(new Response('abc', { headers: { 'content-type': 'application/octet-stream' } })),
  );
  expect(saved).not.toHaveBeenCalled();
  expect(screen.queryByRole('list', { name: 'Answer deliverables' })).toBeNull();
});

it('leaves fenced examples intact and never fetches deliveries for unfinished or intermediate messages', async () => {
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://cma.test', fetch });
  const state = transcript();
  const message = state.messages[0]!;
  const messages = [
    { ...message, finalAnswer: false },
    { ...message, id: 'event:9', liveOnly: true },
    { ...message, id: 'event:13', status: 'streaming' as const },
    {
      ...message,
      id: 'event:17',
      content: `A quoted example:\n\`\`\`html\n${marker(first)}\n\`\`\``,
    },
  ];
  render(
    <MessageStream
      conversation={{ ...state, messages }}
      onApproval={vi.fn()}
      deliverables={{ agentId: 'agent-1', viewerId: 'owner', client: client.agents }}
    />,
  );
  await act(async () => {});
  expect(fetch).not.toHaveBeenCalled();
  expect(screen.queryByRole('list', { name: 'Answer deliverables' })).toBeNull();
  expect(
    within(screen.getByText('A quoted example:').closest('article')!).getByText(marker(first)),
  ).toBeVisible();
});
