import { useId, useState } from 'react';

import { useI18n } from '../i18n';
import type { ContextUsage } from '../lib/types';

import './ContextUsageIndicator.css';

function compactTokens(tokens: number): string {
  const divisor = tokens >= 1_000_000 ? 1_000_000 : tokens >= 1_000 ? 1_000 : 1;
  const value = Number((tokens / divisor).toFixed(1));
  return `${value}${divisor === 1_000_000 ? 'M' : divisor === 1_000 ? 'k' : ''}`;
}

export function ContextUsageIndicator({
  usage,
  contextWindow,
}: {
  usage: ContextUsage | null;
  contextWindow: number;
}) {
  const { t, locale } = useI18n();
  const tooltipId = useId();
  const [dismissed, setDismissed] = useState(false);
  const limit = usage?.tokenLimit ?? contextWindow;
  const current = usage?.currentTokens;
  const ratio = current === undefined ? 0 : Math.min(1, Math.max(0, current / limit));
  const percent = current === undefined ? null : Math.round((current / limit) * 1_000) / 10;
  const compact = `${current === undefined ? '—' : compactTokens(current)} / ${compactTokens(limit)}`;
  const exact = `${current === undefined ? '—' : current.toLocaleString(locale)} / ${limit.toLocaleString(locale)}`;
  const description =
    percent === null ? t('chat.contextWaiting') : t('chat.contextUsed', { percent, usage: exact });

  return (
    <button
      type="button"
      className={`context-usage ${ratio >= 0.9 ? 'context-usage-high' : ''}`}
      aria-label={t('chat.contextWindow')}
      aria-describedby={tooltipId}
      data-dismissed={dismissed || undefined}
      onClick={() => setDismissed(false)}
      onMouseEnter={() => setDismissed(false)}
      onFocus={() => setDismissed(false)}
      onKeyDown={(event) => {
        if (event.key === 'Escape') setDismissed(true);
      }}
    >
      <svg
        width="18"
        height="18"
        viewBox="0 0 20 20"
        role={current === undefined ? undefined : 'meter'}
        aria-hidden={current === undefined ? true : undefined}
        aria-label={t('chat.contextWindow')}
        aria-valuemin={current === undefined ? undefined : 0}
        aria-valuemax={current === undefined ? undefined : limit}
        aria-valuenow={current === undefined ? undefined : Math.min(current, limit)}
        aria-valuetext={current === undefined ? undefined : description}
      >
        <circle className="context-usage-track" cx="10" cy="10" r="7" />
        <circle
          className="context-usage-fill"
          cx="10"
          cy="10"
          r="7"
          pathLength="100"
          strokeDasharray={`${ratio * 100} 100`}
          transform="rotate(-90 10 10)"
        />
      </svg>
      <span className="context-usage-tooltip" id={tooltipId} role="tooltip">
        <strong>{t('chat.contextWindow')}</strong>
        <span className="context-usage-count">{compact}</span>
        <span>{description}</span>
      </span>
    </button>
  );
}
