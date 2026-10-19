import { useState } from 'react';
import type { ReactNode } from 'react';

import { errorMessage, isDesktop } from '../lib/api';
import { externalUrl, openExternalUrl } from '../lib/external';

export function BrowserLink({
  href,
  children,
  className,
}: {
  href: string | undefined;
  children: ReactNode;
  className?: string;
}) {
  const [error, setError] = useState<string | null>(null);
  const url = externalUrl(href);
  if (!url) return <span className={className}>{children}</span>;
  return (
    <>
      <a
        href={url}
        className={className}
        target="_blank"
        rel="noreferrer"
        onClick={(event) => {
          if (!isDesktop()) return;
          event.preventDefault();
          setError(null);
          void openExternalUrl(url).catch((reason: unknown) => setError(errorMessage(reason)));
        }}
      >
        {children}
      </a>
      {error && (
        <span className="link-error" role="alert">
          链接未能打开：{error}
        </span>
      )}
    </>
  );
}
