const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { mkdtempSync, mkdirSync, rmSync, unlinkSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const check = path.join(__dirname, '../../.github/scripts/check-release-assets.sh');

function fixture() {
  const root = mkdtempSync(path.join(tmpdir(), 'dev-metrics-assets-'));
  const expected = path.join(root, 'expected');
  const actual = path.join(root, 'actual');
  mkdirSync(expected);
  mkdirSync(actual);
  writeFileSync(path.join(expected, 'dev-metrics.tar.gz'), 'archive');
  writeFileSync(path.join(expected, 'dev-metrics.tar.gz.sha256'), 'hash');
  writeFileSync(path.join(actual, 'dev-metrics.tar.gz'), 'archive');
  writeFileSync(path.join(actual, 'dev-metrics.tar.gz.sha256'), 'hash');
  return { root, expected, actual };
}

function run(expected, actual, state = 'false') {
  return spawnSync('bash', [check, expected, actual, state], { encoding: 'utf8' });
}

test('accepts identical archives and checksums', () => {
  const { root, expected, actual } = fixture();
  try {
    assert.equal(run(expected, actual).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects changed asset bytes', () => {
  const { root, expected, actual } = fixture();
  try {
    writeFileSync(path.join(actual, 'dev-metrics.tar.gz'), 'other');
    assert.notEqual(run(expected, actual).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects missing or unexpected assets', () => {
  const { root, expected, actual } = fixture();
  try {
    writeFileSync(path.join(actual, 'unexpected.tar.gz'), 'extra');
    assert.notEqual(run(expected, actual).status, 0);
    writeFileSync(path.join(expected, 'missing.tar.gz'), 'missing');
    assert.notEqual(run(expected, actual).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects an empty expected set', () => {
  const { root, actual } = fixture();
  const empty = path.join(root, 'empty');
  mkdirSync(empty);
  try {
    assert.notEqual(run(empty, actual).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects hidden assets and an unverified release state', () => {
  const { root, expected, actual } = fixture();
  try {
    writeFileSync(path.join(actual, '.stale.tar.gz'), 'hidden');
    assert.notEqual(run(expected, actual).status, 0);
    assert.notEqual(run(expected, expected, 'unknown').status, 0);
    assert.notEqual(run(expected, expected, '').status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a matching draft lists only missing assets for upload', () => {
  const { root, expected, actual } = fixture();
  const missing = path.join(actual, 'dev-metrics.tar.gz.sha256');
  try {
    unlinkSync(missing);
    const result = run(expected, actual, 'true');
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), path.join(expected, 'dev-metrics.tar.gz.sha256'));
    assert.notEqual(run(expected, actual, 'false').status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('an empty draft lists every required asset', () => {
  const { root, expected } = fixture();
  const empty = path.join(root, 'empty');
  mkdirSync(empty);
  try {
    const result = run(expected, empty, 'true');
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.trim().split('\n').sort(), [
      path.join(expected, 'dev-metrics.tar.gz'),
      path.join(expected, 'dev-metrics.tar.gz.sha256'),
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('a draft rejects changed and unexpected assets', () => {
  const { root, expected, actual } = fixture();
  try {
    writeFileSync(path.join(actual, 'dev-metrics.tar.gz'), 'other');
    assert.notEqual(run(expected, actual, 'true').status, 0);
    writeFileSync(path.join(actual, 'dev-metrics.tar.gz'), 'archive');
    writeFileSync(path.join(actual, 'unexpected.zip'), 'extra');
    assert.notEqual(run(expected, actual, 'true').status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
