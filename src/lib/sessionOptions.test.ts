import { describe, expect, it } from 'vitest';

import { newSessionPayload } from './sessionOptions';

const selection = { providerId: 'provider-1', modelId: 'model-1' };

describe('permission scope for new sessions', () => {
  it('starts every new work draft with full access', () => {
    expect(newSessionPayload('project-1', selection)).toEqual({
      ...selection,
      projectId: 'project-1',
      permissionMode: 'allow-all',
      reasoningEffort: null,
    });
    expect(newSessionPayload('project-1', selection, 'allow-all').permissionMode).toBe('allow-all');
    expect(newSessionPayload('project-1', selection).permissionMode).toBe('allow-all');
  });

  it('never grants full authorization to chat even if passed a work selection', () => {
    expect(newSessionPayload(null, selection, 'allow-all')).toEqual({
      ...selection,
      projectId: null,
      permissionMode: 'ask',
      reasoningEffort: null,
    });
  });
});

describe('reasoning effort for new sessions', () => {
  it.each([null, 'project-1'])('includes the independent override for project %s', (projectId) => {
    for (const effort of [null, 'low', 'medium', 'high', 'xhigh', 'max'] as const)
      expect(newSessionPayload(projectId, selection, 'ask', effort).reasoningEffort).toBe(effort);
    expect(newSessionPayload(projectId, selection).reasoningEffort).toBeNull();
  });
});
