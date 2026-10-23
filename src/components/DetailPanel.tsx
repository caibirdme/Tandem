import * as Dialog from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useSyncExternalStore } from 'react';
import type { ReactNode, RefObject } from 'react';

import { useI18n } from '../i18n';

import './DetailPanel.css';

const narrowQuery = '(max-width: 1100px)';
function subscribeToViewport(callback: () => void) {
  const query = window.matchMedia(narrowQuery);
  query.addEventListener('change', callback);
  return () => query.removeEventListener('change', callback);
}
function isNarrowViewport() {
  return window.matchMedia(narrowQuery).matches;
}
function serverViewport() {
  return false;
}
export function useNarrowDetailPanel() {
  return useSyncExternalStore(subscribeToViewport, isNarrowViewport, serverViewport);
}
function restorePanelFocus(
  ref: RefObject<HTMLElement | null> | undefined,
  opener: HTMLElement | null,
) {
  const target = ref?.current ?? opener;
  if (target?.isConnected) target.focus({ preventScroll: true });
}

/** Content-independent shell: panel kinds own their data and rendering. */
export function DetailPanel({
  id,
  title,
  children,
  onClose,
  returnFocusRef,
  bodyClassName,
}: {
  id: string;
  title: string;
  children: ReactNode;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null> | undefined;
  bodyClassName?: string;
}) {
  const { t } = useI18n();
  const narrow = useNarrowDetailPanel();
  const closeButton = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => restorePanelFocus(returnFocusRef, opener);
  }, [returnFocusRef]);
  useLayoutEffect(() => {
    closeButton.current?.focus({ preventScroll: true });
  }, [narrow]);
  useEffect(() => {
    if (narrow) return;
    function keydown(event: KeyboardEvent) {
      if (
        event.key === 'Escape' &&
        !event.defaultPrevented &&
        event.target instanceof Node &&
        panel.current?.contains(event.target)
      ) {
        event.preventDefault();
        onClose();
      }
    }
    document.addEventListener('keydown', keydown);
    return () => document.removeEventListener('keydown', keydown);
  }, [narrow, onClose]);

  const content = (
    <section ref={panel} id={id} className="detail-panel" aria-labelledby={`${id}-title`}>
      <header className="detail-panel-header">
        {narrow ? (
          <Dialog.Title id={`${id}-title`} className="detail-panel-title">
            {title}
          </Dialog.Title>
        ) : (
          <h2 id={`${id}-title`} className="detail-panel-title">
            {title}
          </h2>
        )}
        <button
          type="button"
          ref={closeButton}
          className="icon-button"
          aria-label={t('panel.close')}
          onClick={onClose}
        >
          <X size={18} />
        </button>
      </header>
      <div className={`detail-panel-body ${bodyClassName ?? ''}`}>{children}</div>
    </section>
  );
  if (!narrow) return content;
  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Dialog.Content
        asChild
        aria-describedby={undefined}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          closeButton.current?.focus();
        }}
        onCloseAutoFocus={(event) => event.preventDefault()}
        onInteractOutside={(event) => event.preventDefault()}
      >
        {content}
      </Dialog.Content>
    </Dialog.Root>
  );
}
