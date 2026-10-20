import { beforeEach, describe, expect, it, vi } from 'vitest';

import { dispatch } from './api';
import { loadProjectSkills } from './projectSkills';
import type { ProjectSkill } from './types';

vi.mock('./api', () => ({ dispatch: vi.fn() }));

beforeEach(() => {
  vi.mocked(dispatch).mockReset();
});

describe('optional project skill discovery', () => {
  it('preserves discovered skills for selection', async () => {
    const skills: ProjectSkill[] = [
      {
        name: 'review',
        description: 'Review project changes',
        path: '/project/.agents/skills/review/SKILL.md',
      },
    ];
    vi.mocked(dispatch).mockResolvedValue(skills);

    await expect(loadProjectSkills('project-1')).resolves.toEqual(skills);
    expect(dispatch).toHaveBeenCalledWith('list_project_skills', { projectId: 'project-1' });
  });

  it.each([
    new Error('Failed to scan skill directory: permission denied'),
    new Error('SSH connection closed'),
    { code: 'unavailable', error: 'Runtime unavailable' },
  ])('treats discovery failure as an empty catalog', async (error) => {
    vi.mocked(dispatch).mockRejectedValue(error);

    await expect(loadProjectSkills('project-1')).resolves.toEqual([]);
  });
});
