import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { dispatch } from './api';
import { createProjectSkillCache } from './projectSkills';
import type { ProjectSkill } from './types';

vi.mock('./api', () => ({ dispatch: vi.fn() }));

let cache: ReturnType<typeof createProjectSkillCache>;
const skills: ProjectSkill[] = [
  {
    name: 'review',
    description: 'Review project changes',
    path: '/project/.agents/skills/review/SKILL.md',
  },
];

beforeEach(() => {
  vi.mocked(dispatch).mockReset();
  vi.useFakeTimers();
  cache = createProjectSkillCache();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('optional project skill discovery', () => {
  it('preserves discovered skills for selection', async () => {
    vi.mocked(dispatch).mockResolvedValue(skills);

    await expect(cache.load('project-1')).resolves.toEqual(skills);
    expect(dispatch).toHaveBeenCalledWith('list_project_skills', { projectId: 'project-1' });
  });

  it.each([
    new Error('Failed to scan skill directory: permission denied'),
    new Error('SSH connection closed'),
    { code: 'unavailable', error: 'Runtime unavailable' },
  ])('treats discovery failure as an empty catalog', async (error) => {
    vi.mocked(dispatch).mockRejectedValue(error);

    await expect(cache.load('project-1')).resolves.toEqual([]);
    expect(cache.get('project-1')).toBeUndefined();
    vi.mocked(dispatch).mockResolvedValue(skills);
    await expect(cache.load('project-1')).resolves.toEqual(skills);
    expect(dispatch).toHaveBeenCalledTimes(2);
  });

  it('shares an in-flight scan between prefetch and repeated picker openings', async () => {
    vi.mocked(dispatch).mockResolvedValue(skills);
    const prefetch = cache.load('project-1');
    expect(cache.get('project-1')).toBeUndefined();
    expect(cache.load('project-1')).toBe(prefetch);
    await prefetch;
    expect(cache.get('project-1')).toBe(skills);
    await cache.load('project-1');
    expect(dispatch).toHaveBeenCalledTimes(1);
  });

  it('shows the last catalog while refreshing an expired entry', async () => {
    vi.mocked(dispatch).mockResolvedValueOnce(skills).mockResolvedValueOnce([]);
    await cache.load('project-1');
    vi.advanceTimersByTime(60_000);
    const refreshed = cache.load('project-1');
    expect(cache.get('project-1')).toBe(skills);
    expect(cache.load('project-1')).toBe(refreshed);
    await expect(refreshed).resolves.toEqual([]);
    expect(cache.get('project-1')).toEqual([]);
    expect(dispatch).toHaveBeenCalledTimes(2);
  });

  it('keeps a stale catalog on refresh failure and retries on the next opening', async () => {
    vi.mocked(dispatch).mockResolvedValueOnce(skills).mockRejectedValueOnce(new Error('Offline'));
    await cache.load('project-1');
    vi.advanceTimersByTime(60_000);
    await expect(cache.load('project-1')).resolves.toBe(skills);
    expect(cache.get('project-1')).toBe(skills);
    vi.mocked(dispatch).mockResolvedValueOnce([]);
    await expect(cache.load('project-1')).resolves.toEqual([]);
    expect(dispatch).toHaveBeenCalledTimes(3);
  });

  it('caches genuinely empty catalogs without rescanning on every opening', async () => {
    vi.mocked(dispatch).mockResolvedValue([]);
    await cache.load('project-1');
    await cache.load('project-1');
    expect(cache.get('project-1')).toEqual([]);
    expect(dispatch).toHaveBeenCalledTimes(1);
  });

  it('isolates local and SSH project catalogs', async () => {
    vi.mocked(dispatch).mockResolvedValueOnce(skills).mockResolvedValueOnce([]);
    await Promise.all([cache.load('local'), cache.load('ssh')]);
    expect(cache.get('local')).toBe(skills);
    expect(cache.get('ssh')).toEqual([]);
    expect(dispatch).toHaveBeenCalledTimes(2);
  });

  it('discards disconnected requests without overwriting a replacement scan', async () => {
    vi.mocked(dispatch).mockResolvedValueOnce(skills).mockResolvedValueOnce([]);
    const disconnected = cache.load('project-1');
    cache.invalidate('project-1');
    const reconnected = cache.load('project-1');
    expect(reconnected).not.toBe(disconnected);
    await disconnected;
    await reconnected;
    expect(cache.get('project-1')).toEqual([]);
    expect(dispatch).toHaveBeenCalledTimes(2);
  });

  it('bounds cached catalogs and retains recently reused projects', async () => {
    vi.mocked(dispatch).mockResolvedValue(skills);
    await Promise.all(
      Array.from({ length: 20 }, (_, index) => cache.load(`project-${String(index)}`)),
    );
    await cache.load('project-0');
    await cache.load('project-20');
    expect(cache.get('project-1')).toBeUndefined();
    expect(cache.get('project-0')).toBe(skills);
    expect(cache.get('project-20')).toBe(skills);
  });
});
