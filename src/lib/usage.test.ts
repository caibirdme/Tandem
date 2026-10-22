import { describe, expect, it } from 'vitest';

import { formatTokens, usageDayWindows, usageSeries } from './usage';
import type { UsageDashboardData } from './usage';

describe('usage formatting', () => {
  it.each([
    [0, '0'],
    [999, '999'],
    [1_000, '1K'],
    [1_234, '1.23K'],
    [999_999, '999.99K'],
    [1_000_000, '1M'],
    [1_234_567, '1.23M'],
    [1_150_000, '1.15M'],
  ])('formats %s without rounding across unit boundaries', (value, expected) => {
    expect(formatTokens(value)).toBe(expected);
  });
});

describe('usage calendar windows', () => {
  it.each([7, 14, 30] as const)('includes today and produces %s contiguous local days', (range) => {
    const now = new Date(2026, 0, 2, 23, 59);
    const days = usageDayWindows(range, now);
    expect(days).toHaveLength(range);
    expect(days.at(-1)?.date).toBe('2026-01-02');
    const last = days.at(-1);
    if (!last) throw new Error('Missing day');
    expect(new Date(last.start).getHours()).toBe(0);
    expect(new Date(last.end).getDate()).toBe(3);
    for (let index = 1; index < days.length; index++) {
      expect(days[index - 1]?.end).toBe(days[index]?.start);
    }
  });
  it('uses local calendar constructors across daylight-saving transitions', () => {
    // Run this suite with TZ=America/New_York as well as UTC to exercise 23/25-hour days.
    for (const now of [new Date(2026, 2, 10, 12), new Date(2026, 10, 3, 12)]) {
      const days = usageDayWindows(7, now);
      for (const day of days) {
        const start = new Date(day.start);
        const end = new Date(day.end);
        expect(start.getHours()).toBe(0);
        expect(end.getHours()).toBe(0);
        expect(end.getTime() - start.getTime()).toBe(
          86_400_000 + (end.getTimezoneOffset() - start.getTimezoneOffset()) * 60_000,
        );
      }
    }
  });
});

describe('usage chart series', () => {
  it('keeps providers distinct, fills missing days, and accumulates repeated rows', () => {
    const days = usageDayWindows(7, new Date(2026, 9, 22));
    const data: UsageDashboardData = {
      trackingStartedAt: '2026-10-20T00:00:00Z',
      totalTokens: 60,
      skillInvocations: 2,
      recentSessions: [],
      dailyTokens: [
        {
          date: '2026-10-22',
          providerId: 'a',
          providerName: 'Same name',
          modelId: 'model',
          inputTokens: 10,
          outputTokens: 10,
          totalTokens: 20,
        },
        {
          date: '2026-10-22',
          providerId: 'b',
          providerName: 'Same name',
          modelId: 'model',
          inputTokens: 15,
          outputTokens: 15,
          totalTokens: 30,
        },
        {
          date: '2026-10-22',
          providerId: 'a',
          providerName: 'Same name',
          modelId: 'model',
          inputTokens: 5,
          outputTokens: 5,
          totalTokens: 10,
        },
      ],
      dailySkills: [{ date: '2026-10-21', skillName: 'review', count: 2 }],
    };
    const series = usageSeries(data, days);
    expect(series.tokens).toHaveLength(2);
    expect(series.tokens[0]?.values).toEqual([0, 0, 0, 0, 0, 0, 30]);
    expect(series.tokens[1]?.values).toEqual([0, 0, 0, 0, 0, 0, 30]);
    expect(series.skills[0]?.values).toEqual([0, 0, 0, 0, 0, 2, 0]);
  });
});
