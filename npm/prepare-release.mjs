import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const rootPackage = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8'));
const platforms = JSON.parse(readFileSync(path.join(directory, 'platforms.json'), 'utf8'));
const [distDirectory, version] = process.argv.slice(2);

if (!distDirectory || version !== rootPackage.version) {
  throw new Error(`release version must match npm package version ${rootPackage.version}`);
}

const packages = Object.entries(platforms).map(([key, platform]) => {
  if (rootPackage.optionalDependencies[platform.name] !== `=${version}`) {
    throw new Error(`optional dependency ${platform.name} must use =${version}`);
  }
  const archiveName = `dev-metrics-v${version}-${platform.target}.tar.gz`;
  const archive = path.join(distDirectory, archiveName);
  const checksum = readFileSync(`${archive}.sha256`, 'utf8');
  const match = /^([0-9a-f]{64})  ([^\n]+)\n?$/.exec(checksum);
  if (!match || match[2] !== archiveName) {
    throw new Error(`invalid checksum for ${archiveName}`);
  }
  const actual = createHash('sha256').update(readFileSync(archive)).digest('hex');
  if (actual !== match[1]) {
    throw new Error(`checksum mismatch for ${archiveName}`);
  }
  const binary = execFileSync('tar', ['-xOzf', archive, platform.binary], { maxBuffer: 128 * 1024 * 1024 });
  return { key, platform, binary };
});

for (const { key, platform, binary } of packages) {
  const packageDirectory = path.join(distDirectory, 'npm', key);
  const binDirectory = path.join(packageDirectory, 'bin');
  mkdirSync(binDirectory, { recursive: true });
  const [os, cpu] = key.split('-');
  const manifest = {
    name: platform.name,
    version,
    description: rootPackage.description,
    repository: rootPackage.repository,
    os: [os],
    cpu: [cpu],
    files: [`bin/${platform.binary}`],
  };
  writeFileSync(path.join(packageDirectory, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  writeFileSync(path.join(binDirectory, platform.binary), binary, { mode: 0o755 });
}
