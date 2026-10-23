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

for (const [target, engineArchitecture, architecture, compiler, compilerPackage] of [
  ['x86_64-unknown-linux-gnu', 'x86_64', 'amd64', 'x86_64-linux-gnu', ''],
  ['x86_64-unknown-linux-gnu', 'aarch64', 'arm64', 'x86_64-linux-gnu', 'g++-x86-64-linux-gnu'],
  ['aarch64-unknown-linux-gnu', 'arm64', 'arm64', 'aarch64-linux-gnu', ''],
  ['aarch64-unknown-linux-gnu', 'amd64', 'amd64', 'aarch64-linux-gnu', 'g++-aarch64-linux-gnu'],
]) {
  test(`builds ${target} with native compilers on a ${engineArchitecture} engine`, (t) => {
    const platform = `linux/${architecture}`;
    const root = fixture(t);
    const calls = [];
    const sourcePath = buildLinuxArtifact({
      root,
      target,
      runCommand(command, args, options) {
        calls.push({ command, args, options });
        if (args[0] === 'info')
          return JSON.stringify({ OSType: 'linux', Architecture: engineArchitecture });
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
    assert.ok(build.includes(`RUST_TARGET=${target}`));
    assert.ok(build.includes(`CROSS_COMPILER_PACKAGE=${compilerPackage}`));
    assert.equal(
      build[build.indexOf('--tag') + 1],
      `tandem-remote-builder:1.99.1-${architecture}-${target}`,
    );
    assert.equal(build.at(-1), join(root, 'scripts'));
    assert.equal(
      build[build.indexOf('--file') + 1],
      join(root, 'scripts', 'remote-linux.Dockerfile'),
    );

    const run = calls[2].args;
    assert.equal(run[run.indexOf('--platform') + 1], platform);
    assert.ok(run.includes(`type=bind,"source=${root}",target=/workspace,readonly`));
    assert.ok(run.includes(`type=bind,"source=${dirname(sourcePath)}",target=/output`));
    assert.ok(
      run.includes(
        `type=volume,source=tandem-remote-${architecture}-${target}-cargo,target=/cargo-cache`,
      ),
    );
    assert.ok(
      run.some(
        (argument) =>
          argument.startsWith('type=volume,source=tandem-remote-') &&
          argument.endsWith(`-${architecture}-${target}-target,target=/build-target`),
      ),
    );
    assert.ok(
      run.includes(
        'cargo build --release --locked --package tandem-server --target "$1"; install -m 755 "$CARGO_TARGET_DIR/$1/release/tandem-server" /output/tandem-server',
      ),
    );
    const environmentTarget = target.replaceAll('-', '_');
    assert.ok(
      run.includes(`CARGO_TARGET_${environmentTarget.toUpperCase()}_LINKER=${compiler}-gcc`),
    );
    assert.ok(run.includes(`CC_${environmentTarget}=${compiler}-gcc`));
    assert.ok(run.includes(`CXX_${environmentTarget}=${compiler}-g++`));
    assert.ok(run.includes(`AR_${environmentTarget}=${compiler}-ar`));
    assert.equal(
      run.some((argument) => /^(CC|CXX|AR)=/.test(argument)),
      false,
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
    () =>
      buildLinuxArtifact({
        root,
        runCommand: () => JSON.stringify({ OSType: 'windows', Architecture: 'x86_64' }),
      }),
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
            if (args[0] === 'info')
              return JSON.stringify({ OSType: 'linux', Architecture: 'aarch64' });
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

for (const [information, expected] of [
  ['invalid-json', /Docker returned invalid system information/],
  ['null', /Docker returned invalid system information/],
  ['[]', /Docker returned invalid system information/],
  [
    JSON.stringify({ OSType: 'linux', Architecture: 's390x' }),
    /Unsupported Docker engine architecture/,
  ],
]) {
  test(`rejects unsupported Docker information: ${information}`, (t) => {
    const root = fixture(t);
    const commands = [];
    assert.throws(
      () =>
        buildLinuxArtifact({
          root,
          runCommand(_command, args) {
            commands.push(args[0]);
            return information;
          },
        }),
      expected,
    );
    assert.deepEqual(commands, ['info']);
    assert.equal(existsSync(join(root, 'target')), false);
  });
}
