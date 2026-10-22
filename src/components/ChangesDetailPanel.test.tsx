import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { createReviewController } from '../lib/reviews';
import type { ReviewState } from '../lib/reviews';
import type { ReviewFile, ReviewSummary, ReviewUnavailableCode } from '../lib/reviewTypes';
import { ChangesDetailPanel } from './ChangesDetailPanel';

const controller = createReviewController({
  summary: async () => ({ summary: null, unavailable: null, turns: [] }),
  files: async () => ({ files: [], nextCursor: null }),
  file: async () => {
    throw new Error('No file selected');
  },
});
const summary: ReviewSummary = {
  reviewId: 'review',
  revision: 'one',
  workspacePath: '/project',
  repositoryPath: '/project',
  selection: { kind: 'workspace', comparison: 'net' },
  capturedAt: 1000,
  filesTotal: 0,
  additions: 0,
  deletions: 0,
  complete: true,
  excludedFiles: 0,
};
function render(changes: Partial<ReviewState>) {
  return renderToStaticMarkup(
    <ChangesDetailPanel
      id="review"
      controller={controller}
      state={{ ...controller.getSnapshot(), ...changes }}
      messages={[]}
      running={false}
      onClose={() => undefined}
    />,
  );
}
afterEach(() => setLocale('en'));

describe('review availability and capture coverage', () => {
  it.each<ReviewUnavailableCode>([
    'not_repository',
    'git_unavailable',
    'remote_unavailable',
    'baseline_unavailable',
    'capturing',
    'inspection_failed',
  ])('never describes %s as an empty comparison', (code) => {
    const html = render({
      overview: { summary: null, unavailable: { code, message: 'Diagnostic detail' }, turns: [] },
    });
    expect(html).toContain('Diagnostic detail');
    expect(html).not.toContain('No changes in this comparison');
    expect(html).not.toContain('review-file-list');
  });

  it('distinguishes a complete empty review from excluded files in a partial capture', () => {
    const empty = render({ overview: { summary, unavailable: null, turns: [] } });
    expect(empty).toContain('No changes in this comparison');
    expect(empty).not.toContain('This review is incomplete');
    const partial = render({
      overview: {
        summary: { ...summary, complete: false, excludedFiles: 3 },
        unavailable: null,
        turns: [],
      },
    });
    expect(partial).toContain('This review is incomplete');
    expect(partial).toContain('3 files were excluded');
  });

  it('shows unknown lazy totals without implying an incomplete capture or zero changes', () => {
    const html = render({
      overview: {
        summary: { ...summary, filesTotal: 11, additions: null, deletions: null },
        unavailable: null,
        turns: [],
      },
    });
    expect(html).toContain('+?');
    expect(html).toContain('−?');
    expect(html).not.toContain('This review is incomplete');
    expect(html).not.toContain('No changes in this comparison');
  });

  it('shows saved mode-only evidence and explains attribution in the selected language', () => {
    setLocale('zh-CN');
    const file: ReviewFile = {
      id: 'file',
      path: 'scripts/run.sh',
      oldPath: null,
      oldMode: '100644',
      newMode: '100755',
      status: 'modified',
      indexStatus: '.',
      worktreeStatus: 'M',
      contentKind: 'text',
      additions: 0,
      deletions: 0,
    };
    const html = render({
      selection: { kind: 'session' },
      overview: { summary: { ...summary, filesTotal: 1 }, unavailable: null, turns: [] },
      files: [file],
      selectedFileId: file.id,
      diff: {
        reviewId: summary.reviewId,
        revision: summary.revision,
        file,
        patch: null,
        hunks: [],
      },
    });
    expect(html).toContain('100644 → 100755');
    expect(html).toContain('此比较中没有文本差异');
    expect(html).toContain('其他会话或工具也可能编辑了这些文件');
    expect(html).toContain('只读审查');
  });
});
