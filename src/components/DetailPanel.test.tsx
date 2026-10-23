import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import { DetailPanel } from './DetailPanel';
import { SubagentDetailPanel } from './SubagentDetailPanel';
import { WorkspacePanels } from './WorkspacePanels';

afterEach(() => setLocale('en'));

describe('Detail panel shell', () => {
  it('provides a named region and close control independently of the layout', () => {
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
    expect(markup).not.toContain('aria-modal="true"');
  });

  it('uses a named keyboard-accessible separator only when details are present', () => {
    const markup = renderToStaticMarkup(
      <WorkspacePanels detail={<p>Details</p>} onCloseDetail={() => undefined}>
        <main>Conversation</main>
      </WorkspacePanels>,
    );
    expect(markup).toContain('role="separator"');
    expect(markup).toContain('aria-label="Resize detail panel"');
    expect(markup).toContain('tabindex="0"');
    expect(markup).toContain('id="conversation-pane"');
    expect(markup).toContain('id="detail-pane"');
    expect(markup).toContain('Conversation');
    expect(markup).toContain('Details');

    const closed = renderToStaticMarkup(
      <WorkspacePanels detail={null} onCloseDetail={() => undefined}>
        <main>Conversation</main>
      </WorkspacePanels>,
    );
    expect(closed).toContain('id="conversation-pane"');
    expect(closed).not.toContain('role="separator"');
    expect(closed).not.toContain('id="detail-pane"');
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
    expect(markup).toContain('Inspect permissions');
    expect(markup).toContain('Trace approvals');
    expect(markup).toContain('Live finding');
    expect(markup).toContain('test-model');
  });
});
