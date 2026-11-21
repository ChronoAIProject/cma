import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { Composer } from './Composer';
import type { UploadAttachment } from './attachments';
import { useAgentDraft } from './draft';
import { agentClient } from '../test/agent';

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}
const props = {
  status: 'ready' as const,
  running: false,
  busy: false,
  inputAvailability: { allowed: true },
  onStop: vi.fn(),
};

it.each(['click', 'enter'])(
  'hands off synchronously on %s and restores exact untouched input on rejection',
  async (method) => {
    const request = deferred();
    const send = vi.fn(() => request.promise);
    render(<Composer {...props} onSend={send} />);
    const field = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(field, { target: { value: '  Exact draft\nwith whitespace  ' } });
    if (method === 'click') fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
    else fireEvent.keyDown(field, { key: 'Enter' });
    expect(field).toHaveValue('');
    expect(send).toHaveBeenCalledExactlyOnceWith(
      'Exact draft\nwith whitespace',
      expect.any(Function),
    );
    fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
    expect(send).toHaveBeenCalledOnce();
    await act(async () => request.reject(new Error('Refused')));
    expect(field).toHaveValue('  Exact draft\nwith whitespace  ');
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
  },
);

it('does not erase new identical text through either late acceptance path', async () => {
  const request = deferred();
  let accept!: () => void;
  const send = vi.fn((_text: string, callback: () => void) => {
    accept = callback;
    return request.promise;
  });
  const view = render(<Composer {...props} onSend={send} />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Same text, new intent' } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(field).toHaveValue('');
  fireEvent.change(field, { target: { value: 'Same text, new intent' } });
  act(() => accept());
  view.rerender(<Composer {...props} onSend={send} acceptedMessage="Same text, new intent" />);
  await act(async () => request.resolve());
  expect(field).toHaveValue('Same text, new intent');
});

it('restores ready files with their original receipts after rejection without uploading them again', async () => {
  const request = deferred();
  const file = new File(['contents'], 'original.txt', { type: 'text/plain' });
  const upload = vi.fn<UploadAttachment>().mockResolvedValue({
    id: 'att_original',
    agentId: 'agt_1',
    name: file.name,
    path: '/home/attachments/agt_1/att_original/original.txt',
    size: file.size,
    mediaType: file.type,
    sha256: 'a'.repeat(64),
  });
  const send = vi.fn(() => request.promise);
  render(<Composer {...props} uploadAttachment={upload} onSend={send} />);
  const field = screen.getByRole('textbox');
  fireEvent.change(field, { target: { value: 'Review my file' } });
  fireEvent.drop(field, { dataTransfer: { files: [file] } });
  await screen.findByText('Ready');
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(field).toHaveValue('');
  expect(screen.queryByRole('list', { name: 'Attachments' })).not.toBeInTheDocument();
  expect(send).toHaveBeenCalledWith(
    expect.stringContaining('/home/attachments/agt_1/att_original/original.txt'),
    expect.any(Function),
  );
  await act(async () => request.reject(new Error('Refused')));
  expect(field).toHaveValue('Review my file');
  expect(screen.getByText('original.txt')).toBeInTheDocument();
  expect(screen.getByText('Ready')).toBeInTheDocument();
  expect(upload).toHaveBeenCalledOnce();
});

it('keeps later files and text when an old acceptance callback is repeated', async () => {
  const request = deferred();
  let accept!: () => void;
  const upload = vi.fn<UploadAttachment>(async (file) => ({
    id: file.name,
    agentId: 'agt_1',
    name: file.name,
    path: `/home/attachments/agt_1/${file.name}`,
    size: file.size,
    mediaType: file.type,
    sha256: 'b'.repeat(64),
  }));
  const send = vi.fn((_text: string, callback: () => void) => {
    accept = callback;
    return request.promise;
  });
  render(<Composer {...props} uploadAttachment={upload} onSend={send} />);
  const field = screen.getByRole('textbox');
  fireEvent.drop(field, { dataTransfer: { files: [new File(['first'], 'first.txt')] } });
  await screen.findByText('Ready');
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  fireEvent.change(field, { target: { value: 'Next draft' } });
  await act(async () => {
    accept();
    request.resolve();
  });
  fireEvent.drop(field, { dataTransfer: { files: [new File(['next'], 'next.txt')] } });
  await waitFor(() => expect(screen.getByText('Ready')).toBeInTheDocument());
  act(() => accept());
  expect(field).toHaveValue('Next draft');
  expect(screen.getByText('next.txt')).toBeInTheDocument();
  expect(screen.queryByText('first.txt')).not.toBeInTheDocument();
  expect(upload).toHaveBeenCalledTimes(2);
});

it('retains the hidden snapshot across navigation and never clears new input on a later visit', async () => {
  const client = agentClient();
  const request = deferred();
  let accept!: () => void;
  const send = vi.fn((_text: string, callback: () => void) => {
    accept = callback;
    return request.promise;
  });
  function Surface({ agentId, accepted }: { agentId: string; accepted?: string }) {
    const draft = useAgentDraft(client, 'owner', agentId);
    return (
      <Composer key={agentId} {...props} draft={draft} onSend={send} acceptedMessage={accepted} />
    );
  }
  const view = render(<Surface agentId="agt_1" />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Same text, new visit' } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(field).toHaveValue('');
  view.rerender(<Surface agentId="agt_2" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('');
  view.rerender(<Surface agentId="agt_1" />);
  const returningField = screen.getByRole('textbox', { name: 'Message' });
  expect(returningField).toHaveValue('Same text, new visit');
  fireEvent.change(returningField, { target: { value: '' } });
  fireEvent.change(returningField, { target: { value: 'Same text, new visit' } });
  act(() => accept());
  view.rerender(<Surface agentId="agt_1" accepted="Same text, new visit" />);
  await act(async () => request.resolve());
  expect(returningField).toHaveValue('Same text, new visit');
  expect(send).toHaveBeenCalledOnce();
});
