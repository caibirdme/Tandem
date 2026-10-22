import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { ReviewFileDiff } from '../lib/reviewTypes';
import { ReviewDiff } from './ReviewDiff';

const diff: ReviewFileDiff = {
  reviewId: 'review',
  revision: 'one',
  patch: null,
  file: {
    id: 'file',
    path: 'src/page.html',
    oldPath: null,
    oldMode: '100644',
    newMode: '100644',
    status: 'modified',
    indexStatus: '.',
    worktreeStatus: 'M',
    contentKind: 'text',
    additions: 1,
    deletions: 1,
  },
  hunks: [
    {
      oldStart: 1,
      newStart: 1,
      header: '@@ -1 +1 @@',
      lines: [
        { kind: 'deletion', oldLine: 1, newLine: null, text: 'old <script>alert(1)</script>' },
        { kind: 'addition', oldLine: null, newLine: 1, text: 'new <strong>title</strong>' },
      ],
    },
  ],
};

afterEach(() => setLocale('en'));

describe('read-only diff rendering', () => {
  it('escapes source markup and presents addition/deletion markers and line numbers', () => {
    const html = renderToStaticMarkup(<ReviewDiff diff={diff} split={false} wrap={false} />);
    expect(html).toContain('aria-label="File changes"');
    expect(html).toContain('old &lt;script&gt;alert(1)&lt;/script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).toContain('review-diff-line deletion');
    expect(html).toContain('review-diff-line addition');
    expect(html).toContain('>1</span>');
    expect(html).not.toContain('contenteditable');
  });

  it('labels both sides in the selected language', () => {
    setLocale('zh-CN');
    const html = renderToStaticMarkup(<ReviewDiff diff={diff} split wrap />);
    expect(html).toContain('修改前');
    expect(html).toContain('修改后');
    expect(html).toContain('review-wrap');
    expect(html).toContain('old &lt;script&gt;');
    expect(html).toContain('new &lt;strong&gt;');
  });

  it('bounds the initial rendered rows and leaves further changes accessible', () => {
    const many: ReviewFileDiff = {
      ...diff,
      hunks: [
        {
          oldStart: 1,
          newStart: 1,
          header: '',
          lines: Array.from({ length: 800 }, (_, index) => ({
            kind: 'addition',
            oldLine: null,
            newLine: index + 1,
            text: `line ${index + 1}`,
          })),
        },
      ],
    };
    const html = renderToStaticMarkup(<ReviewDiff diff={many} split={false} wrap={false} />);
    expect(html).toContain('line 499');
    expect(html).not.toContain('line 800');
    expect(html).toContain('Show next 301 lines');
  });
});
