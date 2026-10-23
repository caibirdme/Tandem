import assert from 'node:assert/strict';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';

import { buildLinuxArtifact } from './build-remote-linux.mjs';

function rejectDockerCall() {
  assert.fail('Docker must not run for invalid configuration');
}

function fixture(t, version = '1.99.1') {
  const root = mkdtempSync(join(tmpdir(), 'tandem linux builder, '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'rust-toolchain.toml'), `[toolchain]\nchannel = "${version}"\n`);
  writeFileSync(join(root, 'Cargo.lock'), 'original lockfile\n');
  mkdirSync(join(root, 'scripts'));
  return root;
}

for (const [target, platform] of [
  ['x86_64-unknown-linux-gnu', 'linux/amd64'],
  ['aarch64-unknown-linux-gnu', 'linux/arm64'],
]) {
  test(`builds ${target} with isolated writable mounts and an immutable source`, (t) => {
    const root = fixture(t);
    const calls = [];
    const sourcePath = buildLinuxArtifact({
      root,
      target,
      runCommand(command, args, options) {
        calls.push({ command, args, options });
        if (args[0] === 'info') return 'linux\n';
        if (args[0] === 'run') {
          const outputParent = join(root, 'target', 'remote-linux', target);
          const [directory] = readdirSync(outputParent);
          writeFileSync(join(outputParent, directory, 'tandem-server'), 'built server');
        }
        return '';
      },
    });

    assert.equal(readFileSync(sourcePath, 'utf8'), 'built server');
    assert.equal(readFileSync(join(root, 'Cargo.lock'), 'utf8'), 'original lockfile\n');
    assert.deepEqual(
      calls.map(({ command, args }) => [command, args[0]]),
      [
        ['docker', 'info'],
        ['docker', 'build'],
        ['docker', 'run'],
      ],
    );

    const build = calls[1].args;
    assert.equal(build[build.indexOf('--platform') + 1], platform);
    assert.equal(build[build.indexOf('--build-arg') + 1], 'RUST_VERSION=1.99.1');
    assert.equal(build.at(-1), join(root, 'scripts'));
    assert.equal(
      build[build.indexOf('--file') + 1],
      join(root, 'scripts', 'remote-linux.Dockerfile'),
    );

    const run = calls[2].args;
    assert.equal(run[run.indexOf('--platform') + 1], platform);
    assert.ok(run.includes(`type=bind,"source=${root}",target=/workspace,readonly`));
    assert.ok(run.includes(`type=bind,"source=${dirname(sourcePath)}",target=/output`));
    assert.ok(run.includes(`type=volume,source=tandem-remote-${target}-cargo,target=/cargo-cache`));
    assert.ok(
      run.some(
        (argument) =>
          argument.startsWith('type=volume,source=tandem-remote-') &&
          argument.endsWith(`-${target}-target,target=/build-target`),
      ),
    );
    assert.ok(
      run.includes(
        'cargo build --release --locked --package tandem-server --target "$1"; install -m 755 "$CARGO_TARGET_DIR/$1/release/tandem-server" /output/tandem-server',
      ),
    );
    assert.equal(run.at(-1), target);
    assert.ok(run.includes('--rm'));
  });
}

for (const [name, failure, message] of [
  [
    'Docker is missing',
    Object.assign(new Error('not found'), { code: 'ENOENT' }),
    /Install Docker Desktop/,
  ],
  [
    'Docker is stopped',
    new Error('Cannot connect to daemon'),
    /Start Docker Desktop or your Docker Engine/,
  ],
]) {
  test(`${name} produces an actionable failure before building`, (t) => {
    const root = fixture(t);
    const calls = [];
    assert.throws(
      () =>
        buildLinuxArtifact({
          root,
          runCommand(_command, args) {
            calls.push(args[0]);
            throw failure;
          },
        }),
      message,
    );
    assert.deepEqual(calls, ['info']);
    assert.equal(existsSync(join(root, 'target')), false);
  });
}

test('rejects Windows container mode with instructions to switch', (t) => {
  const root = fixture(t);
  assert.throws(
    () => buildLinuxArtifact({ root, runCommand: () => 'windows\n' }),
    /Switch Docker Desktop to Linux containers/,
  );
});

test('rejects unsupported targets and unpinned toolchains before invoking Docker', (t) => {
  const root = fixture(t, 'stable');
  assert.throws(
    () => buildLinuxArtifact({ root, target: 'x86_64-apple-darwin', runCommand: rejectDockerCall }),
    /Unsupported Linux remote target/,
  );
  assert.throws(
    () => buildLinuxArtifact({ root, runCommand: rejectDockerCall }),
    /must specify an exact Rust version/,
  );
});

for (const failure of ['container failure', 'missing output']) {
  test(`${failure} cleans up the incomplete staging directory`, (t) => {
    const root = fixture(t);
    assert.throws(
      () =>
        buildLinuxArtifact({
          root,
          runCommand(_command, args) {
            if (args[0] === 'info') return 'linux\n';
            if (args[0] === 'run' && failure === 'container failure') {
              throw new Error('cargo failed');
            }
            return '';
          },
        }),
      /Could not build the Linux SSH server/,
    );
    assert.deepEqual(
      readdirSync(join(root, 'target', 'remote-linux', 'x86_64-unknown-linux-gnu')),
      [],
    );
  });
}
