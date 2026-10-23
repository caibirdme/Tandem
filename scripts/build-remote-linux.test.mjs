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

const engines = [
  { command: 'docker', name: 'Docker', format: '{{json .}}' },
  { command: 'podman', name: 'Podman', format: 'json' },
];
const defaultTarget = 'x86_64-unknown-linux-gnu';
const unavailableFailures = [
  ['missing', () => Object.assign(new Error('not found'), { code: 'ENOENT' })],
  ['stopped', () => new Error('Cannot connect to engine')],
];

function build(options) {
  return buildLinuxArtifact({ environment: {}, ...options });
}

function rejectContainerCall() {
  assert.fail('Container commands must not run for invalid configuration');
}

function information(command, architecture = 'amd64', os = 'linux') {
  return JSON.stringify(
    command === 'podman'
      ? { host: { os, arch: architecture } }
      : { OSType: os, Architecture: architecture },
  );
}

function fixture(t, version = '1.99.1') {
  const root = mkdtempSync(join(tmpdir(), 'tandem linux builder, '));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, 'rust-toolchain.toml'), `[toolchain]\nchannel = "${version}"\n`);
  writeFileSync(join(root, 'Cargo.lock'), 'original lockfile\n');
  mkdirSync(join(root, 'scripts'));
  return root;
}

function writeArtifact(root, target = defaultTarget) {
  const outputParent = join(root, 'target', 'remote-linux', target);
  const [directory] = readdirSync(outputParent);
  writeFileSync(join(outputParent, directory, 'tandem-server'), 'built server');
}

