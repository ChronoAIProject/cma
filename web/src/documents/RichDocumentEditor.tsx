import { useEffect, useMemo, useRef, useState } from 'react';
import { EditorContent, useEditor, useEditorState, type Editor } from '@tiptap/react';
import { documentExtensions, safeDocumentImage } from './DocumentExtensions';
import { Button } from '../ui/Button';
import { Menu } from '../ui/Menu';
import { Picker } from '../ui/Picker';
import { Modal } from '../ui/Modal';
import { Input } from '../ui/Input';
import { Alert } from '../ui/Alert';
import { IconPlusOutline16 } from '../ui/icons';

function uploadImage(file: File, editor: Editor, failure: (message: string) => void) {
  if (!/^image\/(png|jpeg|gif|webp)$/.test(file.type) || file.size > 600 * 1024) {
    failure('Use a PNG, JPEG, GIF or WebP image up to 600 KiB, or insert an image URL.');
    return;
  }
  const reader = new FileReader();
  reader.onload = () => {
    if (
      !editor.isDestroyed &&
      typeof reader.result === 'string' &&
      safeDocumentImage(reader.result)
    )
      editor.chain().focus().setImage({ src: reader.result, alt: file.name }).run();
  };
  reader.onerror = () => failure('The image could not be read. Try again.');
  reader.readAsDataURL(file);
}
export default function RichDocumentEditor({
  source,
  editable,
  onChange,
  onSave,
}: {
  readonly source: string;
  readonly editable: boolean;
  readonly onChange: (markdown: string) => void;
  readonly onSave: () => void;
}) {
  const extensions = useMemo(() => documentExtensions(), []);
  const [insertOpen, setInsertOpen] = useState(false);
  const [imageOpen, setImageOpen] = useState(false);
  const [imageUrl, setImageUrl] = useState('');
  const [imageAlt, setImageAlt] = useState('');
  const [error, setError] = useState<string>();
  const upload = useRef<HTMLInputElement>(null);
  const editorRef = useRef<Editor | null>(null);
  const editor = useEditor({
    extensions,
    content: source,
    contentType: 'markdown',
    editable,
    editorProps: {
      attributes: {
        class: 'document-rich-text',
        role: 'textbox',
        'aria-label': 'Document content',
        'aria-multiline': 'true',
      },
      handlePaste: (_view, event) => {
        const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
          file.type.startsWith('image/'),
        );
        if (!files.length || !editable || !editorRef.current) return false;
        event.preventDefault();
        files.forEach((file) => uploadImage(file, editorRef.current!, setError));
        return true;
      },
      handleDrop: (_view, event) => {
        const files = Array.from(event.dataTransfer?.files ?? []).filter((file) =>
          file.type.startsWith('image/'),
        );
        if (!files.length || !editable || !editorRef.current) return false;
        event.preventDefault();
        files.forEach((file) => uploadImage(file, editorRef.current!, setError));
        return true;
      },
      handleKeyDown: (_view, event) => {
        if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') {
          event.preventDefault();
          onSave();
          return true;
        }
        return false;
      },
    },
    onUpdate: ({ editor: current }) => {
      const value = current.getMarkdown();
      setError(
        new TextEncoder().encode(value).length > 1024 * 1024
          ? 'This document exceeds 1 MiB. Remove an embedded image or shorten the content before saving.'
          : undefined,
      );
      onChange(value);
    },
  });
  useEffect(() => {
    editorRef.current = editor;
    return () => {
      editorRef.current = null;
    };
  }, [editor]);
  useEffect(() => {
    editor?.setEditable(editable, false);
  }, [editor, editable]);
  const selected = useEditorState({
    editor,
    selector: (context) => ({
      bold: context.editor?.isActive('bold') ?? false,
      italic: context.editor?.isActive('italic') ?? false,
      underline: context.editor?.isActive('underline') ?? false,
      heading: context.editor?.isActive('heading', { level: 1 })
        ? 'h1'
        : context.editor?.isActive('heading', { level: 2 })
          ? 'h2'
          : context.editor?.isActive('heading', { level: 3 })
            ? 'h3'
            : 'paragraph',
      font: String(context.editor?.getAttributes('documentFont').family ?? 'sans'),
      table: context.editor?.isActive('table') ?? false,
    }),
  });
  const insert = (kind: string) => {
    setInsertOpen(false);
    if (!editor || !editable) return;
    const chain = editor.chain().focus();
    switch (kind) {
      case 'table':
        chain.insertTable({ rows: 3, cols: 3, withHeaderRow: true }).run();
        break;
      case 'image':
        setImageOpen(true);
        break;
      case 'bullet':
        chain.toggleBulletList().run();
        break;
      case 'ordered':
        chain.toggleOrderedList().run();
        break;
      case 'checklist':
        chain.toggleTaskList().run();
        break;
      case 'code':
        chain.toggleCodeBlock().run();
        break;
      case 'divider':
        chain.setHorizontalRule().run();
        break;
      case 'quote':
        chain.toggleBlockquote().run();
        break;
      case 'date':
        chain
          .insertContent({ type: 'date', attrs: { value: new Date().toLocaleDateString('en-CA') } })
          .run();
        break;
      case 'details':
        chain
          .insertContent({
            type: 'details',
            attrs: { open: true },
            content: [
              { type: 'detailsSummary', content: [{ type: 'text', text: 'Details' }] },
              {
                type: 'detailsContent',
                content: [
                  { type: 'paragraph', content: [{ type: 'text', text: 'Add details here.' }] },
                ],
              },
            ],
          })
          .run();
        break;
      case 'mermaid':
        chain
          .insertContent({
            type: 'codeBlock',
            attrs: { language: 'mermaid' },
            content: [{ type: 'text', text: 'flowchart LR\n  A[Start] --> B[Finish]' }],
          })
          .run();
        break;
      case 'row':
        chain.addRowAfter().run();
        break;
      case 'column':
        chain.addColumnAfter().run();
        break;
      case 'remove-table':
        chain.deleteTable().run();
        break;
    }
  };
  return (
    <div className="document-editor">
      {editable ? (
        <div className="document-editor__toolbar" role="toolbar" aria-label="Document formatting">
          <Picker
            size="sm"
            label="Paragraph style"
            value={selected?.heading ?? 'paragraph'}
            options={[
              { value: 'paragraph', label: 'Text' },
              { value: 'h1', label: 'Heading 1' },
              { value: 'h2', label: 'Heading 2' },
              { value: 'h3', label: 'Heading 3' },
            ]}
            onChange={(value) =>
              value === 'paragraph'
                ? editor?.chain().focus().setParagraph().run()
                : editor
                    ?.chain()
                    .focus()
                    .setHeading({ level: Number(value.slice(1)) as 1 | 2 | 3 })
                    .run()
            }
          />
          <Picker
            size="sm"
            label="Font"
            value={selected?.font ?? 'sans'}
            options={[
              { value: 'sans', label: 'Sans' },
              { value: 'serif', label: 'Serif' },
              { value: 'mono', label: 'Mono' },
            ]}
            onChange={(value) =>
              editor?.chain().focus().setMark('documentFont', { family: value }).run()
            }
          />
          <Button
            size="sm"
            variant="ghost"
            aria-label="Bold"
            aria-pressed={selected?.bold}
            onClick={() => editor?.chain().focus().toggleBold().run()}
          >
            <strong>B</strong>
          </Button>
          <Button
            size="sm"
            variant="ghost"
            aria-label="Italic"
            aria-pressed={selected?.italic}
            onClick={() => editor?.chain().focus().toggleItalic().run()}
          >
            <em>I</em>
          </Button>
          <Button
            size="sm"
            variant="ghost"
            aria-label="Underline"
            aria-pressed={selected?.underline}
            onClick={() => editor?.chain().focus().toggleUnderline().run()}
          >
            <u>U</u>
          </Button>
          <Menu
            open={insertOpen}
            label="Insert content"
            anchor={
              <Button
                size="sm"
                variant="ghost"
                icon={<IconPlusOutline16 />}
                aria-label="Insert content"
                aria-haspopup="menu"
                aria-expanded={insertOpen}
                onClick={() => setInsertOpen(!insertOpen)}
              />
            }
            onClose={() => setInsertOpen(false)}
            onSelect={insert}
            items={[
              { id: 'table', label: 'Table' },
              { id: 'image', label: 'Image' },
              { id: 'bullet', label: 'Bullet list' },
              { id: 'ordered', label: 'Numbered list' },
              { id: 'checklist', label: 'Checklist' },
              { id: 'quote', label: 'Quote' },
              { id: 'code', label: 'Code block' },
              { id: 'mermaid', label: 'Mermaid diagram' },
              { id: 'details', label: 'Dropdown section' },
              { id: 'date', label: 'Date' },
              { id: 'divider', label: 'Divider' },
              ...(selected?.table
                ? [
                    { type: 'separator' as const, id: 'table-actions' },
                    { id: 'row', label: 'Add row' },
                    { id: 'column', label: 'Add column' },
                    { id: 'remove-table', label: 'Delete table', danger: true },
                  ]
                : []),
            ]}
          />
          <Button size="sm" variant="ghost" onClick={() => editor?.chain().focus().undo().run()}>
            Undo
          </Button>
          <Button size="sm" variant="ghost" onClick={() => editor?.chain().focus().redo().run()}>
            Redo
          </Button>
        </div>
      ) : null}
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
      <div className="document-editor__paper">
        <EditorContent editor={editor} />
      </div>
      <Modal
        open={imageOpen}
        title="Insert image"
        onClose={() => setImageOpen(false)}
        footer={
          <>
            <Button onClick={() => upload.current?.click()}>Upload image</Button>
            <Button
              variant="primary"
              disabled={!safeDocumentImage(imageUrl)}
              onClick={() => {
                if (safeDocumentImage(imageUrl))
                  editor?.chain().focus().setImage({ src: imageUrl, alt: imageAlt }).run();
                setImageOpen(false);
              }}
            >
              Insert image
            </Button>
          </>
        }
      >
        <label className="document-field">
          Image URL
          <Input
            value={imageUrl}
            onChange={(event) => setImageUrl(event.target.value)}
            placeholder="https://…"
          />
        </label>
        <label className="document-field">
          Description
          <Input value={imageAlt} onChange={(event) => setImageAlt(event.target.value)} />
        </label>
        <p>PNG, JPEG, GIF or WebP up to 600 KiB can also be pasted or dropped into the document.</p>
        <input
          ref={upload}
          type="file"
          hidden
          accept="image/png,image/jpeg,image/gif,image/webp"
          onChange={(event) => {
            const file = event.target.files?.[0];
            if (file && editor) {
              uploadImage(file, editor, setError);
              setImageOpen(false);
            }
            event.target.value = '';
          }}
        />
      </Modal>
    </div>
  );
}
