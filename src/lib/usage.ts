/** Compact token labels keep the requested unit at the 1K and 1M boundaries. */
export function formatTokens(value: number): string {
  const count = Math.max(0, Math.floor(value));
  if (count < 1_000) return String(count);
  const divisor = count >= 1_000_000 ? 1_000_000 : 1_000;
  const compact = Math.floor(count / (divisor / 100)) / 100;
  return `${compact}${divisor === 1_000_000 ? 'M' : 'K'}`;
}

export type UsageRange = 7 | 14 | 30;
export interface UsageDayWindow {
  date: string;
  start: string;
  end: string;
}

/** Calendar arithmetic (not 24-hour subtraction) preserves local DST boundaries. */
export function usageDayWindows(range: UsageRange, now = new Date()): UsageDayWindow[] {
  return Array.from({ length: range }, (_, index) => {
    const start = new Date(now.getFullYear(), now.getMonth(), now.getDate() - range + index + 1);
    const end = new Date(start.getFullYear(), start.getMonth(), start.getDate() + 1);
    const date = `${start.getFullYear()}-${String(start.getMonth() + 1).padStart(2, '0')}-${String(start.getDate()).padStart(2, '0')}`;
    return { date, start: start.toISOString(), end: end.toISOString() };
  });
}

export function usageDateLabel(date: string, locale: string, long = false): string {
  const [year = 2000, month = 1, day = 1] = date.split('-').map(Number);
  return new Intl.DateTimeFormat(locale, {
    month: long ? 'short' : 'numeric',
    day: 'numeric',
    ...(long ? { year: 'numeric' as const } : {}),
  }).format(new Date(year, month - 1, day));
}

export function seriesColor(index: number): string {
  return `var(--usage-color-${index % 8})`;
}

export interface UsageDashboardData {
  trackingStartedAt: string;
  dailyTokens: {
    date: string;
    providerId: string;
    providerName: string;
    modelId: string;
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
  }[];
  dailySkills: { date: string; skillName: string; count: number }[];
  recentSessions: { sessionId: string; title: string; updatedAt: string; totalTokens: number }[];
  totalTokens: number;
  skillInvocations: number;
}

export interface UsageSeries {
  id: string;
  label: string;
  values: number[];
}

export function usageSeries(data: UsageDashboardData, days: UsageDayWindow[]) {
  const tokens = new Map<string, UsageSeries>();
  const skills = new Map<string, UsageSeries>();
  const dayIndex = new Map(days.map((day, index) => [day.date, index]));
  for (const row of data.dailyTokens) {
    const index = dayIndex.get(row.date);
    if (index === undefined) continue;
    const id = JSON.stringify([row.providerId, row.modelId]);
    const series = tokens.get(id) ?? {
      id,
      label: `${row.providerName} / ${row.modelId}`,
      values: days.map(() => 0),
    };
    series.values[index] = (series.values[index] ?? 0) + row.totalTokens;
    tokens.set(id, series);
  }
  for (const row of data.dailySkills) {
    const index = dayIndex.get(row.date);
    if (index === undefined) continue;
    const series = skills.get(row.skillName) ?? {
      id: row.skillName,
      label: row.skillName,
      values: days.map(() => 0),
    };
    series.values[index] = (series.values[index] ?? 0) + row.count;
    skills.set(row.skillName, series);
  }
  const sorted = (values: Iterable<UsageSeries>) =>
    [...values].toSorted((a, b) => a.id.localeCompare(b.id));
  return { tokens: sorted(tokens.values()), skills: sorted(skills.values()) };
}
