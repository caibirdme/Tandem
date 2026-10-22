import { readFileSync } from 'node:fs';

import { describe, expect, it } from 'vitest';

function rule(file: string, selector: string) {
  const styles = readFileSync(new URL(file, import.meta.url), 'utf8');
  const escapedSelector = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const declarations = styles.match(new RegExp(`${escapedSelector} \\{([^}]+)\\}`))?.[1];
  expect(declarations).toBeDefined();
  return declarations;
}

describe('transcript scroll boundaries', () => {
  it.each([
    ['./ThinkingMessage.css', '.thinking-message-content'],
    ['./FleetCard.css', '.fleet-agent-list'],
  ])('lets %s %s hand scrolling back to its transcript at either edge', (file, selector) => {
    const declarations = rule(file, selector);
    expect(declarations).toContain('overflow-y: auto;');
    expect(declarations).toContain('overscroll-behavior-y: auto;');
    expect(declarations).not.toMatch(/overscroll-behavior(?:-y)?:\s*(contain|none)/);
  });

  it.each([
    ['./DetailPanel.css', '.detail-panel-body'],
    ['./FleetCard.css', '.fleet-agent-output'],
  ])('keeps the independent %s %s scroll boundary isolated', (file, selector) => {
    expect(rule(file, selector)).toContain('overscroll-behavior: contain;');
  });
});
