import { getLocale, translate } from '../i18n';
import type { Locale } from '../i18n';
import type { Session } from './types';

/** Localize only untouched placeholders; generated and manually saved titles are content. */
export function displaySessionTitle(session: Session, locale: Locale = getLocale()): string {
  const key = session.projectId ? 'common.newWork' : 'common.newChat';
  const placeholder = session.projectId ? 'New work' : 'New chat';
  return !session.title || (session.titleState === 'pending' && session.title === placeholder)
    ? translate(key, {}, locale)
    : session.title;
}
