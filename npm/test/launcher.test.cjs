const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync, chmodSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const platforms = require('../platforms.json');
const launcher = path.join(__dirname, '../bin/dev-metrics.js');
const current = platforms[`${process.platform}-${process.arch}`];

function fixture(binary) {
  const root = mkdtempSync(path.join(tmpdir(), 'dev-metrics-npm-'));
  mkdirSync(path.join(root, 'bin'), { recursive: true });
  copyFileSync(launcher, path.join(root, 'bin/dev-metrics.js'));
  copyFileSync(path.join(__dirname, '../platforms.json'), path.join(root, 'platforms.json'));
  if (binary && current) {
    const binaryPath = path.join(root, 'node_modules', current.name, 'bin', current.binary);
    mkdirSync(path.dirname(binaryPath), { recursive: true });
    writeFileSync(binaryPath, binary);
    chmodSync(binaryPath, 0o755);
  }
  return root;
}

test('forwards arguments and exit status to the platform binary', { skip: !current || process.platform === 'win32' }, () => {
  const root = fixture('#!/bin/sh\nprintf "%s\\n" "$1"\nexit 7\n');
  try {
    const result = spawnSync(process.execPath, [path.join(root, 'bin/dev-metrics.js'), '--help'], { encoding: 'utf8' });
    assert.equal(result.stdout, '--help\n');
    assert.equal(result.status, 7);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('explains a missing platform package', { skip: !current }, () => {
  const root = fixture();
  try {
    const result = spawnSync(process.execPath, [path.join(root, 'bin/dev-metrics.js')], { encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /optionalDependencies/);
    assert.match(result.stderr, new RegExp(current.name));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('rejects an unsupported platform', () => {
  const root = fixture();
  try {
    const script = `Object.defineProperty(process, 'platform', { value: 'freebsd' }); require(${JSON.stringify(path.join(root, 'bin/dev-metrics.js'))});`;
    const result = spawnSync(process.execPath, ['-e', script], { encoding: 'utf8' });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /freebsd/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
