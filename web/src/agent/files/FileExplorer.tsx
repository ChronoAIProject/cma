import { useFileNavigation, type FileNavigationMemory } from './useFileNavigation';
import { FileRoots } from './FilePaths';
import { absoluteFilePath } from './filePath';
import { homeFilePath } from '../fileChanges';
import { ChangedFiles } from './ChangedFiles';
import type { FileChange } from '../fileChanges';
import { saveAgentDownload } from '../saveDelivery';
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
import { FileCreateForm, fileNameIssue } from './FileCreateForm';
import { FileExplorerFrame, FileLocation } from './FileExplorerFrame';
import { FileToolbar } from './FileToolbar';
import { FileRail } from './FileRail';
import { FilePreview, textContent } from './FilePreview';
import { useDownloadInterruption } from './useDownloadInterruption';
import './AgentFiles.css';

export function FileExplorer({
  client,
  agentId,
  readOnly,
  onClose,
  onAccessChanged,
  beginDownload,
  panel = false,
  embedded = false,
  active = true,
  initialDirectory: suppliedDirectory,
  navigationMemory,
  changes,
}: {
  readonly initialDirectory?: string;
  readonly navigationMemory?: FileNavigationMemory;
  readonly changes?: ReadonlyMap<string, FileChange>;
  readonly panel?: boolean;
  readonly embedded?: boolean;
  readonly active?: boolean;
  readonly client: AgentFilesClient;
  readonly agentId: string;
  readonly readOnly: boolean;
  readonly onClose: () => void;
  readonly onAccessChanged?: (downloadMessage?: string) => void;
  readonly beginDownload?: () => (message: string) => void;
}) {
  const knownDirectory =
    suppliedDirectory === undefined
      ? undefined
      : homeFilePath(absoluteFilePath(suppliedDirectory) ?? '');
  const initialDirectory = knownDirectory ?? '';
  const [directories, setDirectories] = useState<ReadonlyMap<string, readonly AgentFileEntry[]>>(
    new Map(),
  );
  const [loading, setLoading] = useState<ReadonlySet<string>>(
    new Set([navigationMemory?.read() ?? initialDirectory]),
  );
  const [hidden, setHidden] = useState(false);
  const { initialRoot, rootPath, directory, expanded, navigate, setDirectory, touch } =
    useFileNavigation(initialDirectory, navigationMemory);
  const [selected, setSelected] = useState<AgentFileEntry>();
  const [content, setContent] = useState<AgentFileContent>();
  const [draft, setDraft] = useState<string>();
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [denied, setDenied] = useState(false);
  const [error, setErrorState] = useState<string>();
  // A later successful listing of the failed folder clears its alert (#2268).
  const listingFailure = useRef<string | undefined>(undefined);
  const setError = useCallback((message?: string) => {
    listingFailure.current = undefined;
    setErrorState(message);
  }, []);
  const beginInterruptionNotice = useDownloadInterruption(setError, beginDownload);
  const [notice, setNotice] = useState<string>();
  const [create, setCreate] = useState<'file' | 'directory'>();
  const [name, setName] = useState('');
  const [confirm, setConfirm] = useState<{
    text: string;
    action: (currentFile?: AgentFileEntry) => void;
  }>();
  const lifetime = useRef<AbortController | undefined>(undefined);
  const readRequest = useRef<AbortController | undefined>(undefined);
  const mutation = useRef(false);
  const dirty = draft !== undefined && content !== undefined && draft !== textContent(content);
  const blocked = readOnly || denied;

  const failure = useCallback(
    (cause: unknown, download = false) => {
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
        if (download) onAccessChanged?.(cause.message);
        else onAccessChanged?.();
      }
    },
    [onAccessChanged, setError],
  );
  const load = useCallback(
    async (path: string, signal: AbortSignal) => {
      if (signal.aborted) return false;
      setLoading((value) => new Set([...value, path]));
      try {
        const entries = await client.list(agentId, path, { signal });
        if (!signal.aborted) {
          setDirectories((value) => new Map(value).set(path, entries));
          if (listingFailure.current === path) setError(undefined);
        }
        return !signal.aborted;
      } catch (cause) {
        if (!signal.aborted) {
          failure(cause);
          listingFailure.current = path;
        }
        return false;
      } finally {
        if (!signal.aborted)
          setLoading((value) => {
            const next = new Set(value);
            next.delete(path);
            return next;
          });
      }
    },
    [agentId, client, failure, setError],
  );
  useEffect(() => {
    if (!active) return;
    const request = new AbortController();
    lifetime.current = request;
    queueMicrotask(() => {
      if (!request.signal.aborted) {
        void load(initialRoot, request.signal);
        const remembered = navigationMemory?.read();
        if (remembered !== undefined && remembered !== initialRoot)
          void load(remembered, request.signal);
      }
    });
    return () => {
      request.abort();
      readRequest.current?.abort();
    };
  }, [load, active, initialRoot, navigationMemory]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(undefined), 4000);
    return () => clearTimeout(timer);
  }, [notice]);

  const navigateDirectory = (path: string, root = false) =>
    navigate(path, root, lifetime.current?.signal, directories.has(path), load);
  const guardDraft = (action: () => void) => {
    if (mutation.current) return;
    touch();
    if (dirty) setConfirm({ text: 'Discard the unsaved changes to this file?', action });
    else action();
  };
  const openFile = async (file: AgentFileEntry) => {
    if (!active || lifetime.current?.signal.aborted) return;
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
  const run = async (action: (signal: AbortSignal) => Promise<void>, download = false) => {
    const signal = lifetime.current?.signal;
    if (!active || mutation.current || !signal || signal.aborted || denied) return;
    mutation.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await action(signal);
    } catch (cause) {
      if (!signal.aborted) failure(cause, download);
    } finally {
      mutation.current = false;
      setBusy(false);
    }
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
      await load(pending.file.path.split('/').slice(0, -1).join('/'), signal);
    });
  };
  const download = () => {
    if (!selected) return;
    const file = selected;
    void run(async (signal) => {
      const reportInterruption = beginInterruptionNotice();
      setDownloading(true);
      setNotice(undefined);
      try {
        const blob = await client.download(agentId, file.path, { signal });
        signal.throwIfAborted();
        saveAgentDownload(blob, file.name);
      } finally {
        setDownloading(false);
        if (signal.aborted) reportInterruption();
      }
    }, true);
  };
  const remove = (path: string, folder: boolean) => {
    if (blocked) return;
    setConfirm({
      text: `Delete /home/${path}${folder ? ' and everything inside it' : ''}? This cannot be undone.`,
      action: (currentFile) => {
        const clearsSelection =
          currentFile?.path === path ||
          (folder && currentFile?.path.startsWith(`${path}/`) === true);
        void run(async (signal) => {
          await client.remove(agentId, path, folder, { signal });
          if (signal.aborted) return;
          if (clearsSelection) {
            readRequest.current?.abort();
            setSelected(undefined);
            setContent(undefined);
            setDraft(undefined);
          }
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
          await load(parent, signal);
        });
      },
    });
  };
  const createFile = () => {
    if (blocked || !create) return;
    const issue = fileNameIssue(name);
    if (issue) {
      setError(issue);
      return;
    }
    const path = (directory ? `${directory}/` : '') + name;
    const folder = create === 'directory';
    void run(async (signal) => {
      await client.write(agentId, { path, content_base64: '', directory: folder }, { signal });
      if (signal.aborted) return;
      setCreate(undefined);
      setName('');
      await load(directory, signal);
      if (signal.aborted) return;
      if (!folder)
        await openFile({ path, name, kind: 'file', hidden: name.startsWith('.'), size_bytes: 0 });
      setNotice(folder ? 'Folder created.' : 'File created.');
    });
  };
  const uploadFiles = (files: readonly File[]) => {
    if (blocked || !files.length) return;
    touch();
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
        await load(directory, signal);
      }
      if (!signal.aborted)
        setNotice(`${files.length} file${files.length === 1 ? '' : 's'} uploaded.`);
    });
  };

  if (!active) return <p role="status">Checking file access. Your unsaved changes are kept.</p>;
  return (
    <FileExplorerFrame
      panel={panel}
      embedded={embedded}
      busy={busy}
      onClose={() => guardDraft(onClose)}
    >
      <FileRoots directory={knownDirectory} />
      <ChangedFiles
        changes={changes}
        directory={initialDirectory}
        client={client}
        agentId={agentId}
        active={active && !denied}
        busy={busy}
        onError={failure}
        onSelect={(file) => guardDraft(() => void openFile(file))}
      />
      <FileToolbar
        hidden={hidden}
        onHidden={setHidden}
        busy={busy}
        blocked={blocked}
        denied={denied}
        onRefresh={() => {
          const signal = lifetime.current?.signal;
          if (signal) void load(directory, signal);
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
      {downloading ? <p role="status">Downloading file…</p> : null}
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
                  action(selected);
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
        <FileCreateForm
          kind={create}
          name={name}
          busy={busy}
          onName={setName}
          onCancel={() => setCreate(undefined)}
          onCreate={createFile}
        />
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
        <FileRail
          rootPath={rootPath}
          directory={directory}
          directories={directories}
          loading={loading}
          expanded={expanded}
          changes={changes}
          hidden={hidden}
          selected={selected?.path}
          disabled={busy || denied}
          onRoot={() => void navigateDirectory('', true)}
          onFile={(file) => guardDraft(() => void openFile(file))}
          onDirectory={(path) => void navigateDirectory(path)}
        />
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
      <FileLocation
        directory={directory}
        canDelete={!blocked}
        busy={busy}
        onDelete={() => remove(directory, true)}
      />
    </FileExplorerFrame>
  );
}
