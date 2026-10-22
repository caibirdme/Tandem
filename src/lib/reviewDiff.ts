import type { ReviewHunk, ReviewLine } from './reviewTypes';

export type UnifiedReviewRow =
  | { kind: 'header'; key: string; hunk: ReviewHunk }
  | { kind: 'line'; key: string; line: ReviewLine };
export type SplitReviewRow =
  | { kind: 'header'; key: string; hunk: ReviewHunk }
  | { kind: 'line'; key: string; before: ReviewLine | null; after: ReviewLine | null };

export function unifiedReviewRows(hunks: ReviewHunk[]): UnifiedReviewRow[] {
  return hunks.flatMap((hunk, hunkIndex) => [
    { kind: 'header' as const, key: `h${hunkIndex}`, hunk },
    ...hunk.lines.map((line, lineIndex) => ({
      kind: 'line' as const,
      key: `h${hunkIndex}:l${lineIndex}`,
      line,
    })),
  ]);
}

/** Align already-computed change runs for presentation; Rust determines the diff. */
export function splitReviewRows(hunks: ReviewHunk[]): SplitReviewRow[] {
  const rows: SplitReviewRow[] = [];
  for (const [hunkIndex, hunk] of hunks.entries()) {
    rows.push({ kind: 'header', key: `h${hunkIndex}`, hunk });
    let index = 0;
    while (index < hunk.lines.length) {
      const line = hunk.lines[index];
      if (!line) break;
      if (line.kind === 'context' || line.kind === 'note') {
        rows.push({ kind: 'line', key: `h${hunkIndex}:l${index}`, before: line, after: line });
        index += 1;
        continue;
      }
      const start = index;
      const before: ReviewLine[] = [];
      const after: ReviewLine[] = [];
      while (index < hunk.lines.length) {
        const changed = hunk.lines[index];
        if (!changed || (changed.kind !== 'deletion' && changed.kind !== 'addition')) break;
        (changed.kind === 'deletion' ? before : after).push(changed);
        index += 1;
      }
      for (let row = 0; row < Math.max(before.length, after.length); row += 1) {
        rows.push({
          kind: 'line',
          key: `h${hunkIndex}:l${start}:${row}`,
          before: before[row] ?? null,
          after: after[row] ?? null,
        });
      }
    }
  }
  return rows;
}
