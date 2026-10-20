import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it } from 'vitest';

import { setLocale } from '../i18n';
import type { ConversationMessage } from '../lib/conversation';
import { DraftAttachments, MessageAttachments } from './Attachments';

const input = { name: 'screen.png', mimeType: 'image/png', data: 'aGVsbG8=' };
const metadata = { id: 'attachment-1', name: input.name, mimeType: input.mimeType, size: 5 };

afterEach(() => setLocale('en'));

describe('attachment cards', () => {
  it('shows an image thumbnail with a named remove control', () => {
    const markup = renderToStaticMarkup(
      <DraftAttachments
        items={[{ ...metadata, status: 'ready', input }]}
        disabled={false}
        onRemove={() => undefined}
        onRetry={() => undefined}
      />,
    );
    expect(markup).toContain('src="data:image/png;base64,aGVsbG8="');
    expect(markup).toContain('alt="screen.png"');
    expect(markup).toContain('aria-label="Remove screen.png"');
    expect(markup).not.toContain('disabled=""');
  });

  it('shows pending and failed reads with accessible status and retry controls', () => {
    const markup = renderToStaticMarkup(
      <DraftAttachments
        items={[
          { ...metadata, status: 'loading' },
          { ...metadata, id: 'second', name: 'failed.txt', status: 'error' },
        ]}
        disabled={false}
        onRemove={() => undefined}
        onRetry={() => undefined}
      />,
    );
    expect(markup).toContain('aria-label="Reading attachment…"');
    expect(markup).toContain('aria-label="Retry failed.txt"');
  });

  it('retains only metadata in ordinary history and shows an optimistic send from memory', () => {
    const message: ConversationMessage = {
      id: 'message-1',
      sessionId: 'session-1',
      role: 'user',
      content: '',
      createdAt: '2026-10-20T00:00:00Z',
      attachments: [metadata],
    };
    const history = renderToStaticMarkup(<MessageAttachments message={message} />);
    expect(history).toContain('Preview screen.png');
    expect(history).not.toContain('data:image');
    const optimistic = renderToStaticMarkup(
      <MessageAttachments message={{ ...message, pendingAttachments: [input] }} />,
    );
    expect(optimistic).toContain('data:image/png;base64,aGVsbG8=');
  });

  it('offers file downloads and translates remove labels', () => {
    const file = { ...metadata, name: 'report.pdf', mimeType: 'application/pdf' };
    const message: ConversationMessage = {
      id: 'message-1',
      sessionId: 'session-1',
      role: 'user',
      content: '',
      createdAt: '2026-10-20T00:00:00Z',
      attachments: [file],
    };
    expect(renderToStaticMarkup(<MessageAttachments message={message} />)).toContain(
      'Download report.pdf',
    );
    setLocale('zh-CN');
    const markup = renderToStaticMarkup(
      <DraftAttachments
        items={[{ ...metadata, status: 'ready', input }]}
        disabled
        onRemove={() => undefined}
        onRetry={() => undefined}
      />,
    );
    expect(markup).toContain('aria-label="移除 screen.png"');
    expect(markup).toContain('disabled=""');
  });
});
