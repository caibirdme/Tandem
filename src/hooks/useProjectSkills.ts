import { useEffect, useMemo, useState } from 'react';

import { dispatch, errorMessage } from '../lib/api';
import type { ProjectSkill } from '../lib/types';

export function useProjectSkills(projectId: string | undefined, open: boolean, connected: boolean) {
  const [revision, setRevision] = useState(0);
  const request = useMemo(
    () => ({ projectId, open, connected, revision }),
    [projectId, open, connected, revision],
  );
  const [catalog, setCatalog] = useState<{
    request: typeof request;
    skills: ProjectSkill[];
    error: string | null;
  } | null>(null);

  useEffect(() => {
    if (!request.projectId || !request.open || !request.connected) return;
    let active = true;
    void dispatch('list_project_skills', { projectId: request.projectId }).then(
      (skills) => {
        if (active) setCatalog({ request, skills, error: null });
      },
      (reason: unknown) => {
        if (active) setCatalog({ request, skills: [], error: errorMessage(reason) });
      },
    );
    return () => {
      active = false;
    };
  }, [request]);

  return {
    skills: catalog?.request === request && connected ? catalog.skills : [],
    loading: connected && catalog?.request !== request,
    error: catalog?.request === request ? catalog.error : null,
    reload: () => setRevision((current) => current + 1),
  };
}
