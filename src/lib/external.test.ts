import { describe, expect, it } from 'vitest';

import { externalUrl } from './external';

describe('external link protocol boundary', () => {
  it.each([
    ['https://example.com/docs?q=hello', 'https://example.com/docs?q=hello'],
    ['http://localhost:3000/', 'http://localhost:3000/'],
    ['javascript:alert(1)', null],
    ['file:///etc/passwd', null],
    ['data:text/html,test', null],
    ['custom:launch', null],
    ['//example.com', null],
    ['/workspace/file', null],
    [undefined, null],
  ])('validates %s', (value, expected) => {
    expect(externalUrl(value)).toBe(expected);
  });
});
