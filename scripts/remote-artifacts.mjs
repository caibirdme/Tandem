import { createHash, randomUUID } from 'node:crypto';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { join, resolve } from 'node:path';

export const REMOTE_TARGETS = ['x86_64-unknown-linux-gnu'];

const schemaVersion = 1;
const maxArtifactBytes = 512 * 1024 * 1024;
const remoteCrates = ['copilot-runtime', 'tandem-protocol', 'tandem-server'];
const executableTargets = {
  'x86_64-unknown-linux-gnu': { format: 'elf', cpu: 62 },
  'aarch64-unknown-linux-gnu': { format: 'elf', cpu: 183 },
  'x86_64-apple-darwin': { format: 'macho', cpu: 0x01000007 },
  'aarch64-apple-darwin': { format: 'macho', cpu: 0x0100000c },
};

function sourceFiles(root, relativeDirectory) {
  return readdirSync(join(root, relativeDirectory), { withFileTypes: true }).flatMap((entry) => {
    const relativePath = `${relativeDirectory}/${entry.name}`;
    if (entry.isDirectory()) return sourceFiles(root, relativePath);
    if (entry.isFile()) return [relativePath];
    throw new Error(`Unsupported remote source entry: ${relativePath}. Use regular source files.`);
  });
}

export function sourceFingerprint(root) {
  const files = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'];
  for (const crate of remoteCrates) {
    const directory = `crates/${crate}`;
    files.push(`${directory}/Cargo.toml`, ...sourceFiles(root, `${directory}/src`));
    if (existsSync(join(root, directory, 'build.rs'))) files.push(`${directory}/build.rs`);
  }
  const hash = createHash('sha256').update('tandem-remote-source-v1\0');
  for (const relativePath of files.toSorted()) {
    const content = readFileSync(join(root, relativePath));
    hash.update(`${relativePath}\0${content.length}\0`).update(content);
  }
  return hash.digest('hex');
}

function validateTarget(target) {
  if (!Object.hasOwn(executableTargets, target)) {
    throw new Error(
      `Unsupported remote target ${target}. Supported targets: ${Object.keys(executableTargets).join(', ')}.`,
    );
  }
  return executableTargets[target];
}

function validateBinary(binary, target, path) {
  const expected = validateTarget(target);
  const invalid = () => {
    throw new Error(
      `Invalid tandem-server artifact at ${path}: expected a complete 64-bit ${expected.format === 'elf' ? 'ELF' : 'Mach-O'} executable for ${target}. Rebuild the server for this target.`,
    );
  };
  if (expected.format === 'elf') {
    if (
      binary.length < 64 ||
      binary.readUInt32BE(0) !== 0x7f454c46 ||
      binary[4] !== 2 ||
      binary[5] !== 1 ||
      binary[6] !== 1 ||
      ![2, 3].includes(binary.readUInt16LE(16)) ||
      binary.readUInt16LE(18) !== expected.cpu ||
      binary.readUInt32LE(20) !== 1 ||
      binary.readUInt16LE(52) !== 64
    )
      invalid();
  } else if (
    binary.length < 32 ||
    binary.readUInt32LE(0) !== 0xfeedfacf ||
    binary.readUInt32LE(4) !== expected.cpu ||
    binary.readUInt32LE(12) !== 2
  )
    invalid();
}

function readBinary(path, target) {
  const metadata = existsSync(path) ? statSync(path) : undefined;
  if (!metadata?.isFile()) {
    throw new Error(
      `Missing tandem-server artifact at ${path}. Build the server for ${target} and prepare its remote artifact.`,
    );
  }
  if (metadata.size > maxArtifactBytes) {
    throw new Error(
      `Invalid tandem-server artifact at ${path}: size exceeds the 512 MiB remote upload limit. Rebuild the server for ${target}.`,
    );
  }
  const binary = readFileSync(path);
  validateBinary(binary, target, path);
  return binary;
}

function binaryHash(binary) {
  return createHash('sha256').update(binary).digest('hex');
}

function writeAtomic(path, content, mode) {
  const temporary = `${path}.${randomUUID()}.tmp`;
  try {
    writeFileSync(temporary, content, { flag: 'wx', mode });
    chmodSync(temporary, mode);
    renameSync(temporary, path);
  } finally {
    rmSync(temporary, { force: true });
  }
}

export function stageArtifact({
  root,
  target,
  source,
  directory = join(root, 'src-tauri', 'remote'),
}) {
  validateTarget(target);
  const binary = readBinary(source, target);
  const metadata = {
    schemaVersion,
    target,
    sourceHash: sourceFingerprint(root),
    sha256: binaryHash(binary),
  };
  const targetDirectory = resolve(directory, target);
  mkdirSync(targetDirectory, { recursive: true });
  const destination = join(targetDirectory, 'tandem-server');
  writeAtomic(destination, binary, 0o700);
  writeAtomic(
    join(targetDirectory, 'artifact.json'),
    `${JSON.stringify(metadata, null, 2)}\n`,
    0o600,
  );
  return destination;
}

export function verifyArtifact({ root, target, directory = join(root, 'src-tauri', 'remote') }) {
  validateTarget(target);
  const targetDirectory = resolve(directory, target);
  const path = join(targetDirectory, 'tandem-server');
  const binary = readBinary(path, target);
  const metadataPath = join(targetDirectory, 'artifact.json');
  if (!existsSync(metadataPath)) {
    throw new Error(
      `Missing artifact provenance at ${metadataPath}. Rebuild and prepare the ${target} remote artifact.`,
    );
  }
  let metadata;
  try {
    metadata = JSON.parse(readFileSync(metadataPath, 'utf8'));
  } catch (cause) {
    throw new Error(
      `Cannot read artifact provenance at ${metadataPath}. Rebuild and prepare the ${target} remote artifact.`,
      { cause },
    );
  }
  if (
    !metadata ||
    metadata.schemaVersion !== schemaVersion ||
    metadata.target !== target ||
    !/^[a-f0-9]{64}$/.test(metadata.sourceHash) ||
    !/^[a-f0-9]{64}$/.test(metadata.sha256)
  ) {
    throw new Error(
      `Invalid artifact provenance at ${metadataPath}. Rebuild and prepare the ${target} remote artifact.`,
    );
  }
  if (metadata.sourceHash !== sourceFingerprint(root)) {
    throw new Error(
      `Stale tandem-server artifact for ${target}: remote server sources have changed. Rebuild and prepare the remote artifact.`,
    );
  }
  if (metadata.sha256 !== binaryHash(binary)) {
    throw new Error(
      `Corrupt tandem-server artifact at ${path}: its SHA-256 does not match artifact.json. Rebuild and prepare the remote artifact.`,
    );
  }
  return path;
}
