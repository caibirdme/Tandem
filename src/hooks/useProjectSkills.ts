import { useEffect, useMemo, useState } from 'react';

import { projectSkills } from '../lib/projectSkills';
import type { ProjectSkill, SessionStatus } from '../lib/types';

export function useProjectSkills(
  projectId: string | undefined,
  open: boolean,
  connected: boolean,
  sessionStatus: SessionStatus,
) {
  const request = useMemo(
    () => ({ projectId, connected, sessionStatus }),
    [projectId, connected, sessionStatus],
  );
  const [catalog, setCatalog] = useState<{
    request: typeof request;
    skills: ProjectSkill[];
  } | null>(null);

  useEffect(() => {
    if (!request.projectId) return;
    if (!request.connected) {
      projectSkills.invalidate(request.projectId);
      return;
    }
    let active = true;
    // Warm the catalog when Work opens, and refresh expired entries when the picker reopens.
    void projectSkills.load(request.projectId).then((skills) => {
      if (active) setCatalog({ request, skills });
    });
    return () => {
      active = false;
    };
  }, [request, open]);

  const cached = projectId && connected ? projectSkills.get(projectId) : undefined;
  const skills = cached ?? (catalog?.request === request && connected ? catalog.skills : undefined);

  return {
    skills: skills ?? [],
    loading: Boolean(projectId && connected && skills === undefined),
  };
}
