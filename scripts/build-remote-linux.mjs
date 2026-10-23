import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const platforms = new Map([
  ['x86_64-unknown-linux-gnu', 'linux/amd64'],
  ['aarch64-unknown-linux-gnu', 'linux/arm64'],
]);

function bindMount(source, destination, readonly = false) {
  const field = `source=${source}`;
  const escaped = /[,"\n\r]/.test(field) ? `"${field.replaceAll('"', '""')}"` : field;
  return `type=bind,${escaped},target=${destination}${readonly ? ',readonly' : ''}`;
}

export function buildLinuxArtifact({
  root = repositoryRoot,
  target = 'x86_64-unknown-linux-gnu',
  runCommand = execFileSync,
} = {}) {
  const platform = platforms.get(target);
  if (!platform) {
    throw new Error(
      `Unsupported Linux remote target: ${target}. Use ${[...platforms.keys()].join(' or ')}.`,
    );
  }

  const sourceRoot = resolve(root);
  const toolchain = readFileSync(join(sourceRoot, 'rust-toolchain.toml'), 'utf8');
  const rustVersion = toolchain.match(/^\s*channel\s*=\s*"(\d+\.\d+\.\d+)"\s*$/m)?.[1];
  if (!rustVersion) {
    throw new Error(
      'rust-toolchain.toml must specify an exact Rust version such as channel = "1.99.1".',
    );
  }

  let operatingSystem;
  try {
    operatingSystem = runCommand('docker', ['info', '--format', '{{.OSType}}'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
  } catch (error) {
    if (error.code === 'ENOENT') {
      throw new Error(
        'Docker is required to build the Linux SSH server. Install Docker Desktop (https://docs.docker.com/get-docker/), start it, and retry the build.',
        { cause: error },
      );
    }
    throw new Error(
      'Cannot connect to Docker. Start Docker Desktop or your Docker Engine, confirm that docker info succeeds, and retry the build.',
      { cause: error },
    );
  }
  if (operatingSystem !== 'linux') {
    throw new Error(
      'The Linux SSH server requires Linux containers. Switch Docker Desktop to Linux containers and retry the build.',
    );
  }

  const architecture = platform.split('/')[1];
  const image = `tandem-remote-builder:${rustVersion}-${architecture}`;
  console.info(`Preparing the ${platform} SSH server builder with Rust ${rustVersion}.`);
  try {
    runCommand(
      'docker',
      [
        'build',
        '--platform',
        platform,
        '--build-arg',
        `RUST_VERSION=${rustVersion}`,
        '--tag',
        image,
        '--file',
        join(sourceRoot, 'scripts', 'remote-linux.Dockerfile'),
        join(sourceRoot, 'scripts'),
      ],
      { stdio: 'inherit' },
    );
  } catch (error) {
    throw new Error(
      `Could not prepare the Docker builder for ${platform}. Check the Docker output above. When building for another architecture, use Docker Desktop or configure QEMU emulation in Docker Engine.`,
      { cause: error },
    );
  }

  const outputParent = join(sourceRoot, 'target', 'remote-linux', target);
  mkdirSync(outputParent, { recursive: true });
  const output = mkdtempSync(join(outputParent, 'build-'));
  const sourcePath = join(output, 'tandem-server');
  const repositoryId = createHash('sha256').update(sourceRoot).digest('hex').slice(0, 12);
  console.info(`Building tandem-server for ${target} in Docker.`);
  try {
    runCommand(
      'docker',
      [
        'run',
        '--rm',
        '--platform',
        platform,
        '--mount',
        bindMount(sourceRoot, '/workspace', true),
        '--mount',
        `type=volume,source=tandem-remote-${target}-cargo,target=/cargo-cache`,
        '--mount',
        `type=volume,source=tandem-remote-${repositoryId}-${target}-target,target=/build-target`,
        '--mount',
        bindMount(output, '/output'),
        image,
        'sh',
        '-eu',
        '-c',
        'cargo build --release --locked --package tandem-server --target "$1"; install -m 755 "$CARGO_TARGET_DIR/$1/release/tandem-server" /output/tandem-server',
        '--',
        target,
      ],
      { stdio: 'inherit' },
    );
    if (!existsSync(sourcePath)) {
      throw new Error(`Docker completed without writing ${sourcePath}.`);
    }
  } catch (error) {
    rmSync(output, { recursive: true, force: true });
    throw new Error(
      `Could not build the Linux SSH server for ${target}. Check the Docker output above.`,
      {
        cause: error,
      },
    );
  }
  return sourcePath;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const sourcePath = buildLinuxArtifact({ target: process.argv[2] });
    console.info(`Built Linux SSH server: ${sourcePath}`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
