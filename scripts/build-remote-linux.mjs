import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const platforms = new Map([
  [
    'x86_64-unknown-linux-gnu',
    {
      architecture: 'amd64',
      compiler: 'x86_64-linux-gnu',
      package: 'g++-x86-64-linux-gnu',
    },
  ],
  [
    'aarch64-unknown-linux-gnu',
    {
      architecture: 'arm64',
      compiler: 'aarch64-linux-gnu',
      package: 'g++-aarch64-linux-gnu',
    },
  ],
]);
const engineArchitectures = new Map([
  ['x86_64', 'amd64'],
  ['amd64', 'amd64'],
  ['aarch64', 'arm64'],
  ['arm64', 'arm64'],
]);

function bindMount(source, destination, readonly = false) {
  const field = `source=${source}`;
  const escaped = /[,"\n\r]/.test(field) ? `"${field.replaceAll('"', '""')}"` : field;
  return `type=bind,${escaped},target=${destination}${readonly ? ',readonly' : ''}`;
}

function selectContainerEngine(runCommand, requestedEngine) {
  if (requestedEngine !== undefined && !['docker', 'podman'].includes(requestedEngine)) {
    throw new Error('TANDEM_CONTAINER_ENGINE must be docker or podman.');
  }
  const failures = [];
  const candidates = requestedEngine ? [requestedEngine] : ['docker', 'podman'];
  for (const command of candidates) {
    const name = command === 'podman' ? 'Podman' : 'Docker';
    const format = command === 'podman' ? 'json' : '{{json .}}';
    let information;
    try {
      information = runCommand(command, ['info', '--format', format], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
      }).trim();
    } catch (error) {
      failures.push(
        new Error(
          error.code === 'ENOENT'
            ? `${name} was not found on PATH.`
            : `Cannot connect to ${name}; confirm that ${command} info succeeds.`,
          { cause: error },
        ),
      );
      continue;
    }
    let engine;
    try {
      engine = JSON.parse(information);
      if (!engine || typeof engine !== 'object' || Array.isArray(engine)) {
        throw new Error('Expected a container engine system information object.');
      }
    } catch (error) {
      throw new Error(
        `${name} returned invalid system information. Check that ${command} info succeeds and retry the build.`,
        { cause: error },
      );
    }
    return {
      command,
      name,
      os: command === 'podman' ? engine.host?.os : engine.OSType,
      architecture: command === 'podman' ? engine.host?.arch : engine.Architecture,
    };
  }
  throw new Error(
    `${failures.map((error) => error.message).join(' ')} ` +
      'Building the Linux SSH server requires a running Docker or Podman engine. ' +
      'Start Docker Desktop or your Docker Engine, or run podman machine start on Windows/macOS ' +
      '(run podman machine init first if no machine exists). ' +
      'Install the chosen CLI on PATH and set TANDEM_CONTAINER_ENGINE=docker or podman to select it explicitly.',
    { cause: new AggregateError(failures, 'Container engine detection failed.') },
  );
}

export function buildLinuxArtifact({
  root = repositoryRoot,
  target = 'x86_64-unknown-linux-gnu',
  runCommand = execFileSync,
  environment = process.env,
  containerEngine = environment.TANDEM_CONTAINER_ENGINE,
} = {}) {
  const targetPlatform = platforms.get(target);
  if (!targetPlatform) {
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

  const engine = selectContainerEngine(runCommand, containerEngine);
  if (engine.os !== 'linux') {
    throw new Error(
      'The Linux SSH server requires Linux containers. ' +
        (engine.command === 'docker'
          ? 'Switch Docker Desktop to Linux containers and retry the build.'
          : 'Select a Linux Podman connection, confirm that podman info succeeds, and retry the build.'),
    );
  }

  const architecture = engineArchitectures.get(engine.architecture);
  if (!architecture) {
    throw new Error(
      `Unsupported ${engine.name} engine architecture: ${engine.architecture}. Use a Linux ${engine.name} engine on x86_64 or ARM64.`,
    );
  }
  const platform = `linux/${architecture}`;
  const compilerPackage =
    architecture === targetPlatform.architecture ? '' : targetPlatform.package;
  const image = `localhost/tandem-remote-builder:${rustVersion}-${architecture}-${target}`;
  console.info(
    `Preparing the native ${platform} SSH server builder for ${target} with Rust ${rustVersion}.`,
  );
  try {
    runCommand(
      engine.command,
      [
        'build',
        '--platform',
        platform,
        '--build-arg',
        `RUST_VERSION=${rustVersion}`,
        '--build-arg',
        `RUST_TARGET=${target}`,
        '--build-arg',
        `CROSS_COMPILER_PACKAGE=${compilerPackage}`,
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
      `Could not prepare the ${engine.name} builder for ${target} on ${platform}. Check the ${engine.name} output above and access to Ubuntu packages and the Rust toolchain.`,
      { cause: error },
    );
  }

  const outputParent = join(sourceRoot, 'target', 'remote-linux', target);
  mkdirSync(outputParent, { recursive: true });
  const output = mkdtempSync(join(outputParent, 'build-'));
  const sourcePath = join(output, 'tandem-server');
  const repositoryId = createHash('sha256').update(sourceRoot).digest('hex').slice(0, 12);
  const environmentTarget = target.replaceAll('-', '_');
  console.info(`Building tandem-server for ${target} in ${engine.name}.`);
  try {
    runCommand(
      engine.command,
      [
        'run',
        '--rm',
        '--platform',
        platform,
        '--mount',
        bindMount(sourceRoot, '/workspace', true),
        '--mount',
        `type=volume,source=tandem-remote-${architecture}-${target}-cargo,target=/cargo-cache`,
        '--mount',
        `type=volume,source=tandem-remote-${repositoryId}-${architecture}-${target}-target,target=/build-target`,
        '--mount',
        bindMount(output, '/output'),
        '--env',
        `CARGO_TARGET_${environmentTarget.toUpperCase()}_LINKER=${targetPlatform.compiler}-gcc`,
        '--env',
        `CC_${environmentTarget}=${targetPlatform.compiler}-gcc`,
        '--env',
        `CXX_${environmentTarget}=${targetPlatform.compiler}-g++`,
        '--env',
        `AR_${environmentTarget}=${targetPlatform.compiler}-ar`,
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
      throw new Error(`${engine.name} completed without writing ${sourcePath}.`);
    }
  } catch (error) {
    rmSync(output, { recursive: true, force: true });
    throw new Error(
      `Could not build the Linux SSH server for ${target}. Check the ${engine.name} output above.`,
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