for (const { command, name, format } of engines) {
  for (const [target, engineArchitecture, architecture, compiler, compilerPackage] of [
    [defaultTarget, 'x86_64', 'amd64', 'x86_64-linux-gnu', ''],
    [defaultTarget, 'aarch64', 'arm64', 'x86_64-linux-gnu', 'g++-x86-64-linux-gnu'],
    ['aarch64-unknown-linux-gnu', 'arm64', 'arm64', 'aarch64-linux-gnu', ''],
    ['aarch64-unknown-linux-gnu', 'amd64', 'amd64', 'aarch64-linux-gnu', 'g++-aarch64-linux-gnu'],
  ]) {
    test(`builds ${target} on ${name} ${engineArchitecture} without CPU emulation`, (t) => {
      const platform = `linux/${architecture}`;
      const root = fixture(t);
      const calls = [];
      const sourcePath = build({
        root,
        target,
        containerEngine: command,
        runCommand(invokedCommand, args, options) {
          calls.push({ command: invokedCommand, args, options });
          if (args[0] === 'info') return information(command, engineArchitecture);
          if (args[0] === 'run') writeArtifact(root, target);
          return '';
        },
      });

      assert.equal(readFileSync(sourcePath, 'utf8'), 'built server');
      assert.equal(readFileSync(join(root, 'Cargo.lock'), 'utf8'), 'original lockfile\n');
      assert.deepEqual(
        calls.map(({ command: invokedCommand, args }) => [invokedCommand, args[0]]),
        [
          [command, 'info'],
          [command, 'build'],
          [command, 'run'],
        ],
      );
      assert.deepEqual(calls[0].args, ['info', '--format', format]);
      assert.deepEqual(calls[0].options, {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
      });

      const image = `localhost/tandem-remote-builder:1.99.1-${architecture}-${target}`;
      const buildArguments = calls[1].args;
      assert.equal(buildArguments[buildArguments.indexOf('--platform') + 1], platform);
      assert.ok(buildArguments.includes('RUST_VERSION=1.99.1'));
      assert.ok(buildArguments.includes(`RUST_TARGET=${target}`));
      assert.ok(buildArguments.includes(`CROSS_COMPILER_PACKAGE=${compilerPackage}`));
      assert.equal(buildArguments[buildArguments.indexOf('--tag') + 1], image);
      assert.equal(buildArguments.at(-1), join(root, 'scripts'));
      assert.equal(
        buildArguments[buildArguments.indexOf('--file') + 1],
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
      assert.ok(run.includes(image));
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
}

for (const [state, failure] of [['healthy', undefined], ...unavailableFailures]) {
  test(`automatic selection uses ${state === 'healthy' ? 'Docker' : 'Podman'} when Docker is ${state}`, (t) => {
    const root = fixture(t);
    const calls = [];
    const selected = state === 'healthy' ? 'docker' : 'podman';
    build({
      root,
      runCommand(command, args) {
        calls.push([command, args[0]]);
        if (args[0] === 'info') {
          if (command === 'docker' && failure) throw failure();
          return information(command);
        }
        if (args[0] === 'run') writeArtifact(root);
        return '';
      },
    });
    assert.deepEqual(calls, [
      ['docker', 'info'],
      ...(selected === 'podman' ? [['podman', 'info']] : []),
      [selected, 'build'],
      [selected, 'run'],
    ]);
  });
}

for (const { command } of engines) {
  for (const source of ['option', 'environment']) {
    test(`${source} override selects only ${command} without starting or switching engines`, (t) => {
      const root = fixture(t);
      const calls = [];
      build({
        root,
        ...(source === 'option'
          ? {
              containerEngine: command,
              environment: { TANDEM_CONTAINER_ENGINE: command === 'docker' ? 'podman' : 'docker' },
            }
          : { environment: { TANDEM_CONTAINER_ENGINE: command } }),
        runCommand(invokedCommand, args) {
          calls.push([invokedCommand, args[0]]);
          if (args[0] === 'info') return information(command);
          if (args[0] === 'run') writeArtifact(root);
          return '';
        },
      });
      assert.deepEqual(calls, [
        [command, 'info'],
        [command, 'build'],
        [command, 'run'],
      ]);
    });
  }

  for (const [state, failure] of unavailableFailures) {
    test(`explicit ${command} ${state} fails without falling back`, (t) => {
      const root = fixture(t);
      const calls = [];
      const cause = failure();
      assert.throws(
        () =>
          build({
            root,
            containerEngine: command,
            runCommand(invokedCommand, args) {
              calls.push([invokedCommand, args[0]]);
              throw cause;
            },
          }),
        (error) => {
          assert.match(error.message, /requires a running Docker or Podman engine/);
          assert.match(error.message, /podman machine start/);
          assert.ok(error.cause instanceof AggregateError);
          assert.equal(error.cause.errors.length, 1);
          assert.equal(error.cause.errors[0].cause, cause);
          return true;
        },
      );
      assert.deepEqual(calls, [[command, 'info']]);
      assert.equal(existsSync(join(root, 'target')), false);
    });
  }
}

for (const [dockerState, dockerFailure] of unavailableFailures) {
  for (const [podmanState, podmanFailure] of unavailableFailures) {
    test(`unavailable engines report Docker ${dockerState} and Podman ${podmanState}`, (t) => {
      const root = fixture(t);
      const calls = [];
      assert.throws(
        () =>
          build({
            root,
            runCommand(command, args) {
              calls.push([command, args[0]]);
              throw command === 'docker' ? dockerFailure() : podmanFailure();
            },
          }),
        (error) => {
          assert.match(
            error.message,
            dockerState === 'missing' ? /Docker was not found on PATH/ : /Cannot connect to Docker/,
          );
          assert.match(
            error.message,
            podmanState === 'missing' ? /Podman was not found on PATH/ : /Cannot connect to Podman/,
          );
          assert.match(error.message, /TANDEM_CONTAINER_ENGINE=docker or podman/);
          assert.ok(error.cause instanceof AggregateError);
          assert.equal(error.cause.errors.length, 2);
          return true;
        },
      );
      assert.deepEqual(calls, [
        ['docker', 'info'],
        ['podman', 'info'],
      ]);
      assert.equal(existsSync(join(root, 'target')), false);
    });
  }
}

for (const containerEngine of ['', null, 'nerdctl', 'docker.exe']) {
  test(`rejects invalid container engine ${JSON.stringify(containerEngine)} before invoking commands`, (t) => {
    const root = fixture(t);
    assert.throws(
      () => build({ root, containerEngine, runCommand: rejectContainerCall }),
      /TANDEM_CONTAINER_ENGINE must be docker or podman/,
    );
    assert.equal(existsSync(join(root, 'target')), false);
  });
}

test('rejects unsupported targets and unpinned toolchains before invoking containers', (t) => {
  const root = fixture(t, 'stable');
  assert.throws(
    () => build({ root, target: 'x86_64-apple-darwin', runCommand: rejectContainerCall }),
    /Unsupported Linux remote target/,
  );
  assert.throws(
    () => build({ root, runCommand: rejectContainerCall }),
    /must specify an exact Rust version/,
  );
});

for (const { command, name } of engines) {
  for (const [description, output, expected] of [
    ['invalid JSON', 'invalid-json', new RegExp(`${name} returned invalid system information`)],
    ['null information', 'null', new RegExp(`${name} returned invalid system information`)],
    ['array information', '[]', new RegExp(`${name} returned invalid system information`)],
    [
      'unsupported architecture',
      information(command, 's390x'),
      new RegExp(`Unsupported ${name} engine architecture`),
    ],
    [
      'non-Linux containers',
      information(command, 'amd64', 'windows'),
      command === 'docker'
        ? /Switch Docker Desktop to Linux containers/
        : /Select a Linux Podman connection/,
    ],
    ['missing host fields', '{}', /requires Linux containers/],
  ]) {
    test(`rejects ${name} ${description} without trying another engine`, (t) => {
      const root = fixture(t);
      const calls = [];
      assert.throws(
        () =>
          build({
            root,
            runCommand(invokedCommand, args) {
              calls.push([invokedCommand, args[0]]);
              if (invokedCommand !== command) {
                throw Object.assign(new Error('not found'), { code: 'ENOENT' });
              }
              return output;
            },
          }),
        expected,
      );
      assert.deepEqual(calls, [
        ...(command === 'podman' ? [['docker', 'info']] : []),
        [command, 'info'],
      ]);
      assert.equal(existsSync(join(root, 'target')), false);
    });
  }

  test(`${name} builder preparation failure retains its cause and does not switch engines`, (t) => {
    const root = fixture(t);
    const calls = [];
    const cause = new Error('image build failed');
    assert.throws(
      () =>
        build({
          root,
          containerEngine: command,
          runCommand(invokedCommand, args) {
            calls.push([invokedCommand, args[0]]);
            if (args[0] === 'info') return information(command);
            throw cause;
          },
        }),
      (error) => {
        assert.match(error.message, new RegExp(`Could not prepare the ${name} builder`));
        assert.equal(error.cause, cause);
        return true;
      },
    );
    assert.deepEqual(calls, [
      [command, 'info'],
      [command, 'build'],
    ]);
    assert.equal(existsSync(join(root, 'target')), false);
  });

  for (const failure of ['container failure', 'missing output']) {
    test(`${name} ${failure} cleans up the incomplete staging directory`, (t) => {
      const root = fixture(t);
      const calls = [];
      const cause = new Error('cargo failed');
      assert.throws(
        () =>
          build({
            root,
            containerEngine: command,
            runCommand(invokedCommand, args) {
              calls.push([invokedCommand, args[0]]);
              if (args[0] === 'info') return information(command, 'aarch64');
              if (args[0] === 'run' && failure === 'container failure') throw cause;
              return '';
            },
          }),
        (error) => {
          assert.match(error.message, /Could not build the Linux SSH server/);
          assert.match(error.message, new RegExp(`Check the ${name} output above`));
          if (failure === 'container failure') assert.equal(error.cause, cause);
          else assert.match(error.cause.message, new RegExp(`${name} completed without writing`));
          return true;
        },
      );
      assert.deepEqual(calls, [
        [command, 'info'],
        [command, 'build'],
        [command, 'run'],
      ]);
      assert.deepEqual(readdirSync(join(root, 'target', 'remote-linux', defaultTarget)), []);
    });
  }
}
