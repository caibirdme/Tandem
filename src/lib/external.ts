import { translate as t } from '../i18n';
export function externalUrl(value: string | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
  } catch {
    return null;
  }
}

export async function openExternalUrl(value: string): Promise<void> {
  const url = externalUrl(value);
  if (!url) throw new Error(t('error.externalProtocol'));
  const { openUrl } = await import('@tauri-apps/plugin-opener');
  await openUrl(url);
}
