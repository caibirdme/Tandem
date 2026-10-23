import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { buildRemoteArtifacts } from './build-remote.mjs';
import { stageArtifact, verifyArtifact } from './remote-artifacts.mjs';

const linux = 'x86_64-unknown-linux-gnu';
const mac = 'aarch64-apple-darwin';
const windows = 'x86_64-pc-windows-msvc';

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

function binary() {
  const bytes = Buffer.alloc(256);
  bytes.writeUInt32BE(0x7f454c46, 0);
  bytes[4] = 2;
  bytes[5] = 1;
  bytes[6] = 1;
  bytes.writeUInt16LE(3, 16);
  bytes.writeUInt16LE(62, 18);
  bytes.writeUInt32LE(1, 20);
  bytes.writeUInt16LE(64, 52);
  return bytes;
}

function prepared(root) {
  const source = join(root, 'ci-output', 'tandem-server');
  write(source, binary());
  return stageArtifact({ root, target: linux, source });
}

function snapshot(path) {
  const metadata = statSync(path, { bigint: true });
  return {
    mode: metadata.mode,
    modified: metadata.mtimeNs,
    contents: metadata.isDirectory()
      ? Object.fromEntries(
          readdirSync(path)
            .toSorted()
            .map((name) => [name, snapshot(join(path, name))]),
        )
      : readFileSync(path),
  };
}

function artifactSnapshot(root) {
  const directory = join(root, 'src-tauri', 'remote');
  return existsSync(directory) ? snapshot(directory) : undefined;
}

function changeSources(root) {
  write(join(root, 'crates/tandem-protocol/src/lib.rs'), 'pub fn changed() {}\n');
}

function corrupt(root) {
  const path = join(root, 'src-tauri', 'remote', linux, 'tandem-server');
  const bytes = readFileSync(path);
  bytes[200] = 42;
  writeFileSync(path, bytes);
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
        assert.deepEqual(args, [
          'build',
          '--release',
          '--locked',
          '-p',
          'tandem-server',
          '--target',
          linux,
        ]);
        write(join(targetDirectory, linux, 'release', 'tandem-server'), binary());
        return '';
      }
      if (command === 'cargo' && args[0] === 'metadata') {
        assert.deepEqual(args, ['metadata', '--format-version', '1', '--no-deps', '--locked']);
        return JSON.stringify({ target_directory: targetDirectory });
      }
      assert.fail(`Unexpected command: ${command} ${args.join(' ')}`);
    },
    buildLinux({ root: requestedRoot, target }) {
      assert.equal(requestedRoot, root);
      assert.equal(target, linux);
      linuxBuilds.push(target);
      const source = join(root, 'docker-output', 'tandem-server');
      write(source, binary());
      return source;
    },
  };
}

for (const host of [mac, 'x86_64-apple-darwin', windows]) {
  test(`${host} prepares only Linux x64 through Docker`, (context) => {
    const root = fixture(context);
    const build = builders(root, host);
    buildRemoteArtifacts(build);
    assert.deepEqual(build.linuxBuilds, [linux]);
    assert.deepEqual(build.commands, []);
    assert.deepEqual(readdirSync(join(root, 'src-tauri', 'remote')), [linux]);
    assert.ok(verifyArtifact({ root, target: linux }));
    assert.equal(existsSync(join(root, 'docker-output', 'tandem-server')), false);
  });
}

test('native Linux x64 uses Cargo and honors its target directory', (context) => {
  const root = fixture(context);
  const build = builders(root, linux);
  buildRemoteArtifacts(build);
  assert.deepEqual(build.linuxBuilds, []);
  assert.deepEqual(
    build.commands.map(([, args]) => args[0]),
    ['build', 'metadata'],
  );
  assert.equal(existsSync(join(root, 'target')), false);
  assert.ok(existsSync(join(root, 'custom-cargo-target', linux, 'release', 'tandem-server')));
  assert.ok(verifyArtifact({ root, target: linux }));
});

for (const checkOnly of [false, true]) {
  test(`valid Linux cache is reused without host discovery or builds (checkOnly=${checkOnly})`, (context) => {
    const root = fixture(context);
    prepared(root);
    const before = artifactSnapshot(root);
    const build = builders(root);
    buildRemoteArtifacts({ ...build, host: undefined, checkOnly });
    assert.deepEqual(build.commands, []);
    assert.deepEqual(build.linuxBuilds, []);
    assert.deepEqual(artifactSnapshot(root), before);
  });
}

