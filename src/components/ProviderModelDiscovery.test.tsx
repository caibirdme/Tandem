import { readFileSync } from 'node:fs';

import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { initialCredentialEditor } from '../lib/credentialEditor';
import { ProviderModelDiscovery } from './ProviderModelDiscovery';

function markup(saved = false, blank = false, disabled = false) {
  return renderToStaticMarkup(
    <ProviderModelDiscovery
      baseUrl="https://example.com"
      listModelsPath="/v1/models"
      onListModelsPathChange={() => undefined}
      protocol="anthropic"
      credential={{ ...initialCredentialEditor(saved && !blank), modified: blank }}
      hasSavedKey={saved}
      disabled={disabled}
      existingIds={[]}
      onAdd={() => undefined}
    />,
  );
}

describe('discovery controls', () => {
  it('requires an explicit fetch action and initially renders no selected or added models', () => {
    const rendered = markup();
    expect(rendered).toContain('Fetch models');
    expect(rendered).toContain('type="button"');
    expect(rendered).not.toContain('type="checkbox"');
    expect(rendered).not.toContain('Add selected');
    expect(rendered).not.toContain('disabled');
  });
  it('keeps the labeled path and primary fetch button together, with help below', () => {
    const rendered = markup();
    const row = rendered.match(/<div class="model-discovery-controls">(.*?)<\/div>/)?.[1];
    expect(row).toBeDefined();
    expect(row).toMatch(/<input[^>]*value="\/v1\/models"[^>]*><button/);
    expect(row).toContain('class="button primary"');
    expect(row).toContain('aria-hidden="true"');
    const pathId = row?.match(/id="([^"]+)"/)?.[1];
    expect(rendered).toContain(`<label for="${pathId}">`);
    expect(row).toContain(`aria-describedby="${pathId}-hint"`);
    expect(rendered).toContain(`</button></div><small id="${pathId}-hint"`);
    expect(rendered).toContain('aria-busy="false"');
  });
  it('disables the fetch action while the provider is busy', () => {
    expect(markup(false, false, true)).toMatch(/<button[^>]*disabled=""/);
  });
  it('bounds only the path input and allows the controls to wrap on narrow layouts', () => {
    const styles = readFileSync(new URL('./ProviderForm.css', import.meta.url), 'utf8');
    const row = styles.match(/\.model-discovery-controls \{([^}]+)\}/)?.[1];
    const input = styles.match(/\.model-discovery-controls > input \{([^}]+)\}/)?.[1];
    const button = styles.match(/\.model-discovery-controls > \.button \{([^}]+)\}/)?.[1];
    expect(row).toContain('display: flex');
    expect(row).toContain('flex-wrap: wrap');
    expect(input).toContain('max-width: 24rem');
    expect(input).toContain('min-width: 0');
    expect(button).toContain('flex: 0 0 auto');
    expect(button).not.toMatch(/width|background|border|color/);
  });
  it('requires explicit key intent after blanking a saved credential', () => {
    const rendered = markup(true, true);
    expect(rendered).toContain('disabled=""');
    expect(rendered).toContain('Enter an API key or choose Clear saved key before fetching');
  });
  it('disables fetching while a saved credential is loading', () => {
    const rendered = markup(true);
    expect(rendered).toContain('disabled=""');
    expect(rendered).toContain('Wait for the saved API key to load');
  });
});
