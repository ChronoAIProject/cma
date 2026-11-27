import {
  PREVIEW_BYTES,
  MAX_FILE_BYTES,
  fileBytes,
  type AgentFileContent,
  type AgentFileEntry,
} from '../../api/agentFiles';
import { Button } from '../../ui/Button';
import { IconDownloadOutline16, IconTrashOutline16 } from '../../ui/icons';
import { SelectedFilePath } from './FilePaths';
import { ImagePreview } from './ImagePreview';

export function textContent(content: AgentFileContent): string | undefined {
  try {
    const bytes = fileBytes(content.content_base64);
    if (bytes.includes(0)) return undefined;
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return undefined;
  }
}
export function imageType(name: string): string | undefined {
  const extension = name.toLowerCase().split('.').at(-1);
  return (
    {
      png: 'image/png',
      jpg: 'image/jpeg',
      jpeg: 'image/jpeg',
      gif: 'image/gif',
      webp: 'image/webp',
      avif: 'image/avif',
    } as Record<string, string>
  )[extension ?? ''];
}
export function canPreviewFile(file: AgentFileEntry): boolean {
  return file.size_bytes <= (imageType(file.name) ? MAX_FILE_BYTES : PREVIEW_BYTES);
}

export function FilePreview({
  file,
  content,
  draft,
  loading,
  busy,
  readOnly,
  imageSource,
  focusExpandImage,
  onExpandImage,
  onDraft,
  onSave,
  onReload,
  onDownload,
  onDelete,
  hideHeader = false,
}: {
  readonly file?: AgentFileEntry;
  readonly content?: AgentFileContent;
  readonly draft?: string;
  readonly loading: boolean;
  readonly busy: boolean;
  readonly readOnly: boolean;
  readonly imageSource?: string | Blob;
  readonly focusExpandImage?: boolean;
  readonly onExpandImage?: () => void;
  readonly onDraft: (value: string) => void;
  readonly onSave: () => void;
  readonly onReload: () => void;
  readonly onDownload: () => void;
  readonly onDelete: () => void;
  readonly hideHeader?: boolean;
}) {
  const original = content && draft !== undefined ? textContent(content) : undefined;
  const dirty = draft !== undefined && draft !== original;
  return (
    <section
      className="agent-files__preview"
      data-file-selected={file !== undefined}
      aria-label="File preview"
      aria-busy={loading}
    >
      {!file ? (
        <p className="agent-files__empty">Select a file to preview it.</p>
      ) : (
        <>
          {!hideHeader ? (
            <header className="agent-files__file-header">
              <code title={`/home/${file.path}`}>{file.name}</code>
              {dirty ? <span aria-label="Unsaved changes">Unsaved</span> : null}
              <div className="agent-files__actions">
                <Button
                  size="sm"
                  icon={<IconDownloadOutline16 />}
                  aria-label="Download file"
                  title="Download file"
                  disabled={busy}
                  onClick={onDownload}
                />
                {!readOnly ? (
                  <Button
                    size="sm"
                    icon={<IconTrashOutline16 />}
                    aria-label="Delete file"
                    title="Delete file"
                    disabled={busy}
                    onClick={onDelete}
                  />
                ) : null}
              </div>
            </header>
          ) : null}
          {!hideHeader ? <SelectedFilePath key={file.path} path={file.path} /> : null}
          {loading ? (
            <p role="status">Opening file…</p>
          ) : imageSource && onExpandImage ? (
            <ImagePreview
              key={`image:${file.path}`}
              source={imageSource}
              name={file.name}
              initialFocus={focusExpandImage}
              onToggle={onExpandImage}
            />
          ) : draft !== undefined ? (
            <>
              <textarea
                className="agent-files__editor"
                aria-label={`Contents of ${file.name}`}
                value={draft}
                readOnly={readOnly || busy}
                spellCheck={false}
                onChange={(event) => onDraft(event.target.value)}
                onKeyDown={(event) => {
                  if ((event.metaKey || event.ctrlKey) && event.key === 's') {
                    event.preventDefault();
                    if (!readOnly && dirty && !busy) onSave();
                  }
                }}
              />
              {!readOnly ? (
                <div className="agent-files__editor-footer">
                  <Button size="sm" disabled={busy || !dirty} onClick={onReload}>
                    Reload file
                  </Button>
                  <Button size="sm" variant="primary" disabled={busy || !dirty} onClick={onSave}>
                    {busy ? 'Saving…' : 'Save'}
                  </Button>
                </div>
              ) : null}
            </>
          ) : !content && canPreviewFile(file) ? (
            <div role="status">
              <p>File preview unavailable.</p>
              <Button size="sm" disabled={busy} onClick={onReload}>
                Retry preview
              </Button>
            </div>
          ) : (
            <p className="agent-files__empty" role="status">
              Preview unavailable. Download this file to view its contents.
            </p>
          )}
        </>
      )}
    </section>
  );
}
