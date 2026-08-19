import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ToolCall } from './ToolCall';
import { MessageStream } from './MessageStream';
import { fold } from './conversation';
import { AguiConnection, MockSession, transcriptNamed, type AguiEvent } from '../agui';

/**
 * Tool calls as structure — `Web_Style.md` §18.2.
 */

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

async function conversationFrom(name: string, options: { dropAfter?: number } = {}) {
  const session = new MockSession(transcriptNamed(name), options);
  const seen: AguiEvent[] = [];
  const connection = new AguiConnection(session.transport, 'sesn_1', {
    backoffMs: 0,
    sleep: () => Promise.resolve(),
  });
  const stop = connection.subscribe((event) => seen.push(event));
  for (let i = 0; i < 30; i += 1) {
    await tick();
  }
  stop();
  return fold(seen);
}

describe('the collapsed row', () => {
  it('identifies the call without being opened', () => {
    // §18.2: "the collapsed state carries enough to identify the call without
    // expanding it." A row reading only "tool call" is scrollback with extra
    // steps.
    render(
      <ToolCall
        call={{
          kind: 'tool',
          id: 't1',
          toolKind: 'file_write',
          target: 'api/crates/layer2-core/src/session.rs',
          status: 'ok',
          seq: 2,
        }}
      />,
    );
    expect(screen.getByText('file_write')).toBeInTheDocument();
    expect(screen.getByText('api/crates/layer2-core/src/session.rs')).toBeInTheDocument();
  });

  it('renders a kind nobody has a mapping for, as it arrived', () => {
    // §3: the vocabularies differ between kernels and grow without warning,
    // and a session must not fail because one invented a category.
    render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'workspace_search_v2', status: 'ok', seq: 2 }}
      />,
    );
    expect(screen.getByText('workspace_search_v2')).toBeInTheDocument();
  });

  it('is collapsed by default', () => {
    const { container } = render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'ok', output: 'x', seq: 2 }}
      />,
    );
    expect(container.querySelector('.toolcall__output')).toBeNull();
    expect(screen.getByRole('button')).toHaveAttribute('aria-expanded', 'false');
  });
});

describe('telling calls apart without reading them', () => {
  it('distinguishes a running call from a finished one', async () => {
    // §18.2, and the transcript the milestone asks for: a turn with two tool
    // calls, one still running when it is read.
    const conversation = await conversationFrom('two-tool-calls');
    const { container } = render(<MessageStream kernel="codex" entries={conversation.entries} />);

    const calls = [...container.querySelectorAll('.toolcall')];
    expect(calls).toHaveLength(2);
    expect(calls[0]?.getAttribute('data-tone')).toBe('done');
    expect(calls[1]?.getAttribute('data-tone')).toBe('running');
    // A word as well as a tone, because colour never carries meaning alone.
    expect(within(calls[1] as HTMLElement).getByText('running')).toBeInTheDocument();
  });

  it('distinguishes a failed call from a successful one', async () => {
    const conversation = await conversationFrom('failed-tool-call');
    const { container } = render(<MessageStream kernel="codex" entries={conversation.entries} />);
    const call = container.querySelector('.toolcall');

    expect(call?.getAttribute('data-tone')).toBe('failed');
    expect(within(call as HTMLElement).getByText('failed')).toBeInTheDocument();
  });

  it('does not call a status it has never seen a failure', () => {
    // The vocabularies grow. A status nobody recognised must not be drawn as
    // a failure on the strength of not being recognised.
    const { container } = render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'partially_applied', seq: 2 }}
      />,
    );
    expect(container.querySelector('.toolcall')?.getAttribute('data-tone')).toBe('done');
  });

  it('reads the same live and replayed', async () => {
    // The equivalence rule again, at the component §18.2 governs: a turn with
    // two calls, one still running, reads correctly both ways.
    //
    // React's generated ids are normalised out. They are React's counter
    // rather than anything the conversation said, and comparing them would
    // make this test fail on the order two renders happened in — which is
    // precisely not the thing §18.1 is about.
    const withoutGeneratedIds = (html: string) =>
      html.replace(/(id|aria-controls)="[^"]*"/g, '$1="_"');

    const live = await conversationFrom('two-tool-calls');
    const replayed = await conversationFrom('two-tool-calls', { dropAfter: 2 });

    const watching = render(<MessageStream kernel="codex" entries={live.entries} />);
    const html = withoutGeneratedIds(watching.container.innerHTML);
    watching.unmount();

    const reading = render(<MessageStream kernel="codex" entries={replayed.entries} />);
    expect(withoutGeneratedIds(reading.container.innerHTML)).toBe(html);
  });
});

