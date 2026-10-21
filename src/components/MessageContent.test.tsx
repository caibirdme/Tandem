import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import type { SelectedSkill } from '../lib/types';
import { SentMessageContent } from './MessageContent';

const review: SelectedSkill = { name: 'review', path: '/private/project/skills/review/SKILL.md' };
const reviewDocs: SelectedSkill = {
  name: 'review-docs',
  path: '/private/project/skills/review-docs/SKILL.md',
};

function labels(markup: string): string[] {
  return [...markup.matchAll(/<span class="sent-skill-mention-name">(.*?)<\/span>/g)].map(
    (match) => match[1] ?? '',
  );
}

describe('sent skill mentions', () => {
  it('keeps ordinary dollar expressions plain unless a skill was explicitly selected', () => {
    const content = 'Use $review or $audit; the budget is $25.';
    const plain = renderToStaticMarkup(<SentMessageContent content={content} />);
    expect(plain).toBe(`<p class="sent-message-content">${content}</p>`);

    const selected = renderToStaticMarkup(
      <SentMessageContent content={content} selectedSkills={[review]} />,
    );
    expect(labels(selected)).toEqual(['$review']);
    expect(selected).toContain('$audit; the budget is $25.');
    expect(selected).not.toContain(review.path);
    expect(selected).toContain('aria-hidden="true"');
  });

  it('preserves mixed text and whitespace around repeated mentions and distinct names', () => {
    const content = 'First  $review-docs, then $review;\n\n\tRepeat [$review-docs].';
    const markup = renderToStaticMarkup(
      <SentMessageContent content={content} selectedSkills={[review, reviewDocs]} />,
    );
    expect(labels(markup)).toEqual(['$review-docs', '$review', '$review-docs']);
    const text = markup.replace(/<svg\b[\s\S]*?<\/svg>/g, '').replace(/<[^>]*>/g, '');
    expect(text).toBe(content);
  });

  it.each([
    { content: 'x$review \\$review $reviewing $review.v2', expected: [] },
    {
      content: '$review ($review) [$review] {$review}',
      expected: Array<string>(4).fill('$review'),
    },
  ])('requires exact mention boundaries in $content', ({ content, expected }) => {
    const markup = renderToStaticMarkup(
      <SentMessageContent content={content} selectedSkills={[review]} />,
    );
    expect(labels(markup)).toEqual(expected);
  });

  it('renders each occurrence once when selected identities share a name', () => {
    const markup = renderToStaticMarkup(
      <SentMessageContent
        content="$review and $review"
        selectedSkills={[review, review, { ...review, path: '/another/skills/review/SKILL.md' }]}
      />,
    );
    expect(labels(markup)).toEqual(['$review', '$review']);
    expect(markup.match(/class="sent-skill-mention"/g)).toHaveLength(2);
  });

  it('escapes surrounding user text while rendering a selected skill', () => {
    const markup = renderToStaticMarkup(
      <SentMessageContent
        content={'<script>alert("x")</script> $review & check'}
        selectedSkills={[review]}
      />,
    );
    expect(markup).not.toContain('<script>');
    expect(markup).toContain('&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt;');
    expect(markup).toContain('&amp; check');
    expect(labels(markup)).toEqual(['$review']);
  });
});
