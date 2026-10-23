import assert from 'node:assert/strict';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  truncateSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { test } from 'node:test';

import {
  REMOTE_TARGETS,
  sourceFingerprint,
  stageArtifact,
  verifyArtifact,
} from './remote-artifacts.mjs';

const linuxTarget = 'x86_64-unknown-linux-gnu';
const sourceFixture = {
  'Cargo.toml': '[workspace]\nmembers = []\n',
  'Cargo.lock': 'version = 4\n',
  'rust-toolchain.toml': '[toolchain]\nchannel = "1.99.1"\n',
  'crates/copilot-runtime/Cargo.toml': '[package]\nname = "copilot-runtime"\n',
  'crates/copilot-runtime/src/lib.rs': 'pub fn start() {}\n',
  'crates/copilot-runtime/src/session/mod.rs': 'pub fn session() {}\n',
  'crates/tandem-protocol/Cargo.toml': '[package]\nname = "tandem-protocol"\n',
  'crates/tandem-protocol/src/lib.rs': 'pub const PROTOCOL: u32 = 1;\n',
  'crates/tandem-server/Cargo.toml': '[package]\nname = "tandem-server"\n',
  'crates/tandem-server/build.rs': 'fn main() {}\n',
  'crates/tandem-server/src/main.rs': 'fn main() {}\n',
};

function write(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}

function fixture(context, entries = Object.entries(sourceFixture)) {
  const root = mkdtempSync(join(tmpdir(), 'tandem-remote-artifacts-'));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  for (const [path, content] of entries) write(join(root, path), content);
  return root;
}

function executable(target = linuxTarget) {
  const binary = Buffer.alloc(256);
  if (target.endsWith('linux-gnu')) {
    binary.writeUInt32BE(0x7f454c46, 0);
    binary[4] = 2;
    binary[5] = 1;
    binary[6] = 1;
    binary.writeUInt16LE(3, 16);
    binary.writeUInt16LE(target.startsWith('x86_64') ? 62 : 183, 18);
    binary.writeUInt32LE(1, 20);
    binary.writeUInt16LE(64, 52);
  } else {
    binary.writeUInt32LE(0xfeedfacf, 0);
    binary.writeUInt32LE(target.startsWith('x86_64') ? 0x01000007 : 0x0100000c, 4);
    binary.writeUInt32LE(2, 12);
  }
  return binary;
}

function stage(root, target = linuxTarget, directory) {
  const source = join(root, 'build', target, 'tandem-server');
  write(source, executable(target));
  return stageArtifact({ root, target, source, directory });
}

test('source fingerprint is deterministic across root paths and file creation order', (context) => {
  const first = fixture(context);
  const second = fixture(context, Object.entries(sourceFixture).toReversed());
  assert.match(sourceFingerprint(first), /^[a-f0-9]{64}$/);
  assert.equal(sourceFingerprint(first), sourceFingerprint(second));
  chmodSync(join(second, 'crates/tandem-server/src/main.rs'), 0o700);
  assert.equal(sourceFingerprint(first), sourceFingerprint(second));
});

test('every remote source input contributes to the fingerprint', async (context) => {
  const checks = [];
  for (const path of Object.keys(sourceFixture)) {
    checks.push(
      context.test(path, (subcontext) => {
        const root = fixture(subcontext);
        const before = sourceFingerprint(root);
        write(join(root, path), `${sourceFixture[path]}\n// changed\n`);
        assert.notEqual(sourceFingerprint(root), before);
      }),
    );
  }
  await Promise.all(checks);
});

test('source paths and new build scripts contribute to the fingerprint', (context) => {
  const root = fixture(context);
  const before = sourceFingerprint(root);
  renameSync(
    join(root, 'crates/copilot-runtime/src/lib.rs'),
    join(root, 'crates/copilot-runtime/src/renamed.rs'),
  );
  assert.notEqual(sourceFingerprint(root), before);
  const renamed = sourceFingerprint(root);
  write(join(root, 'crates/copilot-runtime/build.rs'), 'fn main() {}\n');
  assert.notEqual(sourceFingerprint(root), renamed);
});

test('the remote fingerprint does not include the development host or local services', (context) => {
  const root = fixture(context);
  const before = sourceFingerprint(root);
  for (const name of ['tandem-dev-server', 'tandem-core', 'tandem-remote']) {
    write(join(root, `crates/${name}/Cargo.toml`), '[package]\nversion = "2.0.0"\n');
    write(join(root, `crates/${name}/src/main.rs`), 'fn main() {}\n');
  }
  write(join(root, 'src/App.tsx'), 'export default null;\n');
  write(join(root, 'target/release/tandem-dev-server'), executable());
  assert.equal(sourceFingerprint(root), before);
});

