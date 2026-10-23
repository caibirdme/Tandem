// @vitest-environment jsdom
import { act, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import type { Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { DetailPanel } from './DetailPanel';
import { WorkspacePanels } from './WorkspacePanels';

function Harness() {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = () => setOpen(false);
  return (
    <WorkspacePanels
      onCloseDetail={close}
      detail={
        open ? (
          <DetailPanel id="details" title="Details" onClose={close} returnFocusRef={trigger}>
            <button>Detail action</button>
          </DetailPanel>
        ) : null
      }
    >
      <main>
        <button ref={trigger} onClick={() => setOpen(true)}>
          Open details
        </button>
        <input aria-label="Draft" defaultValue="Unsent draft" />
      </main>
    </WorkspacePanels>
  );
}

let root: Root;
let host: HTMLDivElement;
let narrow: boolean;
const viewportListeners = new Set<() => void>();

function element<T extends HTMLElement = HTMLElement>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`Missing test element: ${selector}`);
  return found;
}
async function click(selector: string) {
  await act(() => element(selector).click());
}
async function key(target: HTMLElement, value: string) {
  await act(() =>
    target.dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true })),
  );
}
async function viewport(value: boolean) {
  await act(() => {
    narrow = value;
    viewportListeners.forEach((listener) => listener());
  });
}

beforeEach(async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  narrow = false;
  vi.stubGlobal('matchMedia', () => ({
    get matches() {
      return narrow;
    },
    addEventListener: (_: string, listener: () => void) => viewportListeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => viewportListeners.delete(listener),
  }));
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  // jsdom has no layout. Supply a fixed workspace so the real panel library can
  // initialize constraints; pixel rendering and pointer hit-testing need a browser.
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.id === 'conversation-pane' ? 696 : this.id === 'detail-pane' ? 504 : 1200;
  });
  vi.spyOn(HTMLElement.prototype, 'offsetLeft', 'get').mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.id === 'detail-pane' ? 702 : this.hasAttribute('data-separator') ? 696 : 0;
  });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.id === 'conversation-pane') return new DOMRect(0, 0, 696, 800);
    if (this.id === 'detail-pane') return new DOMRect(702, 0, 498, 800);
    if (this.hasAttribute('data-separator')) return new DOMRect(696, 0, 6, 800);
    return new DOMRect(0, 0, 1200, 800);
  });
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  await act(() => root.render(<Harness />));
});

afterEach(async () => {
  await act(() => root.unmount());
  host.remove();
  viewportListeners.clear();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('workspace detail interactions', () => {
  it('delegates keyboard sizing and closes from the separator without remounting the conversation', async () => {
    const draft = element<HTMLInputElement>('input');
    draft.value = 'Keep this unsent message';
    await click('main button');
    const separator = element('[role="separator"]');
    await act(() => separator.focus());
    const before = separator.getAttribute('aria-valuenow');
    await key(separator, 'ArrowLeft');
    expect(separator.getAttribute('aria-valuenow')).not.toBe(before);
    await key(separator, 'Home');
    expect(separator.getAttribute('aria-valuenow')).toBe(separator.getAttribute('aria-valuemin'));
    await key(separator, 'End');
    expect(separator.getAttribute('aria-valuenow')).toBe(separator.getAttribute('aria-valuemax'));
    await key(separator, 'Escape');
    expect(document.querySelector('#details')).toBeNull();
    expect(document.activeElement).toBe(element('main button'));
    expect(element('input')).toBe(draft);
    expect(draft.value).toBe('Keep this unsent message');
  });

  it('changes to a modal drawer and back without losing the conversation', async () => {
    const draft = element('input');
    await click('main button');
    await viewport(true);
    expect(document.querySelector('[role="separator"]')).toBeNull();
    expect(element('#details').getAttribute('role')).toBe('dialog');
    expect(document.activeElement).toBe(element('[aria-label="Close detail panel"]'));
    const detailAction = element<HTMLButtonElement>('#details .detail-panel-body button');
    await act(() => detailAction.focus());
    await key(detailAction, 'Tab');
    expect(document.activeElement).toBe(element('[aria-label="Close detail panel"]'));
    await viewport(false);
    expect(document.querySelector('[role="separator"]')).not.toBeNull();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(element('input')).toBe(draft);
    await viewport(true);
    await key(element('[aria-label="Close detail panel"]'), 'Escape');
    expect(document.querySelector('#details')).toBeNull();
    expect(document.activeElement).toBe(element('main button'));
  });
});