test('protocol and runtime source changes rebuild the Linux artifact', async (context) => {
  await Promise.all(
    ['tandem-protocol', 'copilot-runtime'].map((crate) =>
      context.test(crate, (subcontext) => {
        const root = fixture(subcontext);
        prepared(root);
        write(join(root, `crates/${crate}/src/lib.rs`), 'pub fn changed() {}\n');
        const build = builders(root);
        buildRemoteArtifacts(build);
        assert.deepEqual(build.linuxBuilds, [linux]);
        assert.deepEqual(build.commands, []);
        assert.ok(verifyArtifact({ root, target: linux }));
      }),
    ),
  );
});

test('other staged target directories are ignored during preparation and checks', (context) => {
  const root = fixture(context);
  const extra = join(root, 'src-tauri', 'remote', mac);
  write(join(extra, 'tandem-server'), 'old unsupported binary');
  write(join(extra, 'artifact.json'), '{"sourceHash":"stale"}');
  mkdirSync(join(root, 'src-tauri', 'remote', 'unknown-target'));
  const before = snapshot(extra);
  const build = builders(root);
  buildRemoteArtifacts(build);
  assert.deepEqual(build.linuxBuilds, [linux]);
  assert.deepEqual(build.commands, []);
  assert.deepEqual(snapshot(extra), before);
  const allArtifacts = artifactSnapshot(root);
  const check = builders(root, windows);
  buildRemoteArtifacts({ ...check, checkOnly: true });
  assert.deepEqual(check.commands, []);
  assert.deepEqual(check.linuxBuilds, []);
  assert.deepEqual(artifactSnapshot(root), allArtifacts);
});

test('Linux failures and missing outputs stop preparation', (context) => {
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
          changeSources(root);
          return path;
        },
      }),
    /sources changed during compilation/,
  );
  assert.equal(existsSync(join(root, 'src-tauri/remote')), false);
  assert.deepEqual(build.commands, []);
});

test('check failures provide preparation commands without changing artifacts or invoking tools', async (context) => {
  const cases = [
    { name: 'missing', prepare() {}, reason: /Missing tandem-server artifact/ },
    {
      name: 'stale',
      prepare(root) {
        prepared(root);
        changeSources(root);
      },
      reason: /Stale tandem-server artifact/,
    },
    {
      name: 'corrupt',
      prepare(root) {
        prepared(root);
        corrupt(root);
      },
      reason: /SHA-256 does not match/,
    },
    {
      name: 'invalid provenance',
      prepare(root) {
        prepared(root);
        write(join(root, 'src-tauri', 'remote', linux, 'artifact.json'), '{');
      },
      reason: /Cannot read artifact provenance/,
    },
  ];
  await Promise.all(
    cases.map(({ name, prepare, reason }) =>
      context.test(name, (subcontext) => {
        const root = fixture(subcontext);
        prepare(root);
        const before = artifactSnapshot(root);
        const build = builders(root);
        assert.throws(
          () => buildRemoteArtifacts({ ...build, host: undefined, checkOnly: true }),
          (error) => {
            assert.match(error.message, /Remote artifact check failed/);
            assert.ok(error.message.includes(linux));
            assert.match(error.message, reason);
            assert.match(error.message, /just desktop-build-full/);
            assert.match(error.message, /just remote-build/);
            assert.match(error.cause.message, reason);
            return true;
          },
        );
        assert.deepEqual(build.commands, []);
        assert.deepEqual(build.linuxBuilds, []);
        assert.deepEqual(artifactSnapshot(root), before);
      }),
    ),
  );
});

test('CLI rejects positional targets and unknown flags before invoking build tools', (context) => {
  const root = fixture(context);
  const script = fileURLToPath(new URL('./build-remote.mjs', import.meta.url));
  for (const args of [[linux], ['--unknown'], ['--check', mac]]) {
    const result = spawnSync(process.execPath, [script, ...args], {
      cwd: root,
      encoding: 'utf8',
      env: { ...process.env, PATH: '' },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Unexpected arguments/);
    assert.match(result.stderr, /Usage: node scripts\/build-remote.mjs \[--check\]/);
    assert.equal(result.stdout, '');
  }
});
