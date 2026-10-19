import { describe, expect, it } from 'vitest';

import { newSessionPayload } from './sessionOptions';

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
