import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { clampPanelWidth, DetailPanel } from './DetailPanel';
import { SubagentDetailPanel } from './SubagentDetailPanel';

afterEach(() => setLocale('en'));

describe('Detail panel shell', () => {
  it('provides a named region, close control and keyboard-adjustable width control', () => {
    const markup = renderToStaticMarkup(
      <DetailPanel id="details" title="Inspect changes" onClose={() => undefined}>
        <p>Independent panel content</p>
      </DetailPanel>,
    );
    expect(markup).toContain('<section');
    expect(markup).toContain('aria-labelledby="details-title"');
    expect(markup).not.toContain('role="dialog"');
    expect(markup).toContain('Inspect changes');
    expect(markup).toContain('Independent panel content');
    expect(markup).toContain('Close detail panel');
    expect(markup).toContain('type="range"');
    expect(markup).toContain('aria-valuenow="42"');
    expect(markup).toContain('aria-controls="details"');
    expect(markup).not.toContain('aria-modal="true"');
  });

  it('constrains resizing so both panes remain usable', () => {
    expect(clampPanelWidth(10)).toBe(28);
    expect(clampPanelWidth(90)).toBe(55);
    expect(clampPanelWidth(42)).toBe(42);
    expect(clampPanelWidth(55, 45)).toBe(45);
    expect(clampPanelWidth(43, 45)).toBe(43);
  });

  it('uses localized controls and preserves live agent content and status', () => {
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <SubagentDetailPanel
        id="details"
        agent={{
          id: 'worker',
          toolCallId: 'tool',
          name: 'explore',
          displayName: 'Inspect permissions',
          description: 'Trace approvals',
          status: 'running',
          model: 'test-model',
        }}
        messages={[]}
        stream="Live finding"
        onClose={() => undefined}
      />,
    );
    expect(markup).toContain('关闭详情面板');
    expect(markup).toContain('调整详情面板宽度');
    expect(markup).toContain('Inspect permissions');
    expect(markup).toContain('Trace approvals');
    expect(markup).toContain('Live finding');
    expect(markup).toContain('test-model');
  });
});
