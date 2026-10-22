import * as Dialog from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { CSSProperties, ReactNode, RefObject } from 'react';

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
export function clampPanelWidth(width: number, maximum = 55) {
  return Math.min(maximum, Math.max(28, width));
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
}: {
  id: string;
  title: string;
  children: ReactNode;
  onClose: () => void;
  returnFocusRef?: RefObject<HTMLElement | null> | undefined;
}) {
  const { t } = useI18n();
  const narrow = useSyncExternalStore(subscribeToViewport, isNarrowViewport, serverViewport);
  const [width, setWidth] = useState(42);
  const [maximum, setMaximum] = useState(55);
  const effectiveWidth = clampPanelWidth(width, maximum);
  const closeButton = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLElement>(null);
  const drag = useRef<{ x: number; width: number; available: number } | null>(null);
  useLayoutEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => restorePanelFocus(returnFocusRef, opener);
  }, [returnFocusRef]);
  useLayoutEffect(() => {
    closeButton.current?.focus({ preventScroll: true });
  }, [narrow]);
  useEffect(() => {
    if (narrow) return;
    const parent = panel.current?.parentElement;
    if (!parent) return;
    function measure() {
      if (!parent) return;
      const sidebar = parent.querySelector('.sidebar')?.getBoundingClientRect().width ?? 0;
      setMaximum(
        Math.max(
          28,
          Math.min(55, ((parent.clientWidth - sidebar - 360) / parent.clientWidth) * 100),
        ),
      );
    }
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(parent);
    const mutation = new MutationObserver(measure);
    mutation.observe(parent, { childList: true, attributes: true, attributeFilter: ['class'] });
    return () => {
      observer.disconnect();
      mutation.disconnect();
    };
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
    <section
      ref={panel}
      id={id}
      className="detail-panel"
      aria-labelledby={`${id}-title`}
      style={{ '--detail-panel-width': `${String(effectiveWidth)}%` } as CSSProperties}
    >
      {!narrow && (
        <input
          type="range"
          min={28}
          max={maximum}
          step="any"
          value={effectiveWidth}
          onChange={(event) => setWidth(Number(event.currentTarget.value))}
          className="detail-panel-resize"
          aria-label={t('panel.resize')}
          aria-controls={id}
          aria-valuemin={28}
          aria-valuemax={maximum}
          aria-valuenow={effectiveWidth}
          onKeyDown={(event) => {
            if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
            event.preventDefault();
            setWidth(
              clampPanelWidth(
                event.key === 'Home'
                  ? 28
                  : event.key === 'End'
                    ? maximum
                    : effectiveWidth + (event.key === 'ArrowLeft' ? 2 : -2),
                maximum,
              ),
            );
          }}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            event.currentTarget.focus();
            const available = panel.current?.parentElement?.clientWidth;
            if (!available) return;
            drag.current = { x: event.clientX, width: effectiveWidth, available };
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => {
            const start = drag.current;
            if (start)
              setWidth(
                clampPanelWidth(
                  start.width + ((start.x - event.clientX) / start.available) * 100,
                  maximum,
                ),
              );
          }}
          onPointerUp={(event) => {
            drag.current = null;
            if (event.currentTarget.hasPointerCapture(event.pointerId))
              event.currentTarget.releasePointerCapture(event.pointerId);
          }}
          onPointerCancel={() => {
            drag.current = null;
          }}
          onLostPointerCapture={() => {
            drag.current = null;
          }}
        />
      )}
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
      <div className="detail-panel-body">{children}</div>
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
