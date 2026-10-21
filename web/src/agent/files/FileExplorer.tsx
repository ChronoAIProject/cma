import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AgentFileError,
  MAX_FILE_BYTES,
  PREVIEW_BYTES,
  fileBase64,
  type AgentFileContent,
  type AgentFileEntry,
  type AgentFilesClient,
} from '../../api/agentFiles';
import { Alert } from '../../ui/Alert';
import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { Modal } from '../../ui/Modal';
import { IconFolderOpen16 } from '../../ui/icons';
import { FileToolbar } from './FileToolbar';
import { FileTree } from './FileTree';
import { FilePreview, textContent } from './FilePreview';
import './AgentFiles.css';

export function FileExplorer({
  client,
  agentId,
  readOnly,
  onClose,
  onAccessChanged,
}: {
  readonly client: AgentFilesClient;
  readonly agentId: string;
  readonly readOnly: boolean;
  readonly onClose: () => void;
  readonly onAccessChanged?: () => void;
}) {
  const [directories, setDirectories] = useState<ReadonlyMap<string, readonly AgentFileEntry[]>>(
    new Map(),
  );
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [loading, setLoading] = useState<ReadonlySet<string>>(new Set(['']));
  const [hidden, setHidden] = useState(false);
  const [directory, setDirectory] = useState('');
  const [selected, setSelected] = useState<AgentFileEntry>();
  const [content, setContent] = useState<AgentFileContent>();
  const [draft, setDraft] = useState<string>();
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [denied, setDenied] = useState(false);
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [create, setCreate] = useState<'file' | 'directory'>();
  const [name, setName] = useState('');
  const [confirm, setConfirm] = useState<{ text: string; action: () => void }>();
  const lifetime = useRef<AbortController | undefined>(undefined);
  const readRequest = useRef<AbortController | undefined>(undefined);
  const mutation = useRef(false);
  const dirty = draft !== undefined && content !== undefined && draft !== textContent(content);
  const blocked = readOnly || denied;

  const failure = useCallback(
    (cause: unknown) => {
      if (lifetime.current?.signal.aborted) return;
      setError(
        cause instanceof AgentFileError && cause.code === 'file_runtime_unsupported'
          ? 'File browsing requires the latest Sandbox runtime. The owner can update it from Sandbox details when the Agent is idle.'
          : cause instanceof Error
            ? cause.message
            : 'Files could not be accessed. Try again.',
      );
      if (
        cause instanceof AgentFileError &&
        ([401, 403].includes(cause.status) || cause.code === 'resource_not_found')
      ) {
        lifetime.current?.abort();
        readRequest.current?.abort();
        setDenied(true);
        setLoading(new Set());
        setReading(false);
        setBusy(false);
        setDirectories(new Map());
        setContent(undefined);
        setDraft(undefined);
        setSelected(undefined);
        onAccessChanged?.();
      }
    },
    [onAccessChanged],
  );
  const load = useCallback(
    async (path: string, signal: AbortSignal) => {
      setLoading((value) => new Set([...value, path]));
      try {
        const entries = await client.list(agentId, path, { signal });
        if (!signal.aborted) setDirectories((value) => new Map(value).set(path, entries));
      } catch (cause) {
        if (!signal.aborted) failure(cause);
      } finally {
        if (!signal.aborted)
          setLoading((value) => {
            const next = new Set(value);
            next.delete(path);
            return next;
          });
      }
    },
    [agentId, client, failure],
  );
  useEffect(() => {
    const request = new AbortController();
    lifetime.current = request;
    queueMicrotask(() => {
      if (!request.signal.aborted) void load('', request.signal);
    });
    return () => {
      request.abort();
      readRequest.current?.abort();
    };
  }, [load]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(undefined), 4000);
    return () => clearTimeout(timer);
  }, [notice]);

  const guardDraft = (action: () => void) => {
    if (mutation.current) return;
    if (dirty) setConfirm({ text: 'Discard the unsaved changes to this file?', action });
    else action();
  };
  const openFile = async (file: AgentFileEntry) => {
    readRequest.current?.abort();
    const request = new AbortController();
    readRequest.current = request;
    setSelected(file);
    setContent(undefined);
    setDraft(undefined);
    setError(undefined);
    if (file.size_bytes > PREVIEW_BYTES) {
      setReading(false);
      return;
    }
    setReading(true);
    try {
      const value = await client.read(agentId, file.path, { signal: request.signal });
      if (!request.signal.aborted) {
        setContent(value);
        setDraft(textContent(value));
      }
    } catch (cause) {
      if (!request.signal.aborted) failure(cause);
    } finally {
      if (!request.signal.aborted) setReading(false);
    }
  };
  const run = async (action: (signal: AbortSignal) => Promise<void>) => {
    const signal = lifetime.current?.signal;
    if (mutation.current || !signal || signal.aborted || denied) return;
    mutation.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await action(signal);
    } catch (cause) {
      if (!signal.aborted) failure(cause);
    } finally {
      mutation.current = false;
      if (!signal.aborted) setBusy(false);
    }
  };
  const refresh = async (path: string, signal: AbortSignal) => {
    await load(path, signal);
  };
  const save = () => {
    if (blocked || !selected || !content || draft === undefined || !dirty) return;
    const pending = { file: selected, original: content, draft };
    void run(async (signal) => {
      const encoded = fileBase64(new TextEncoder().encode(pending.draft));
      const saved = await client.write(
        agentId,
        {
          path: pending.file.path,
          content_base64: encoded,
          expected_sha256: pending.original.sha256,
        },
        { signal },
      );
      if (signal.aborted) return;
      setContent({
        ...pending.original,
        sha256: saved.sha256!,
        size_bytes: saved.size_bytes!,
        content_base64: encoded,
      });
      setNotice('File saved.');
      await refresh(pending.file.path.split('/').slice(0, -1).join('/'), signal);
    });
  };
  const download = () => {
    if (!selected) return;
    const file = selected;
    void run(async (signal) => {
      const blob = await client.download(agentId, file.path, { signal });
      if (signal.aborted) return;
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = file.name;
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    });
  };
  const remove = (path: string, folder: boolean) => {
    if (blocked) return;
    setConfirm({
      text: `Delete /home/${path}${folder ? ' and everything inside it' : ''}? This cannot be undone.`,
      action: () => {
        void run(async (signal) => {
          await client.remove(agentId, path, folder, { signal });
          if (signal.aborted) return;
          readRequest.current?.abort();
          setSelected(undefined);
          setContent(undefined);
          setDraft(undefined);
          const parent = path.split('/').slice(0, -1).join('/');
          if (folder) {
            setDirectory(parent);
            setDirectories(
              (current) =>
                new Map(
                  [...current].filter(([key]) => key !== path && !key.startsWith(`${path}/`)),
                ),
            );
          }
          setNotice('Deleted.');
          await refresh(parent, signal);
        });
      },
    });
  };
  const createFile = () => {
    if (
      blocked ||
      !create ||
      !name.trim() ||
      /[/\\]/.test(name) ||
      [...name].some((character) => character.charCodeAt(0) < 32) ||
      name === '.' ||
      name === '..'
    ) {
      setError('Enter a file or folder name without path separators.');
      return;
    }
    const path = (directory ? `${directory}/` : '') + name;
    const folder = create === 'directory';
    void run(async (signal) => {
      await client.write(agentId, { path, content_base64: '', directory: folder }, { signal });
      if (signal.aborted) return;
      setCreate(undefined);
      setName('');
      await refresh(directory, signal);
      if (!folder)
        await openFile({ path, name, kind: 'file', hidden: name.startsWith('.'), size_bytes: 0 });
      setNotice(folder ? 'Folder created.' : 'File created.');
    });
  };
  const uploadFiles = (files: readonly File[]) => {
    if (blocked || !files.length) return;
    void run(async (signal) => {
      for (const file of files) {
        if (signal.aborted) return;
        if (file.size > MAX_FILE_BYTES) throw new Error(`${file.name} must be 32 MiB or smaller.`);
        if (
          /[/\\]/.test(file.name) ||
          [...file.name].some((character) => character.charCodeAt(0) < 32)
        )
          throw new Error('The filename is invalid.');
        const bytes = new Uint8Array(await file.arrayBuffer());
        await client.write(
          agentId,
          {
            path: (directory ? `${directory}/` : '') + file.name,
            content_base64: fileBase64(bytes),
          },
          { signal },
        );
        await refresh(directory, signal);
      }
      if (!signal.aborted)
        setNotice(`${files.length} file${files.length === 1 ? '' : 's'} uploaded.`);
    });
  };

  return (
    <Modal
      open
      title="Files"
      description="Browse the persistent /home/ directory."
      placement="agent-files"
      dismissible={!busy}
      onClose={() => guardDraft(onClose)}
    >
      <FileToolbar
        hidden={hidden}
        onHidden={setHidden}
        busy={busy}
        blocked={blocked}
        denied={denied}
        onRefresh={() => {
          const signal = lifetime.current?.signal;
          if (signal) void refresh(directory, signal);
        }}
        onCreate={(kind) =>
          guardDraft(() => {
            setCreate(kind);
            setName('');
          })
        }
        onUpload={uploadFiles}
      />
      {error ? (
        <Alert
          tone="error"
          actions={
            <Button size="sm" onClick={() => setError(undefined)}>
              Dismiss
            </Button>
          }
        >
          {error}
        </Alert>
      ) : null}
      {notice ? (
        <Alert
          tone="success"
          actions={
            <Button size="sm" onClick={() => setNotice(undefined)}>
              Dismiss
            </Button>
          }
        >
          {notice}
        </Alert>
      ) : null}
      {confirm ? (
        <Alert
          tone="warning"
          actions={
            <>
              <Button size="sm" onClick={() => setConfirm(undefined)}>
                Cancel
              </Button>
              <Button
                size="sm"
                variant="danger"
                onClick={() => {
                  const action = confirm.action;
                  setConfirm(undefined);
                  action();
                }}
              >
                Confirm
              </Button>
            </>
          }
        >
          {confirm.text}
        </Alert>
      ) : null}
      {create ? (
        <form
          className="agent-files__create"
          onSubmit={(event) => {
            event.preventDefault();
            createFile();
          }}
        >
          <label>
            {create === 'directory' ? 'Folder name' : 'File name'}
            <Input
              aria-label={create === 'directory' ? 'Folder name' : 'File name'}
              value={name}
              onChange={(event) => setName(event.target.value)}
              maxLength={255}
            />
          </label>
          <Button size="sm" disabled={busy} onClick={() => setCreate(undefined)}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" type="submit" disabled={busy || !name.trim()}>
            Create
          </Button>
        </form>
      ) : null}
      <div
        className="agent-files__layout"
        onDragOver={(event) => {
          if (!blocked && event.dataTransfer.types.includes('Files')) event.preventDefault();
        }}
        onDrop={(event) => {
          if (blocked || !event.dataTransfer.files.length) return;
          event.preventDefault();
          uploadFiles(Array.from(event.dataTransfer.files));
        }}
      >
        <nav className="agent-files__rail" aria-label="File explorer">
          <button
            type="button"
            className="agent-files__row"
            disabled={busy || denied}
            onClick={() => setDirectory('')}
            aria-current={directory === '' ? 'true' : undefined}
          >
            <IconFolderOpen16 />
            <span>/home/</span>
          </button>
          {loading.has('') ? <p role="status">Loading files…</p> : null}
          <FileTree
            directories={directories}
            expanded={expanded}
            hidden={hidden}
            selected={selected?.path}
            loading={loading}
            disabled={busy || denied}
            onFile={(file) => guardDraft(() => void openFile(file))}
            onDirectory={(path) => {
              setDirectory(path);
              setExpanded((value) => {
                const next = new Set(value);
                if (next.has(path)) next.delete(path);
                else next.add(path);
                return next;
              });
              const signal = lifetime.current?.signal;
              if (!directories.has(path) && signal) void load(path, signal);
            }}
          />
          {!loading.has('') && directories.get('')?.length === 0 ? <p>No files yet.</p> : null}
        </nav>
        <FilePreview
          file={selected}
          content={content}
          draft={draft}
          loading={reading}
          busy={busy}
          readOnly={blocked}
          onDraft={setDraft}
          onSave={save}
          onReload={() => {
            if (selected) guardDraft(() => void openFile(selected));
          }}
          onDownload={download}
          onDelete={() => {
            if (selected) remove(selected.path, false);
          }}
        />
      </div>
      <footer className="agent-files__location">
        <code title={`/home/${directory}`}>/home/{directory}</code>
        {directory && !blocked ? (
          <Button
            size="sm"
            variant="danger"
            disabled={busy}
            onClick={() => remove(directory, true)}
          >
            Delete folder
          </Button>
        ) : null}
      </footer>
    </Modal>
  );
}
