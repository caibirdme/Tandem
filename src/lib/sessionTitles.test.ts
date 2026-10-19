import { describe, expect, it } from 'vitest';

import { displaySessionTitle } from './sessionTitles';
import type { Session } from './types';

const session: Session = {
  contextUsage: null,
  id: 'session',
  title: 'New chat',
  titleState: 'pending',
  projectId: null,
  providerId: 'provider',
  modelId: 'model',
  permissionMode: 'ask',
  status: 'idle',
  createdAt: '',
  updatedAt: '',
};

describe('session display titles', () => {
  it.each([
    { projectId: null, title: 'New chat', en: 'New chat', zh: '新对话' },
    { projectId: 'project', title: 'New work', en: 'New work', zh: '新工作' },
  ])(
    'localizes untouched $title without changing the stored title',
    ({ projectId, title, en, zh }) => {
      const item = { ...session, projectId, title };
      expect(displaySessionTitle(item, 'en')).toBe(en);
      expect(displaySessionTitle(item, 'zh-CN')).toBe(zh);
      expect(item.title).toBe(title);
    },
  );

  it.each(['preserved', 'attempted'] as const)(
    'preserves existing content for title state %s even when it matches the default',
    (titleState) => {
      const item = { ...session, titleState };
      expect(displaySessionTitle(item, 'zh-CN')).toBe('New chat');
    },
  );
});
