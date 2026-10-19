import { describe, expect, it } from 'vitest';

import { needsWorkPermissionConfirmation, newSessionPayload } from './sessionOptions';
import type { Message, PermissionMode } from './types';

const selection = { providerId: 'provider-1', modelId: 'model-1' };

describe('permission scope for new sessions', () => {
  it('starts every new work draft with approval required unless explicitly changed', () => {
    expect(newSessionPayload('project-1', selection)).toEqual({
      ...selection,
      projectId: 'project-1',
      permissionMode: 'ask',
    });
    expect(newSessionPayload('project-1', selection, 'allow-all').permissionMode).toBe('allow-all');
    expect(newSessionPayload('project-1', selection).permissionMode).toBe('ask');
  });

  it('never grants full authorization to chat even if passed a work selection', () => {
    expect(newSessionPayload(null, selection, 'allow-all')).toEqual({
      ...selection,
      projectId: null,
      permissionMode: 'ask',
    });
  });
});

describe('first work message permission reminder', () => {
  const cases: {
    name: string;
    projectId: string | null;
    sessionId: string | null;
    permissionMode: PermissionMode;
    historyLoaded: boolean;
    roles: Message['role'][];
    expected: boolean;
  }[] = [
    {
      name: 'new work draft',
      projectId: 'local-project',
      sessionId: null,
      permissionMode: 'ask',
      historyLoaded: false,
      roles: [],
      expected: true,
    },
    {
      name: 'SSH work draft',
      projectId: 'ssh-project',
      sessionId: null,
      permissionMode: 'ask',
      historyLoaded: false,
      roles: [],
      expected: true,
    },
    {
      name: 'ordinary chat',
      projectId: null,
      sessionId: null,
      permissionMode: 'ask',
      historyLoaded: true,
      roles: [],
      expected: false,
    },
    {
      name: 'explicit full authorization',
      projectId: 'project',
      sessionId: null,
      permissionMode: 'allow-all',
      historyLoaded: true,
      roles: [],
      expected: false,
    },
    {
      name: 'existing empty work',
      projectId: 'project',
      sessionId: 'session',
      permissionMode: 'ask',
      historyLoaded: true,
      roles: [],
      expected: true,
    },
    {
      name: 'existing work with unknown history',
      projectId: 'project',
      sessionId: 'session',
      permissionMode: 'ask',
      historyLoaded: false,
      roles: [],
      expected: false,
    },
    {
      name: 'subsequent work message',
      projectId: 'project',
      sessionId: 'session',
      permissionMode: 'ask',
      historyLoaded: true,
      roles: ['user', 'assistant'],
      expected: false,
    },
    {
      name: 'first message after an assistant notice',
      projectId: 'project',
      sessionId: 'session',
      permissionMode: 'ask',
      historyLoaded: true,
      roles: ['assistant'],
      expected: true,
    },
  ];

  it.each(cases)('$name', ({ roles, expected, ...options }) => {
    expect(
      needsWorkPermissionConfirmation({ ...options, messages: roles.map((role) => ({ role })) }),
    ).toBe(expected);
  });
});
