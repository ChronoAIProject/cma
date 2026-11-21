import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { WriteForm } from './data';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function setup() {
  // user-event supplies a FileList through the DOM property. jsdom's native
  // FormData reads its private FileList instead, so bridge that test-only gap.
  vi.stubGlobal(
    'FormData',
    class extends FormData {
      constructor(form?: HTMLFormElement) {
        super(form);
        for (const input of form?.querySelectorAll<HTMLInputElement>('input[type="file"]') ?? []) {
          const file = input.files?.[0];
          if (file !== undefined) this.set(input.name, file);
        }
      }
    },
  );
  const fetcher = vi.fn().mockResolvedValue(new Response('{}', { status: 201 }));
  vi.stubGlobal('fetch', fetcher);
  const done = vi.fn();
  render(
    <WriteForm
      service="image-manager"
      path="/v1/images/img_one/builds"
      label="Start build"
      fields={[
        { name: 'repository', label: 'Repository', initial: 'https://github.com/example/repo' },
        {
          name: 'dockerfile_content',
          label: 'Import Dockerfile',
          type: 'file',
          maxBytes: 16 * 1024,
        },
      ]}
      onDone={done}
      denied={vi.fn()}
    />,
  );
  const input = screen.getByLabelText('Import Dockerfile') as HTMLInputElement;
  const submit = () => fireEvent.submit(input.form!);
  return { actor: userEvent.setup(), fetcher, done, input, submit };
}

it('imports the exact file once and preserves the request for an uncertain retry', async () => {
  const test = setup();
  const content = 'FROM alpine:3.20\nRUN echo "hello"\n';
  test.fetcher.mockRejectedValueOnce(new TypeError('Network failure'));
  await test.actor.upload(test.input, new File([content], 'Dockerfile', { type: 'text/plain' }));
  test.submit();
  test.submit();
  expect(screen.getByRole('button', { name: 'Submitting…' })).toBeDisabled();
  expect(test.input).toBeDisabled();
  await screen.findByRole('button', { name: 'Retry same request' });
  expect(test.fetcher).toHaveBeenCalledOnce();
  const request = test.fetcher.mock.calls[0]?.[1] as RequestInit;
  expect(JSON.parse(request.body as string)).toEqual({
    repository: 'https://github.com/example/repo',
    dockerfile_content: content,
  });
  test.submit();
  await waitFor(() => expect(test.done).toHaveBeenCalledOnce());
  expect(test.fetcher.mock.calls[1]?.[1].body).toBe(request.body);
  expect(test.fetcher.mock.calls[1]?.[1].headers).toEqual(request.headers);
});

it.each([
  ['empty', new File([], 'Dockerfile'), /must contain/],
  ['oversized', new File(['x'.repeat(16 * 1024 + 1)], 'Dockerfile'), /must contain/],
  ['binary', new File(['FROM alpine\0'], 'Dockerfile'), /plain UTF-8/],
  ['invalid UTF-8', new File([new Uint8Array([0xff, 0xfe])], 'Dockerfile'), /plain UTF-8/],
])('rejects an %s import before submitting a build', async (_, file, message) => {
  const test = setup();
  await test.actor.upload(test.input, file);
  test.submit();
  expect(await screen.findByRole('alert')).toHaveTextContent(message);
  expect(test.fetcher).not.toHaveBeenCalled();
  expect(test.input).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Start build' })).toBeEnabled();
});

it('shows file read failures and permits choosing a replacement', async () => {
  const test = setup();
  await test.actor.upload(test.input, new File(['FROM alpine'], 'Dockerfile'));
  const read = vi.spyOn(FileReader.prototype, 'readAsArrayBuffer').mockImplementation(function (
    this: FileReader,
  ) {
    this.dispatchEvent(new ProgressEvent('error'));
  });
  await act(async () => test.submit());
  expect(screen.getByRole('alert')).toHaveTextContent(
    'The file could not be read. Choose it again.',
  );
  expect(test.fetcher).not.toHaveBeenCalled();
  read.mockRestore();
  await test.actor.upload(test.input, new File(['FROM scratch'], 'Dockerfile'));
  test.submit();
  await waitFor(() => expect(test.done).toHaveBeenCalledOnce());
});

it('omits an optional unselected import from the build request', async () => {
  const test = setup();
  test.submit();
  await waitFor(() => expect(test.done).toHaveBeenCalledOnce());
  expect(JSON.parse(test.fetcher.mock.calls[0]?.[1].body as string)).toEqual({
    repository: 'https://github.com/example/repo',
  });
});
