import { useEffect, useMemo, useState } from 'react';

import { loadProjectSkills } from '../lib/projectSkills';
import type { ProjectSkill } from '../lib/types';

export function useProjectSkills(projectId: string | undefined, open: boolean, connected: boolean) {
  const request = useMemo(() => ({ projectId, open, connected }), [projectId, open, connected]);
  const [catalog, setCatalog] = useState<{
    request: typeof request;
    skills: ProjectSkill[];
  } | null>(null);

  useEffect(() => {
    if (!request.projectId || !request.open || !request.connected) return;
    let active = true;
    void loadProjectSkills(request.projectId).then((skills) => {
      if (active) setCatalog({ request, skills });
    });
    return () => {
      active = false;
    };
  }, [request]);

  return {
    skills: catalog?.request === request && connected ? catalog.skills : [],
    loading: connected && catalog?.request !== request,
  };
}
