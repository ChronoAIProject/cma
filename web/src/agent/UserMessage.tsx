import { IconPaperclipOutline16 } from '../ui/icons';
import { splitAttachmentMessage } from './attachments';
import './ComposerAttachments.css';

export function UserMessage({ content }: { readonly content: string }) {
  const message = splitAttachmentMessage(content);
  if (message === undefined) return <p>{content}</p>;
  return (
    <>
      <p>{message.text}</p>
      <ul className="message-attachments" aria-label="Attached files">
        {message.files.map((file, index) => (
          <li key={`${String(index)}:${file.path}`} title={file.path}>
            <IconPaperclipOutline16 />
            <span>{file.name}</span>
            <small>
              {file.size_bytes < 1024
                ? `${String(file.size_bytes)} B`
                : `${(file.size_bytes / 1024).toFixed(0)} KiB`}
            </small>
          </li>
        ))}
      </ul>
    </>
  );
}
