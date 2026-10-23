import assert from 'node:assert/strict';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';

import { buildRemoteArtifacts, defaultTargets } from './build-remote.mjs';
import { stageArtifact, verifyArtifact } from './remote-artifacts.mjs';

const linux = 'x86_64-unknown-linux-gnu';
const linuxArm = 'aarch64-unknown-linux-gnu';
const mac = 'aarch64-apple-darwin';
const macIntel = 'x86_64-apple-darwin';

function write(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

function fixture(context) {
  const root = mkdtempSync(join(tmpdir(), 'tandem-build-remote-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  write(join(root, 'Cargo.toml'), '[workspace]\n');
  write(join(root, 'Cargo.lock'), 'version = 4\n');
  write(join(root, 'rust-toolchain.toml'), '[toolchain]\nchannel = "1.99.1"\n');
  for (const crate of ['copilot-runtime', 'tandem-protocol', 'tandem-server']) {
    write(join(root, `crates/${crate}/Cargo.toml`), `[package]\nname = "${crate}"\n`);
    write(join(root, `crates/${crate}/src/lib.rs`), 'pub fn start() {}\n');
  }
  return root;
}

function binary(target) {
  const bytes = Buffer.alloc(256);
  if (target.endsWith('linux-gnu')) {
    bytes.writeUInt32BE(0x7f454c46, 0);
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes.writeUInt16LE(3, 16);
    bytes.writeUInt16LE(target === linux ? 62 : 183, 18);
    bytes.writeUInt32LE(1, 20);
    bytes.writeUInt16LE(64, 52);
  } else {
    bytes.writeUInt32LE(0xfeedfacf, 0);
    bytes.writeUInt32LE(target === mac ? 0x0100000c : 0x01000007, 4);
    bytes.writeUInt32LE(2, 12);
  }
  return bytes;
}

function prepared(root, target) {
  const source = join(root, 'ci-output', target, 'tandem-server');
  write(source, binary(target));
  return stageArtifact({ root, target, source });
}

function builders(root, host = mac) {
  const commands = [];
  const linuxBuilds = [];
  const targetDirectory = join(root, 'custom-cargo-target');
  return {
    root,
    host,
    commands,
    linuxBuilds,
    run(command, args, options) {
      commands.push([command, args]);
      assert.equal(options.cwd, root);
      if (command === 'cargo' && args[0] === 'build') {
        const target = args[args.indexOf('--target') + 1];
        assert.deepEqual(args, [
          'build',
          '--release',
          '--locked',
          '-p',
          'tandem-server',
          '--target',
          target,
        ]);
        write(join(targetDirectory, target, 'release', 'tandem-server'), binary(target));
        return '';
      }
      if (command === 'cargo' && args[0] === 'metadata') {
        return JSON.stringify({ target_directory: targetDirectory });
      }
      assert.fail(`Unexpected command: ${command} ${args.join(' ')}`);
    },
    buildLinux({ root: requestedRoot, target }) {
      assert.equal(requestedRoot, root);
      linuxBuilds.push(target);
      const source = join(root, 'docker-output', target, 'tandem-server');
      write(source, binary(target));
      return source;
    },
  };
}

test('Mac defaults build Linux plus the native server using the Cargo target directory', (context) => {
  assert.deepEqual(defaultTargets(mac), [linux, mac]);
  assert.deepEqual(defaultTargets(macIntel), [linux, macIntel]);
  assert.deepEqual(defaultTargets(linux), [linux]);
  assert.deepEqual(defaultTargets('x86_64-pc-windows-msvc'), [linux]);
  const root = fixture(context);
  const build = builders(root);
  buildRemoteArtifacts(build);
  assert.deepEqual(build.linuxBuilds, [linux]);
  assert.deepEqual(
    build.commands.map(([, args]) => args[0]),
    ['build', 'metadata'],
  );
  assert.equal(existsSync(join(root, 'target')), false);
  for (const target of [linux, mac]) assert.ok(verifyArtifact({ root, target }));
});

test('verified CI artifacts are reused without compilation or Docker on a different host', (context) => {
  const root = fixture(context);
  for (const target of [linux, mac]) prepared(root, target);
  const build = builders(root, 'x86_64-pc-windows-msvc');
  buildRemoteArtifacts(build);
  assert.deepEqual(build.commands, []);
  assert.deepEqual(build.linuxBuilds, []);
});

test('protocol and runtime source changes rebuild both Linux and native artifacts', async (context) => {
  await Promise.all(
    ['tandem-protocol', 'copilot-runtime'].map((crate) =>
      context.test(crate, (subcontext) => {
        const root = fixture(subcontext);
        for (const target of [linux, mac]) prepared(root, target);
        write(join(root, `crates/${crate}/src/lib.rs`), 'pub fn changed() {}\n');
        const build = builders(root);
        buildRemoteArtifacts(build);
        assert.deepEqual(build.linuxBuilds, [linux]);
        assert.equal(build.commands.filter(([, args]) => args[0] === 'build').length, 1);
        for (const target of [linux, mac]) assert.ok(verifyArtifact({ root, target }));
      }),
    ),
  );
});

test('a stale Mac artifact blocks builds on a host that cannot rebuild it', (context) => {
  const root = fixture(context);
  prepared(root, macIntel);
  write(join(root, 'crates/tandem-server/src/lib.rs'), 'pub fn changed() {}\n');
  const build = builders(root);
  assert.throws(
    () => buildRemoteArtifacts({ ...build, targets: [macIntel] }),
    /matching Mac.*prepare-remote/,
  );
  assert.deepEqual(build.commands, []);
  assert.deepEqual(build.linuxBuilds, []);
  assert.throws(() => verifyArtifact({ root, target: macIntel }), /Stale tandem-server artifact/);
});

test('Linux failures and missing outputs stop preparation before native compilation', (context) => {
  const root = fixture(context);
  const build = builders(root);
  const failure = new Error('Docker build failed');
  assert.throws(
    () =>
      buildRemoteArtifacts({
        ...build,
        buildLinux() {
          throw failure;
        },
      }),
    (error) => error === failure,
  );
  assert.throws(
    () =>
      buildRemoteArtifacts({
        ...build,
        buildLinux() {
          return join(root, 'missing-server');
        },
      }),
    /Missing tandem-server artifact/,
  );
  assert.deepEqual(build.commands, []);
  assert.equal(existsSync(join(root, 'src-tauri/remote')), false);
});

test('source changes during compilation prevent staging the resulting binary', (context) => {
  const root = fixture(context);
  const build = builders(root);
  assert.throws(
    () =>
      buildRemoteArtifacts({
        ...build,
        buildLinux(options) {
          const path = build.buildLinux(options);
          write(join(root, 'crates/tandem-server/src/lib.rs'), 'pub fn changed() {}\n');
          return path;
        },
      }),
    /sources changed during compilation/,
  );
  assert.equal(existsSync(join(root, 'src-tauri/remote')), false);
  assert.deepEqual(build.commands, []);
});

test('additional staged targets are refreshed even when omitted from the requested targets', (context) => {
  const root = fixture(context);
  for (const target of [linux, mac, linuxArm]) prepared(root, target);
  write(join(root, 'crates/tandem-protocol/src/lib.rs'), 'pub fn changed() {}\n');
  const build = builders(root);
  buildRemoteArtifacts(build);
  assert.deepEqual(build.linuxBuilds, [linux, linuxArm]);
  for (const target of [linux, mac, linuxArm]) assert.ok(verifyArtifact({ root, target }));
});

test('unsupported requested or already-staged targets fail before any build command', (context) => {
  const root = fixture(context);
  const build = builders(root);
  assert.throws(
    () => buildRemoteArtifacts({ ...build, targets: ['../unsupported'] }),
    /Unsupported remote artifact target/,
  );
  mkdirSync(join(root, 'src-tauri/remote/unsupported'), { recursive: true });
  assert.throws(() => buildRemoteArtifacts(build), /Unsupported remote artifact target/);
  assert.deepEqual(build.commands, []);
  assert.deepEqual(build.linuxBuilds, []);
});
