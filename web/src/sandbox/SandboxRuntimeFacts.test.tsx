import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { SandboxRuntimeFacts } from './SandboxRuntimeFacts';

afterEach(() => vi.unstubAllGlobals());
const details = { sandboxId: 'sandbox-fixture', runtimeVersion: 'runtime@sha256:fixture' };
function clipboard(writeText = vi.fn().mockResolvedValue(undefined)) {
  vi.stubGlobal('navigator', { clipboard: { writeText } });
  return writeText;
}
const copy = async () =>
  act(async () => fireEvent.click(screen.getByRole('button', { name: 'Copy technical details' })));

it('copies the displayed identity and runtime facts only after an explicit action', async () => {
  const write = clipboard();
  render(<SandboxRuntimeFacts {...details} copyable />);
  expect(write).not.toHaveBeenCalled();
  await copy();
  expect(write).toHaveBeenCalledExactlyOnceWith(
    'Sandbox ID: sandbox-fixture\nCurrent runtime: runtime@sha256:fixture',
  );
  expect(screen.getByRole('status')).toHaveTextContent('Technical details copied.');
});

it.each(['denied', 'unavailable'])(
  'provides selected, read-only text when clipboard is %s',
  async (mode) => {
    if (mode === 'denied') clipboard(vi.fn().mockRejectedValue(new Error('Clipboard denied')));
    else vi.stubGlobal('navigator', {});
    render(<SandboxRuntimeFacts {...details} copyable />);
    await copy();
    const field = screen.getByRole<HTMLTextAreaElement>('textbox', {
      name: 'Technical details to copy',
    });
    expect(screen.getByRole('status')).toHaveTextContent('Copy unavailable.');
    expect(field).toHaveFocus();
    expect(field).toHaveAttribute('readonly');
    expect(field.selectionStart).toBe(0);
    expect(field.selectionEnd).toBe(field.value.length);
    expect(field.value).toContain(details.sandboxId);
  },
);

it('does not report an older copy result for changed facts', async () => {
  let resolve!: () => void;
  clipboard(
    vi.fn().mockReturnValue(
      new Promise<void>((done) => {
        resolve = done;
      }),
    ),
  );
  const view = render(<SandboxRuntimeFacts {...details} copyable />);
  await copy();
  view.rerender(<SandboxRuntimeFacts {...details} runtimeVersion="runtime:new" copyable />);
  await act(async () => resolve());
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(screen.getByText('runtime:new')).toBeVisible();
});

it('keeps the latest copy failure when an older attempt completes later', async () => {
  let resolve!: () => void;
  clipboard(
    vi
      .fn()
      .mockReturnValueOnce(
        new Promise<void>((done) => {
          resolve = done;
        }),
      )
      .mockRejectedValueOnce(new Error('Clipboard denied')),
  );
  render(<SandboxRuntimeFacts {...details} copyable />);
  await copy();
  await copy();
  await act(async () => resolve());
  expect(screen.getByRole('status')).toHaveTextContent('Copy unavailable.');
  expect(screen.getByRole('textbox')).toHaveFocus();
});
