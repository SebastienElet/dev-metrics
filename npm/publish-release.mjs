import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export async function publishPackage(directory, {
  registry = 'https://registry.npmjs.org/',
  timeoutMs = 10000,
  publish = packageDirectory => execFileSync('npm', ['publish', packageDirectory, '--provenance', '--access', 'public'], { stdio: 'inherit' }),
} = {}) {
  const manifest = JSON.parse(readFileSync(path.join(directory, 'package.json'), 'utf8'));
  const packed = JSON.parse(execFileSync('npm', ['pack', '--dry-run', '--json', directory], { encoding: 'utf8' }))[0];
  if (packed.name !== manifest.name || packed.version !== manifest.version || !packed.integrity) {
    throw new Error(`invalid npm package at ${directory}`);
  }

  const url = new URL(`${encodeURIComponent(manifest.name)}/${encodeURIComponent(manifest.version)}`, registry);
  const response = await fetch(url, { headers: { accept: 'application/json' }, signal: AbortSignal.timeout(timeoutMs) });
  if (response.status === 404) {
    await publish(directory);
    return;
  }
  if (!response.ok) {
    throw new Error(`npm registry returned ${response.status} for ${manifest.name}@${manifest.version}`);
  }
  const published = await response.json();
  if (published.dist?.integrity !== packed.integrity) {
    throw new Error(`npm package ${manifest.name}@${manifest.version} has different integrity`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  for (const directory of process.argv.slice(2)) {
    await publishPackage(directory);
  }
}
