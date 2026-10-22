import { describe, expect, it } from 'vitest';

import { splitReviewRows, unifiedReviewRows } from './reviewDiff';
import type { ReviewHunk } from './reviewTypes';

const hunks: ReviewHunk[] = [
  {
    oldStart: 4,
    newStart: 4,
    header: '@@ -4,3 +4,2 @@',
    lines: [
      { kind: 'context', oldLine: 4, newLine: 4, text: 'shared' },
      { kind: 'deletion', oldLine: 5, newLine: null, text: 'old first' },
      { kind: 'deletion', oldLine: 6, newLine: null, text: 'old second' },
      { kind: 'addition', oldLine: null, newLine: 5, text: 'new first' },
      { kind: 'note', oldLine: null, newLine: null, text: 'No newline at end of file' },
    ],
  },
];

describe('diff presentation rows', () => {
  it('retains source order and line numbers in unified view', () => {
    const rows = unifiedReviewRows(hunks);
    expect(rows.filter((row) => row.kind === 'line').map((row) => row.line)).toEqual(
      hunks[0]?.lines,
    );
  });

  it('aligns replacements with empty cells without dropping extra deleted lines or notes', () => {
    const rows = splitReviewRows(hunks).filter((row) => row.kind === 'line');
    expect(rows.map((row) => [row.before?.text ?? null, row.after?.text ?? null])).toEqual([
      ['shared', 'shared'],
      ['old first', 'new first'],
      ['old second', null],
      ['No newline at end of file', 'No newline at end of file'],
    ]);
    expect(rows[1]?.before?.oldLine).toBe(5);
    expect(rows[1]?.after?.newLine).toBe(5);
  });
});
