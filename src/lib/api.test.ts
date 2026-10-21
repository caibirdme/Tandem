import { describe, expect, it, vi } from 'vitest';

import { CommandError, commandError, dispatch, errorMessage, subscribe } from './api';

describe('browser event transport', () => {
  it.each([
    {
      name: 'live context readings and reset events',
      events: [
        { currentTokens: 153_000, tokenLimit: 1_000_000 },
        { currentTokens: 20_000, tokenLimit: 1_000_000 },
        null,
      ].map((data) => ({ sessionId: 'work-1', kind: 'context_usage', data })),
    },
    {
      name: 'grouped activity updates',
      events: ['running', 'completed'].map((status) => ({
        sessionId: 'work-1',
        kind: 'message',
        data: {
          id: 'activity-1',
          sessionId: 'work-1',
          sequence: 2,
          role: 'activity',
          content: 'Inspecting the project',
          createdAt: '2026-10-20T00:00:00Z',
          activity: {
            status,
            tools: [{ toolCallId: 'tool-1', name: 'read_file', status }],
          },
        },
      })),
    },
  ])('forwards $name through the subscription', async ({ events }) => {
    const source = new EventTarget();
    const close = vi.fn();
    vi.stubGlobal('window', {});
    vi.stubGlobal(
      'EventSource',
      class {
        addEventListener = source.addEventListener.bind(source);
        close = close;
      },
    );
    const receive = vi.fn();
    try {
      const unsubscribe = await subscribe(receive, () => undefined);
      for (const event of events) {
        source.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(event) }));
        expect(receive).toHaveBeenLastCalledWith(event);
      }
      expect(receive).toHaveBeenCalledTimes(events.length);
      unsubscribe();
      expect(close).toHaveBeenCalledOnce();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe('typed command failures', () => {
  it('retains the desktop error category and readable message', () => {
    const error = commandError({ code: 'conflict', error: 'The conversation is running.' });
    expect(error).toBeInstanceOf(CommandError);
    expect((error as CommandError).code).toBe('conflict');
    expect(errorMessage(error)).toBe('The conversation is running.');
    expect(errorMessage({ code: 'not_found', error: 'Conversation does not exist' })).toBe(
      'Conversation does not exist',
    );
  });

  it('does not treat unrecognized response codes as valid command failures', () => {
    const original = new Error('Connection failed');
    expect(commandError(original)).toBe(original);
    const error = commandError({ code: 'unknown', error: 'Private body' }, 'Request failed');
    expect(error).not.toBeInstanceOf(CommandError);
    expect(error.message).toBe('Request failed');
  });

  it('retains HTTP failure categories so callers can handle conflicts by type', async () => {
    vi.stubGlobal('window', {});
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: false,
        status: 409,
        json: async () => ({ code: 'conflict', error: 'The conversation is running.' }),
      }),
    );
    try {
      await expect(dispatch('snapshot', {})).rejects.toMatchObject({
        name: 'CommandError',
        code: 'conflict',
        message: 'The conversation is running.',
      });
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
