const assert = require('node:assert/strict');
const { execFileSync, spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdtempSync, mkdirSync, readFileSync, rmSync, unlinkSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const platforms = require('../platforms.json');
const prepare = path.join(__dirname, '../prepare-release.mjs');
const version = '0.1.0';

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'dev-metrics-release-'));
  const dist = path.join(root, 'dist');
  const input = path.join(root, 'input');
  mkdirSync(dist);
  mkdirSync(input);
  for (const platform of Object.values(platforms)) {
    writeFileSync(path.join(input, platform.binary), `binary for ${platform.target}`);
    const archiveName = `dev-metrics-v${version}-${platform.target}.tar.gz`;
    const archive = path.join(dist, archiveName);
    execFileSync('tar', ['-czf', archive, '-C', input, platform.binary]);
    const sha = createHash('sha256').update(readFileSync(archive)).digest('hex');
    writeFileSync(`${archive}.sha256`, `${sha}  ${archiveName}\n`);
  }
  return { root, dist };
}

test('stages one npm package per archive with its binary and platform metadata', () => {
  const { root, dist } = fixture();
  try {
    const result = spawnSync(process.execPath, [prepare, dist, version], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    const packageDir = path.join(dist, 'npm', 'linux-arm64');
    const manifest = JSON.parse(readFileSync(path.join(packageDir, 'package.json'), 'utf8'));
    assert.equal(manifest.name, '@sebastienelet/dev-metrics-linux-arm64');
    assert.equal(manifest.version, version);
    assert.deepEqual(manifest.os, ['linux']);
    assert.deepEqual(manifest.cpu, ['arm64']);
    assert.equal(readFileSync(path.join(packageDir, 'bin/dev-metrics'), 'utf8'), 'binary for aarch64-unknown-linux-gnu');
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects an archive whose published checksum does not match', () => {
  const { root, dist } = fixture();
  try {
    writeFileSync(path.join(dist, `dev-metrics-v${version}-aarch64-apple-darwin.tar.gz.sha256`), `${'0'.repeat(64)}  dev-metrics-v${version}-aarch64-apple-darwin.tar.gz\n`);
    const result = spawnSync(process.execPath, [prepare, dist, version], { encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /checksum mismatch/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects a missing or malformed checksum', () => {
  const { root, dist } = fixture();
  const checksum = path.join(dist, `dev-metrics-v${version}-aarch64-apple-darwin.tar.gz.sha256`);
  try {
    writeFileSync(checksum, 'invalid\n');
    const malformed = spawnSync(process.execPath, [prepare, dist, version], { encoding: 'utf8' });
    assert.notEqual(malformed.status, 0);
    assert.match(malformed.stderr, /invalid checksum/);

    unlinkSync(checksum);
    const missing = spawnSync(process.execPath, [prepare, dist, version], { encoding: 'utf8' });
    assert.notEqual(missing.status, 0);
    assert.match(missing.stderr, /ENOENT/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects a corrupt archive even when its checksum matches', () => {
  const { root, dist } = fixture();
  const archiveName = `dev-metrics-v${version}-aarch64-apple-darwin.tar.gz`;
  try {
    writeFileSync(path.join(dist, archiveName), 'invalid archive');
    const sha = createHash('sha256').update('invalid archive').digest('hex');
    writeFileSync(path.join(dist, `${archiveName}.sha256`), `${sha}  ${archiveName}\n`);
    const result = spawnSync(process.execPath, [prepare, dist, version], { encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /tar/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects a release version that differs from the root npm package', () => {
  const { root, dist } = fixture();
  try {
    const result = spawnSync(process.execPath, [prepare, dist, '0.2.0'], { encoding: 'utf8' });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /version/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
