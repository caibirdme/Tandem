import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

const target =
  process.argv[2] ??
  execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
if (!target) throw new Error('Unable to determine Rust target.');
if (!/^[a-z0-9_-]+$/.test(target)) throw new Error('Invalid Rust target.');
if (target.includes('windows')) {
  console.info('Windows desktops use Linux or macOS remote artifacts from the release build.');
  process.exit(0);
}
const explicit = process.argv[2] !== undefined;
const source = join('target', ...(explicit ? [target] : []), 'release', 'tandem-server');
if (!existsSync(source)) throw new Error(`Build ${source} before preparing remote artifacts.`);
const destination = join('src-tauri', 'remote', target);
mkdirSync(destination, { recursive: true });
copyFileSync(source, join(destination, 'tandem-server'));
chmodSync(join(destination, 'tandem-server'), 0o700);
console.info(`Prepared SSH server for ${target}.`);
