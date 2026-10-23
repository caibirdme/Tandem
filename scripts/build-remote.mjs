import { execFileSync } from 'node:child_process';
import { rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { buildLinuxArtifact } from './build-remote-linux.mjs';
import { sourceFingerprint, stageArtifact, verifyArtifact } from './remote-artifacts.mjs';

const target = 'x86_64-unknown-linux-gnu';

export function buildRemoteArtifacts({
  root,
  host,
  checkOnly = false,
  run = execFileSync,
  buildLinux = buildLinuxArtifact,
}) {
  const directory = join(root, 'src-tauri', 'remote');
  try {
    verifyArtifact({ root, target, directory });
    console.info(`Reusing verified SSH server for ${target}.`);
    return;
  } catch (error) {
    if (checkOnly) {
      throw new Error(
        `Remote artifact check failed for ${target}: ${error.message} ` +
          'Run just desktop-build-full to prepare remote artifacts and build the desktop, ' +
          'or run just remote-build to prepare the artifacts separately.',
        { cause: error },
      );
    }
    console.info(`Preparing SSH server for ${target}: ${error.message}`);
  }
  host ??= run('rustc', ['-vV'], { cwd: root, encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
  if (!host) throw new Error('Unable to determine the Rust host target.');
  const before = sourceFingerprint(root);
  let source;
  if (target === host) {
    run('cargo', ['build', '--release', '--locked', '-p', 'tandem-server', '--target', target], {
      cwd: root,
      stdio: 'inherit',
    });
    const metadata = JSON.parse(
      run('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked'], {
        cwd: root,
        encoding: 'utf8',
      }),
    );
    source = join(metadata.target_directory, target, 'release', 'tandem-server');
  } else {
    source = buildLinux({ root, target });
  }
  if (before !== sourceFingerprint(root)) {
    throw new Error('Remote server sources changed during compilation. Run the build again.');
  }
  stageArtifact({ root, target, source, directory });
  verifyArtifact({ root, target, directory });
  if (target !== host) rmSync(source);
  console.info(`Prepared SSH server for ${target}.`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  try {
    if (args.length > 1 || args.some((argument) => argument !== '--check')) {
      throw new Error(
        `Unexpected arguments: ${args.join(' ')}. Usage: node scripts/build-remote.mjs [--check].`,
      );
    }
    buildRemoteArtifacts({
      root: resolve(dirname(fileURLToPath(import.meta.url)), '..'),
      checkOnly: args.includes('--check'),
    });
  } catch (error) {
    console.error(`Remote artifact preparation failed: ${error.message}`);
    process.exitCode = 1;
  }
}
