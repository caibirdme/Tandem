import type { ReactNode } from 'react';
import { Group, Panel, Separator } from 'react-resizable-panels';

import { useI18n } from '../i18n';
import { useNarrowDetailPanel } from './DetailPanel';

/** Keep the conversation mounted while details open, close or become a drawer. */
export function WorkspacePanels({
  children,
  detail,
  onCloseDetail,
}: {
  children: ReactNode;
  detail: ReactNode;
  onCloseDetail: () => void;
}) {
  const { t } = useI18n();
  const narrow = useNarrowDetailPanel();
  const split = Boolean(detail) && !narrow;
  return (
    <>
      <Group className="workspace-panels" id="workspace-panels" orientation="horizontal">
        <Panel
          id="conversation-pane"
          minSize={split ? '360px' : 0}
          className="workspace-main-panel"
          style={{ overflow: 'hidden' }}
        >
          {children}
        </Panel>
        {split && (
          <>
            <Separator
              className="detail-panel-resize"
              aria-label={t('panel.resize')}
              onKeyDown={(event) => {
                if (event.key === 'Escape' && !event.defaultPrevented) {
                  event.preventDefault();
                  onCloseDetail();
                }
              }}
            />
            <Panel
              id="detail-pane"
              defaultSize="42%"
              minSize="300px"
              maxSize="55%"
              style={{ overflow: 'hidden' }}
            >
              {detail}
            </Panel>
          </>
        )}
      </Group>
      {narrow && detail}
    </>
  );
}