describe('disclosure, not truncation', () => {
  it('drops nothing from a call output', async () => {
    // §18.2: enormous output is agent-emitted content and §1's fidelity rule
    // applies, so it is not silently shortened — it is bounded by the
    // disclosure, which is a frame decision and permitted.
    const user = userEvent.setup();
    const enormous = Array.from({ length: 400 }, (_, i) => `line ${String(i)}`).join('\n');
    render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'ok', output: enormous, seq: 2 }}
      />,
    );

    await user.click(screen.getByRole('button'));

    // Read off the body itself rather than the region around it: §18.2 now
    // gives each body its own labelled scrollport, so the region also holds a
    // gutter label. What the rule guarantees — that nothing is dropped — is a
    // property of the body.
    const region = screen.getByRole('button').getAttribute('aria-controls');
    const body = document.getElementById(region ?? '')?.querySelector('.toolcall__output');
    expect(body?.textContent).toBe(enormous);
  });

  it('gives each body its own window', async () => {
    const user = userEvent.setup();
    // One window shared between them meant a long argument payload pushed the
    // result out of view, so the reader scrolled past what they already knew
    // to reach the thing they opened the row for.
    render(
      <ToolCall
        call={{
          kind: 'tool',
          id: 't1',
          toolKind: 'shell',
          status: 'ok',
          arguments: { text: 'a'.repeat(400), truncated: false },
          result: { text: 'the answer', truncated: false },
          seq: 2,
        }}
      />,
    );
    await user.click(screen.getByRole('button'));
    expect(document.querySelectorAll('.toolcall__output')).toHaveLength(2);
  });

  it('draws nothing for a body that did not arrive', async () => {
    const user = userEvent.setup();
    // §18.5: an absence is a fact, not an empty labelled region.
    render(
      <ToolCall
        call={{
          kind: 'tool',
          id: 't1',
          toolKind: 'shell',
          status: 'ok',
          result: { text: 'only this', truncated: false },
          seq: 2,
        }}
      />,
    );
    await user.click(screen.getByRole('button'));
    const labels = [...document.querySelectorAll('.toolcall__bodylabel')].map((n) => n.textContent);
    expect(labels).toEqual(['result']);
  });

  it('says which body was cut, not just that something was', async () => {
    const user = userEvent.setup();
    render(
      <ToolCall
        call={{
          kind: 'tool',
          id: 't1',
          toolKind: 'shell',
          status: 'ok',
          arguments: { text: 'asked', truncated: true },
          result: { text: 'got', truncated: false },
          seq: 2,
        }}
      />,
    );
    await user.click(screen.getByRole('button'));
    const labels = [...document.querySelectorAll('.toolcall__bodylabel')].map((n) => n.textContent);
    expect(labels).toEqual(['arguments (truncated)', 'result']);
  });

  it('bounds it by scrolling rather than by cutting it', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/chat/ToolCall.css'), 'utf8');
    const output = /\.toolcall__output\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(output).toContain('overflow: auto');
    expect(output).not.toContain('text-overflow');
    expect(output).not.toContain('-webkit-line-clamp');
  });

  it('offers no disclosure when there is nothing behind it', () => {
    render(<ToolCall call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'ok', seq: 2 }} />);
    expect(screen.getByRole('button')).toBeDisabled();
  });

  it('carries the time a finished call took', () => {
    render(
      <ToolCall
        call={{
          kind: 'tool',
          id: 't1',
          toolKind: 'read',
          status: 'ok',
          seq: 2,
          startedAt: '2026-08-19T04:00:00Z',
          finishedAt: '2026-08-19T04:00:02Z',
        }}
      />,
    );
    // Two seconds apart, from the durable log — never estimated.
    expect(screen.getByText('2s')).toBeInTheDocument();
  });

  it('shows no duration when either instant is absent', () => {
    // A call whose start or finish the record does not carry shows no duration
    // rather than one invented (§18.2).
    const { container } = render(
      <ToolCall
        call={{
          kind: 'tool',
          id: 't1',
          toolKind: 'read',
          status: 'ok',
          seq: 2,
          startedAt: '2026-08-19T04:00:00Z',
        }}
      />,
    );
    expect(container.querySelector('.toolcall__duration')).toBeNull();
  });
});