test('stages and verifies supported executable targets with current provenance', async (context) => {
  assert.deepEqual(REMOTE_TARGETS, [linuxTarget, 'aarch64-apple-darwin']);
  const checks = [];
  for (const target of [
    linuxTarget,
    'aarch64-unknown-linux-gnu',
    'x86_64-apple-darwin',
    'aarch64-apple-darwin',
  ]) {
    checks.push(
      context.test(target, (subcontext) => {
        const root = fixture(subcontext);
        const path = stage(root, target);
        assert.equal(verifyArtifact({ root, target }), path);
        assert.deepEqual(readFileSync(path), executable(target));
        const metadata = JSON.parse(readFileSync(join(dirname(path), 'artifact.json'), 'utf8'));
        assert.equal(metadata.schemaVersion, 1);
        assert.equal(metadata.target, target);
        assert.equal(metadata.sourceHash, sourceFingerprint(root));
        assert.match(metadata.sha256, /^[a-f0-9]{64}$/);
        if (process.platform !== 'win32') assert.equal(statSync(path).mode & 0o777, 0o700);
        assert.deepEqual(readdirSync(dirname(path)).toSorted(), ['artifact.json', 'tandem-server']);
      }),
    );
  }
  await Promise.all(checks);
});

test('supports a separate artifact directory and replacing an existing artifact', (context) => {
  const root = fixture(context);
  const directory = join(root, 'custom-artifacts');
  const path = stage(root, linuxTarget, directory);
  write(join(root, 'crates/tandem-server/src/main.rs'), 'fn main() { println!("new"); }\n');
  assert.equal(stage(root, linuxTarget, directory), path);
  assert.equal(verifyArtifact({ root, target: linuxTarget, directory }), path);
  assert.equal(existsSync(join(root, 'src-tauri/remote')), false);
});

test('rejects stale artifacts even if the package version and protocol are unchanged', (context) => {
  const root = fixture(context);
  stage(root);
  write(join(root, 'crates/copilot-runtime/src/lib.rs'), 'pub fn start() { println!("new"); }\n');
  assert.throws(
    () => verifyArtifact({ root, target: linuxTarget }),
    /Stale tandem-server artifact.*sources have changed/,
  );
});

test('rejects a changed executable payload with intact target headers', (context) => {
  const root = fixture(context);
  const path = stage(root);
  const binary = readFileSync(path);
  binary[200] = 42;
  writeFileSync(path, binary);
  assert.throws(() => verifyArtifact({ root, target: linuxTarget }), /SHA-256 does not match/);
});

test('rejects missing executable and missing or invalid provenance', async (context) => {
  const cases = [
    ['missing executable', (path) => rmSync(path), /Missing tandem-server artifact/],
    [
      'missing provenance',
      (path) => rmSync(join(dirname(path), 'artifact.json')),
      /Missing artifact provenance/,
    ],
    [
      'invalid JSON',
      (path) => write(join(dirname(path), 'artifact.json'), '{'),
      /Cannot read artifact provenance/,
    ],
    [
      'wrong target',
      (path) => {
        const metadataPath = join(dirname(path), 'artifact.json');
        const metadata = JSON.parse(readFileSync(metadataPath, 'utf8'));
        write(metadataPath, JSON.stringify({ ...metadata, target: 'aarch64-apple-darwin' }));
      },
      /Invalid artifact provenance/,
    ],
    [
      'wrong schema',
      (path) => {
        const metadataPath = join(dirname(path), 'artifact.json');
        const metadata = JSON.parse(readFileSync(metadataPath, 'utf8'));
        write(metadataPath, JSON.stringify({ ...metadata, schemaVersion: 0 }));
      },
      /Invalid artifact provenance/,
    ],
  ];
  const checks = [];
  for (const [name, change, expected] of cases) {
    checks.push(
      context.test(name, (subcontext) => {
        const root = fixture(subcontext);
        const path = stage(root);
        change(path);
        assert.throws(() => verifyArtifact({ root, target: linuxTarget }), expected);
      }),
    );
  }
  await Promise.all(checks);
});

test('rejects empty, truncated, corrupt, wrong-format, and wrong-CPU executables before staging', async (context) => {
  const cases = [
    ['empty', linuxTarget, Buffer.alloc(0)],
    ['truncated ELF', linuxTarget, executable().subarray(0, 63)],
    ['corrupt ELF magic', linuxTarget, Buffer.alloc(256)],
    ['wrong ELF CPU', linuxTarget, executable('aarch64-unknown-linux-gnu')],
    ['wrong file format', linuxTarget, executable('aarch64-apple-darwin')],
    [
      'truncated Mach-O',
      'aarch64-apple-darwin',
      executable('aarch64-apple-darwin').subarray(0, 31),
    ],
    ['wrong Mach-O CPU', 'aarch64-apple-darwin', executable('x86_64-apple-darwin')],
  ];
  const checks = [];
  for (const [name, target, binary] of cases) {
    checks.push(
      context.test(name, (subcontext) => {
        const root = fixture(subcontext);
        const source = join(root, 'tandem-server');
        write(source, binary);
        assert.throws(
          () => stageArtifact({ root, target, source }),
          /Invalid tandem-server artifact/,
        );
        assert.equal(existsSync(join(root, 'src-tauri/remote')), false);
      }),
    );
  }
  await Promise.all(checks);
});

test('rejects target paths outside the recognized targets', (context) => {
  const root = fixture(context);
  assert.throws(() => verifyArtifact({ root, target: '../outside' }), /Unsupported remote target/);
});

test('rejects oversized artifacts before loading their contents', (context) => {
  const root = fixture(context);
  const path = stage(root);
  truncateSync(path, 512 * 1024 * 1024 + 1);
  assert.throws(() => verifyArtifact({ root, target: linuxTarget }), /512 MiB remote upload limit/);
  assert.throws(
    () => stageArtifact({ root, target: linuxTarget, source: path }),
    /512 MiB remote upload limit/,
  );
});
