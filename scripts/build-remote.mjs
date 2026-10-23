import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { buildLinuxArtifact } from './build-remote-linux.mjs';
import { sourceFingerprint, stageArtifact, verifyArtifact } from './remote-artifacts.mjs';

const supportedTargets = new Set([
  'x86_64-unknown-linux-gnu',
  'aarch64-unknown-linux-gnu',
  'x86_64-apple-darwin',
  'aarch64-apple-darwin',
]);

export function defaultTargets(host) {
  return [...new Set(['x86_64-unknown-linux-gnu', ...(supportedTargets.has(host) ? [host] : [])])];
}

export function buildRemoteArtifacts({
  root,
  targets,
  host,
  run = execFileSync,
  buildLinux = buildLinuxArtifact,
}) {
  host ??= run('rustc', ['-vV'], { cwd: root, encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
  if (!host) throw new Error('Unable to determine the Rust host target.');
  const directory = join(root, 'src-tauri', 'remote');
  const requested = new Set(targets?.length ? targets : defaultTargets(host));
  // Tauri copies the entire resource directory, so extra targets must also be current.
  if (existsSync(directory)) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.isDirectory()) requested.add(entry.name);
    }
  }
  for (const target of requested) {
    if (!supportedTargets.has(target))
      throw new Error(`Unsupported remote artifact target: ${target}`);
  }
  for (const target of requested) {
    try {
      verifyArtifact({ root, target, directory });
      console.info(`Reusing verified SSH server for ${target}.`);
      continue;
    } catch (error) {
      console.info(`Preparing SSH server for ${target}: ${error.message}`);
    }
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
    } else if (target.endsWith('-unknown-linux-gnu')) {
      source = buildLinux({ root, target });
    } else {
      throw new Error(
        `Build ${target} on a matching Mac, run scripts/prepare-remote.mjs there, and copy its ` +
          `complete src-tauri/remote/${target} directory (including artifact.json) into this checkout.`,
      );
    }
    if (before !== sourceFingerprint(root)) {
      throw new Error('Remote server sources changed during compilation. Run the build again.');
    }
    stageArtifact({ root, target, source, directory });
    verifyArtifact({ root, target, directory });
    if (target !== host && target.endsWith('-unknown-linux-gnu')) {
      rmSync(source);
    }
    console.info(`Prepared SSH server for ${target}.`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    buildRemoteArtifacts({
      root: resolve(dirname(fileURLToPath(import.meta.url)), '..'),
      targets: process.argv.slice(2),
    });
  } catch (error) {
    console.error(`Remote artifact preparation failed: ${error.message}`);
    process.exitCode = 1;
  }
}
