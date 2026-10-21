import { dispatch } from './api';
import type { ProjectSkill } from './types';

const FRESH_FOR_MS = 60_000;
const MAX_CACHED_PROJECTS = 20;

export function createProjectSkillCache() {
  const catalogs = new Map<string, { skills: ProjectSkill[]; updatedAt: number }>();
  const pending = new Map<string, Promise<ProjectSkill[]>>();

  function get(projectId: string): ProjectSkill[] | undefined {
    return catalogs.get(projectId)?.skills;
  }

  function load(projectId: string): Promise<ProjectSkill[]> {
    const existing = pending.get(projectId);
    if (existing) return existing;
    const cached = catalogs.get(projectId);
    if (cached && Date.now() - cached.updatedAt < FRESH_FOR_MS) {
      catalogs.delete(projectId);
      catalogs.set(projectId, cached);
      return Promise.resolve(cached.skills);
    }

    const request = dispatch('list_project_skills', { projectId })
      .then((skills) => {
        if (pending.get(projectId) === request) {
          catalogs.delete(projectId);
          catalogs.set(projectId, { skills, updatedAt: Date.now() });
          while (catalogs.size > MAX_CACHED_PROJECTS) {
            const oldest = catalogs.keys().next().value;
            if (oldest !== undefined) catalogs.delete(oldest);
          }
        }
        return skills;
      })
      .catch(() => cached?.skills ?? [])
      .finally(() => {
        if (pending.get(projectId) === request) pending.delete(projectId);
      });
    pending.set(projectId, request);
    return request;
  }

  function invalidate(projectId: string) {
    catalogs.delete(projectId);
    pending.delete(projectId);
  }

  return { get, load, invalidate };
}

export const projectSkills = createProjectSkillCache();
