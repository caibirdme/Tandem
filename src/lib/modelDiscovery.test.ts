import { describe, expect, it } from 'vitest';

import { emptyDiscovery, reduceDiscovery, selectedDiscoveryModels } from './modelDiscovery';

const models = [
  { id: 'one', displayName: 'First model' },
  { id: 'two', displayName: 'Second model' },
];
const loaded = () =>
  reduceDiscovery(reduceDiscovery(emptyDiscovery, { type: 'start', request: 1 }), {
    type: 'loaded',
    request: 1,
    models,
  });

describe('provider model discovery', () => {
  it('starts every fetch with no models selected and deduplicates returned identifiers', () => {
    const state = reduceDiscovery(reduceDiscovery(emptyDiscovery, { type: 'start', request: 1 }), {
      type: 'loaded',
      request: 1,
      models: [...models, { id: 'one', displayName: 'First model' }],
    });
    expect(state.models).toEqual(models);
    expect(state.selected).toEqual([]);
    expect(selectedDiscoveryModels(state, [])).toEqual([]);
  });

  it('adds only manually selected models and excludes models already present', () => {
    let state = reduceDiscovery(loaded(), { type: 'toggle', id: 'two' });
    expect(selectedDiscoveryModels(state, [])).toEqual([models[1]]);
    expect(selectedDiscoveryModels(state, ['two'])).toEqual([]);
    state = reduceDiscovery(state, { type: 'added' });
    expect(selectedDiscoveryModels(state, [])).toEqual([]);
  });

  it('allows deselection and ignores unknown identifiers', () => {
    const state = loaded();
    expect(reduceDiscovery(state, { type: 'toggle', id: 'missing' })).toBe(state);
    const selected = reduceDiscovery(state, { type: 'toggle', id: 'one' });
    expect(reduceDiscovery(selected, { type: 'toggle', id: 'one' }).selected).toEqual([]);
  });

  it('ignores success and failure from before request invalidation', () => {
    const waiting = reduceDiscovery(emptyDiscovery, { type: 'start', request: 1 });
    const invalidated = reduceDiscovery(waiting, { type: 'reset', request: 2 });
    expect(reduceDiscovery(invalidated, { type: 'loaded', request: 1, models })).toBe(invalidated);
    expect(reduceDiscovery(invalidated, { type: 'failed', request: 1, error: 'Old failure' })).toBe(
      invalidated,
    );
    expect(invalidated).toEqual({ ...emptyDiscovery, request: 2 });
  });

  it('resets earlier selections and errors on retry and only accepts the latest completion', () => {
    const selected = reduceDiscovery(loaded(), { type: 'toggle', id: 'one' });
    const retry = reduceDiscovery(selected, { type: 'start', request: 2 });
    expect(retry.selected).toEqual([]);
    expect(retry.models).toBeNull();
    expect(retry.loading).toBe(true);
    expect(reduceDiscovery(retry, { type: 'loaded', request: 1, models })).toBe(retry);
    const failed = reduceDiscovery(retry, { type: 'failed', request: 2, error: 'Request failed' });
    expect(failed.loading).toBe(false);
    expect(failed.error).toBe('Request failed');
    expect(reduceDiscovery(failed, { type: 'start', request: 3 }).error).toBeNull();
  });

  it('distinguishes an empty completed response from never fetched', () => {
    const state = reduceDiscovery(emptyDiscovery, { type: 'loaded', request: 0, models: [] });
    expect(state.models).toEqual([]);
    expect(emptyDiscovery.models).toBeNull();
  });
});
