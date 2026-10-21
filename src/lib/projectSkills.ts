import { dispatch } from './api';
import type { ProjectSkill } from './types';

export async function loadProjectSkills(projectId: string): Promise<ProjectSkill[]> {
  try {
    return await dispatch('list_project_skills', { projectId });
  } catch {
    return [];
  }
}
