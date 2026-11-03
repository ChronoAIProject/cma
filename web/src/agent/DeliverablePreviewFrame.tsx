import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ComponentProps,
} from 'react';
import { AppFrame } from '../shell/AppFrame';
import { DeliverablePreviewContext, type DeliverableSelection } from './previewSelection';
import { DeliverablePreview } from './DeliverablePreview';
import './DeliverablePreview.css';

export function DeliverablePreviewFrame({
  previewScope,
  onPreviewOpen,
  ...props
}: ComponentProps<typeof AppFrame> & {
  readonly previewScope?: string;
  readonly onPreviewOpen: () => void;
}) {
  const [selected, setSelected] = useState<DeliverableSelection>();
  const [opening, setOpening] = useState(0);
  const returnFocus = useRef<HTMLElement | undefined>(undefined);
  useLayoutEffect(() => {
    if (selected || !returnFocus.current) return;
    returnFocus.current.focus({ preventScroll: true });
    returnFocus.current = undefined;
  }, [selected]);
  const [scope, setScope] = useState(previewScope);
  const [detailsOpen, setDetailsOpen] = useState(props.detailsOpen);
  if (scope !== previewScope || detailsOpen !== props.detailsOpen) {
    setScope(previewScope);
    setDetailsOpen(props.detailsOpen);
    if (scope !== previewScope || props.detailsOpen) setSelected(undefined);
  }
  const invalidate = useCallback((access: DeliverableSelection['access']) => {
    setSelected((current) => (current?.access === access ? undefined : current));
  }, []);
  const { onDetailsClose } = props;
  const close = useCallback(() => {
    returnFocus.current = selected?.trigger;
    setSelected(undefined);
    onDetailsClose?.();
  }, [selected, onDetailsClose]);
  useEffect(() => {
    if (!selected) return;
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      event.preventDefault();
      close();
    };
    document.addEventListener('keydown', escape);
    return () => document.removeEventListener('keydown', escape);
  }, [selected, close]);
  return (
    <DeliverablePreviewContext.Provider
      value={{
        selected,
        invalidate,
        open: (value) => {
          onPreviewOpen();
          setOpening((value) => value + 1);
          setSelected(value);
        },
      }}
    >
      <AppFrame
        {...props}
        detailsOpen={props.detailsOpen || selected !== undefined}
        onDetailsClose={close}
        details={
          <>
            <div
              className="delivery-preview-retained"
              hidden={selected !== undefined}
              inert={selected !== undefined || undefined}
            >
              {props.details}
            </div>
            {selected ? (
              <DeliverablePreview key={opening} selected={selected} onClose={close} />
            ) : null}
          </>
        }
      />
    </DeliverablePreviewContext.Provider>
  );
}
