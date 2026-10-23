import { execFileSync } from 'node:child_process';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { stageArtifact } from './remote-artifacts.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
let explicitTarget;
let source;
let directory;
const args = process.argv.slice(2);
for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (argument === '--source' || argument === '--directory') {
    const value = args[index + 1];
    if (!value || value.startsWith('--')) throw new Error(`Expected a path after ${argument}.`);
    if (argument === '--source') source = resolve(value);
    else directory = resolve(value);
    index += 1;
  } else if (!argument.startsWith('-') && !explicitTarget) {
    explicitTarget = argument;
  } else {
    throw new Error(
      `Unexpected argument ${argument}. Usage: node scripts/prepare-remote.mjs [target] [--source path] [--directory path].`,
    );
  }
}
const target =
  explicitTarget ??
  execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
if (!target) throw new Error('Unable to determine Rust target.');
if (!/^[a-z0-9_-]+$/.test(target)) throw new Error('Invalid Rust target.');
if (target.includes('windows')) {
  console.info('Windows desktops use Linux or macOS remote artifacts from the release build.');
  process.exit(0);
}
source ??= join(root, 'target', ...(explicitTarget ? [target] : []), 'release', 'tandem-server');
stageArtifact({ root, target, source, directory });
console.info(`Prepared SSH server for ${target}.`);
