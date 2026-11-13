import { useId, useState } from 'react';
import { Modal } from '../ui/Modal';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import {
  IconBranchOutline16,
  IconCloseOutline16,
  IconGithub16,
  IconRepositoryOutline16,
} from '../ui/icons';
import { parseRepository, validRepositoryRef } from './repository';
import './RepositoryDialog.css';

export function RepositoryDialog({
  url,
  reference,
  onSave,
  onClose,
}: {
  readonly url: string;
  readonly reference: string;
  readonly onSave: (url: string, reference: string) => void;
  readonly onClose: () => void;
}) {
  const [repository, setRepository] = useState(url);
  const [ref, setRef] = useState(reference);
  const [repositoryTouched, setRepositoryTouched] = useState(false);
  const id = useId();
  const canonical = parseRepository(repository);
  const invalidRepository = repositoryTouched && canonical === undefined;
  const invalidRef = ref !== '' && !validRepositoryRef(ref);
  const valid = canonical !== undefined && !invalidRef;
  return (
    <Modal open onClose={onClose} title="Add git repo" headless presentation="repository">
      <form
        className="repository-dialog"
        noValidate
        onKeyDown={(event) => {
          if (
            event.key === 'Enter' &&
            event.target instanceof HTMLInputElement &&
            !event.nativeEvent.isComposing &&
            event.nativeEvent.keyCode !== 229
          ) {
            event.preventDefault();
            event.currentTarget.requestSubmit();
          }
        }}
        onSubmit={(event) => {
          event.preventDefault();
          event.stopPropagation();
          setRepositoryTouched(true);
          if (!valid) return;
          onSave(canonical, ref);
          onClose();
        }}
      >
        <div className="repository-dialog__header">
          <span className="repository-dialog__icon" aria-hidden="true">
            <IconRepositoryOutline16 size={24} />
          </span>
          <h2>Add git repo</h2>
          <p>Your Agent clones it onto the cloud computer before it starts.</p>
        </div>
        <div className="repository-dialog__field">
          <label htmlFor={`${id}-url`}>GitHub repository</label>
          <Input
            id={`${id}-url`}
            aria-required="true"
            aria-invalid={invalidRepository}
            aria-describedby={invalidRepository ? `${id}-repository-error` : undefined}
            onBlur={() => {
              if (repository.trim()) setRepositoryTouched(true);
            }}
            icon={<IconGithub16 />}
            placement="repository-dialog__input"
            placeholder="https://github.com/owner/repository"
            value={repository}
            onChange={(event) => setRepository(event.target.value)}
          />
          {invalidRepository ? (
            <p id={`${id}-repository-error`} role="alert">
              Enter owner/repository or a github.com repository URL.
            </p>
          ) : null}
        </div>
        <div className="repository-dialog__field">
          <div>
            <label htmlFor={`${id}-ref`}>Repository ref</label>{' '}
            <span className="repository-dialog__optional">Optional</span>
          </div>
          <Input
            id={`${id}-ref`}
            icon={<IconBranchOutline16 />}
            placement="repository-dialog__input"
            data-repository-ref
            placeholder="Default branch"
            value={ref}
            aria-invalid={invalidRef}
            aria-describedby={invalidRef ? `${id}-error` : `${id}-helper`}
            onChange={(event) => setRef(event.target.value)}
          />
          {invalidRef ? (
            <p id={`${id}-error`} role="alert">
              Enter a valid repository ref.
            </p>
          ) : null}
        </div>
        <p id={`${id}-helper`} className="repository-dialog__helper">
          The requested ref is verified during preparation.
        </p>
        <div className="repository-dialog__footer">
          <Button variant="outline" placement="repository-dialog__action" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            placement="repository-dialog__save"
            disabled={!valid}
          >
            Add repo
          </Button>
        </div>
        <Button
          size="sm"
          placement="repository-dialog__close"
          aria-label="Close repository dialog"
          icon={<IconCloseOutline16 />}
          onClick={onClose}
        />
      </form>
    </Modal>
  );
}
