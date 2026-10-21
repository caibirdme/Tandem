import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { ThinkingMessage } from './ThinkingMessage';

afterEach(() => setLocale('en'));

describe('thinking disclosure', () => {
  it('starts collapsed with a live status and an accessible three-row text viewport', () => {
    const content =
      'Inspect the files.\nCompare the interfaces.\nCheck the call sites.\nReview failures.';
    const markup = renderToStaticMarkup(<ThinkingMessage content={content} status="running" />);
    expect(markup).toContain('class="thinking-message running" aria-busy="true"');
    expect(markup).not.toMatch(/<details[^>]*\bopen(?:=|\s|>)/);
    expect(markup).toContain('class="thinking-message-dots" aria-hidden="true"');
    expect(markup).toContain('>Thinking</span>');
    expect(markup).toMatch(
      /<textarea[^>]*aria-label="Thinking content"[^>]*readOnly=""[^>]*rows="3"/,
    );
    expect(markup).toContain(content);
  });

  it('can inherit an expanded waiting panel when the first reasoning text arrives', () => {
    const markup = renderToStaticMarkup(
      <ThinkingMessage content="Reviewing the entry point" status="running" defaultExpanded />,
    );
    expect(markup).toMatch(/<details[^>]*\bopen=""/);
    expect(markup).toContain('Reviewing the entry point');
  });

  it.each([
    { status: 'completed', label: 'Thoughts' },
    { status: 'interrupted', label: 'Thinking stopped' },
    { status: 'error', label: 'Thinking stopped' },
  ] as const)('retains readable text after the $status state', ({ status, label }) => {
    const markup = renderToStaticMarkup(
      <ThinkingMessage content="Already received thinking" status={status} defaultExpanded />,
    );
    expect(markup).toContain(`class="thinking-message ${status}" open="" aria-busy="false"`);
    expect(markup).toContain(`>${label}</span>`);
    expect(markup).not.toContain('thinking-message-dots');
    expect(markup).toContain('Already received thinking');
  });

  it('explains an empty waiting panel using the selected language', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <ThinkingMessage content="" status="running" defaultExpanded />,
    );
    expect(markup).toContain('>正在思考</span>');
    expect(markup).toContain('aria-label="思考内容"');
    expect(markup).toContain('placeholder="正在等待模型返回思考内容…"');
  });
});
