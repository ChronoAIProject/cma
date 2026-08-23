import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ToolCall, sectionsOf } from './ToolCall';
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

  it('shows no detail of its own, whatever the call carries', () => {
    // The row is a summary. What the call carries opens in the details
    // column, where there is room for it.
    const { container } = render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'ok', output: 'x', seq: 2 }}
        onInspect={vi.fn()}
      />,
    );
    expect(container.querySelector('pre')).toBeNull();
    expect(screen.getByRole('button')).toHaveAttribute('aria-pressed', 'false');
  });

  it('is not pressable where there is no column to open', () => {
    // Opening onto nothing is worse than saying there is nothing to open.
    render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'ok', output: 'x', seq: 2 }}
      />,
    );
    expect(screen.getByRole('button')).toBeDisabled();
  });

  it('is not pressable where the call carries nothing to show', () => {
    render(
      <ToolCall
        call={{ kind: 'tool', id: 't1', toolKind: 'shell', status: 'ok', seq: 2 }}
        onInspect={vi.fn()}
      />,
    );
    expect(screen.getByRole('button')).toBeDisabled();
  });
});

describe('telling calls apart without reading them', () => {
  it('distinguishes a running call from a finished one', async () => {
    // §18.2, and the transcript the milestone asks for: a turn with two tool
    // calls, one still running when it is read.
    const conversation = await conversationFrom('two-tool-calls');
    const { container } = render(<MessageStream entries={conversation.entries} />);

    const calls = [...container.querySelectorAll('.toolcall')];
    expect(calls).toHaveLength(2);
    expect(calls[0]?.getAttribute('data-tone')).toBe('done');
    expect(calls[1]?.getAttribute('data-tone')).toBe('running');
    // A word as well as a tone, because colour never carries meaning alone.
    expect(within(calls[1] as HTMLElement).getByText('running')).toBeInTheDocument();
  });

  it('distinguishes a failed call from a successful one', async () => {
    const conversation = await conversationFrom('failed-tool-call');
    const { container } = render(<MessageStream entries={conversation.entries} />);
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

    const watching = render(<MessageStream entries={live.entries} />);
    const html = withoutGeneratedIds(watching.container.innerHTML);
    watching.unmount();

    const reading = render(<MessageStream entries={replayed.entries} />);
    expect(withoutGeneratedIds(reading.container.innerHTML)).toBe(html);
  });
});

describe('what a call has to show', () => {
  it('drops nothing from a call output', () => {
    // §18.2: enormous output is agent-emitted content and §1's fidelity rule
    // applies, so it is not silently shortened. Bounding it in a column is a
    // frame decision and permitted; dropping the tail of it is not.
    const enormous = Array.from({ length: 400 }, (_, i) => `line ${String(i)}`).join('\n');
    const sections = sectionsOf({
      kind: 'tool',
      id: 't1',
      toolKind: 'shell',
      status: 'ok',
      output: enormous,
      seq: 2,
    });
    expect(sections.map((section) => section.body)).toEqual([enormous]);
  });

  it('keeps the bodies apart, in the order they happened', () => {
    // What a call was asked to do comes before what it produced, because that
    // is the order a reader reconstructs them in. One region for both meant a
    // long argument payload buried the result.
    const sections = sectionsOf({
      kind: 'tool',
      id: 't1',
      toolKind: 'shell',
      status: 'ok',
      arguments: { text: 'a'.repeat(400), truncated: false },
      result: { text: 'the answer', truncated: false },
      seq: 2,
    });
    expect(sections.map((section) => section.label)).toEqual(['Arguments', 'Result']);
  });

  it('contributes nothing for a body that did not arrive', () => {
    // §18.5: an absence is a fact, not an empty labelled region.
    const sections = sectionsOf({
      kind: 'tool',
      id: 't1',
      toolKind: 'shell',
      status: 'ok',
      result: { text: 'only this', truncated: false },
      seq: 2,
    });
    expect(sections.map((section) => section.label)).toEqual(['Result']);
  });

  it('says which body was cut, not just that something was', () => {
    const sections = sectionsOf({
      kind: 'tool',
      id: 't1',
      toolKind: 'shell',
      status: 'ok',
      arguments: { text: 'asked', truncated: true },
      result: { text: 'got', truncated: false },
      seq: 2,
    });
    expect(sections.map((section) => [section.label, section.truncated === true])).toEqual([
      ['Arguments', true],
      ['Result', false],
    ]);
  });

  it('marks the result of a failed call as the thing that went wrong', () => {
    const sections = sectionsOf({
      kind: 'tool',
      id: 't1',
      toolKind: 'shell',
      status: 'error',
      result: { text: 'no such file', truncated: false },
      seq: 2,
    });
    expect(sections[0]?.failed).toBe(true);
  });
});

describe('the row itself', () => {
  it('leaves no inline body behind in its stylesheet', () => {
    // The rules went with the markup. A stylesheet that still described the
    // old disclosure would be a description of something nothing renders.
    const css = readFileSync(resolve(process.cwd(), 'src/chat/ToolCall.css'), 'utf8');
    expect(css).not.toContain('.toolcall__output');
    expect(css).not.toContain('.toolcall__bodies');
    expect(css).not.toContain('.toolcall__bodylabel');
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
