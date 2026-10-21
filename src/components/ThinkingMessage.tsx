import { ChevronRight, Lightbulb } from 'lucide-react';
import { memo, useLayoutEffect, useRef, useState } from 'react';

import { useI18n } from '../i18n';
import type { ActivityStatus } from '../lib/types';

import './ThinkingMessage.css';

export const ThinkingMessage = memo(function ThinkingMessage({
  content,
  status,
  defaultExpanded = false,
  onExpandedChange,
}: {
  content: string;
  status: ActivityStatus;
  defaultExpanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
}) {
  const { t } = useI18n();
  const [expanded, setExpanded] = useState(defaultExpanded);
  const viewport = useRef<HTMLTextAreaElement>(null);
  const followLatest = useRef(true);
  const readingPosition = useRef(0);
  const running = status === 'running';

  useLayoutEffect(() => {
    if (expanded && viewport.current)
      viewport.current.scrollTop = followLatest.current
        ? viewport.current.scrollHeight
        : readingPosition.current;
  }, [content, expanded]);

  return (
    <details
      className={`thinking-message ${status}`}
      open={expanded}
      aria-busy={running}
      onToggle={(event) => {
        const open = event.currentTarget.open;
        setExpanded(open);
        onExpandedChange?.(open);
      }}
    >
      <summary>
        {running ? (
          <span className="thinking-message-dots" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
        ) : (
          <Lightbulb size={13} aria-hidden="true" />
        )}
        <span>
          {t(
            running
              ? 'chat.thinking'
              : status === 'completed'
                ? 'chat.thoughts'
                : 'chat.thinkingStopped',
          )}
        </span>
        <ChevronRight className="thinking-message-chevron" size={13} aria-hidden="true" />
      </summary>
      <textarea
        className="thinking-message-content"
        ref={viewport}
        aria-label={t('chat.thinkingContent')}
        readOnly
        rows={3}
        spellCheck={false}
        value={content}
        placeholder={t(running ? 'chat.thinkingWaiting' : 'chat.thinkingUnavailable')}
        onScroll={(event) => {
          if (!expanded) return;
          const box = event.currentTarget;
          readingPosition.current = box.scrollTop;
          followLatest.current = box.scrollHeight - box.clientHeight - box.scrollTop < 12;
        }}
      />
    </details>
  );
});
