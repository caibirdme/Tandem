import { useMemo, useState } from 'react';

import { useI18n } from '../i18n';
import { splitReviewRows, unifiedReviewRows } from '../lib/reviewDiff';
import type { ReviewFileDiff, ReviewLine } from '../lib/reviewTypes';

const PAGE_ROWS = 500;
const markers = { context: ' ', addition: '+', deletion: '−', note: '\\' } as const;

function SplitCell({ line, side }: { line: ReviewLine | null; side: 'before' | 'after' }) {
  const { t } = useI18n();
  return (
    <div className={`review-split-cell ${line?.kind ?? 'empty'}`}>
      <span className="review-line-number" aria-hidden="true">
        {side === 'before' ? line?.oldLine : line?.newLine}
      </span>
      <span className="review-line-marker" aria-hidden="true">
        {line ? markers[line.kind] : ''}
      </span>
      <code>
        {line && (
          <span className="review-sr-only">
            {t(`review.line.${line.kind}`, {
              old: line.oldLine ?? '—',
              new: line.newLine ?? '—',
            })}{' '}
          </span>
        )}
        {line?.text ?? ''}
      </code>
    </div>
  );
}

export function ReviewDiff({
  diff,
  split,
  wrap,
}: {
  diff: ReviewFileDiff;
  split: boolean;
  wrap: boolean;
}) {
  const { t } = useI18n();
  const [limit, setLimit] = useState(PAGE_ROWS);
  const unified = useMemo(() => unifiedReviewRows(diff.hunks), [diff.hunks]);
  const paired = useMemo(() => splitReviewRows(diff.hunks), [diff.hunks]);
  const total = split ? paired.length : unified.length;
  const shown = Math.min(limit, total);
  return (
    <section className={`review-diff ${wrap ? 'review-wrap' : ''}`} aria-label={t('review.diff')}>
      {split && (
        <div className="review-split-labels">
          <span>{t('review.before')}</span>
          <span>{t('review.after')}</span>
        </div>
      )}
      {split
        ? paired.slice(0, limit).map((row) =>
            row.kind === 'header' ? (
              <div className="review-hunk-header" key={row.key} title={row.hunk.header}>
                {t('review.hunk', { old: row.hunk.oldStart, new: row.hunk.newStart })}
              </div>
            ) : (
              <div className="review-split-row" key={row.key}>
                <SplitCell line={row.before} side="before" />
                <SplitCell line={row.after} side="after" />
              </div>
            ),
          )
        : unified.slice(0, limit).map((row) =>
            row.kind === 'header' ? (
              <div className="review-hunk-header" key={row.key} title={row.hunk.header}>
                {t('review.hunk', { old: row.hunk.oldStart, new: row.hunk.newStart })}
              </div>
            ) : (
              <div className={`review-diff-line ${row.line.kind}`} key={row.key}>
                <span className="review-line-number" aria-hidden="true">
                  {row.line.oldLine}
                </span>
                <span className="review-line-number" aria-hidden="true">
                  {row.line.newLine}
                </span>
                <span className="review-line-marker" aria-hidden="true">
                  {markers[row.line.kind]}
                </span>
                <code>
                  <span className="review-sr-only">
                    {t(`review.line.${row.line.kind}`, {
                      old: row.line.oldLine ?? '—',
                      new: row.line.newLine ?? '—',
                    })}{' '}
                  </span>
                  {row.line.text}
                </code>
              </div>
            ),
          )}
      {shown < total && (
        <div className="review-more-lines">
          <span>{t('review.linesShown', { shown, total })}</span>
          <button
            type="button"
            className="button"
            onClick={() => setLimit((value) => value + PAGE_ROWS)}
          >
            {t('review.moreLines', { count: Math.min(PAGE_ROWS, total - shown) })}
          </button>
        </div>
      )}
    </section>
  );
}
