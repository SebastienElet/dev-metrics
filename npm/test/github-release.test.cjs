const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, unlinkSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

const publish = path.join(__dirname, '../../.github/scripts/publish-github-release.sh');
const fakeGh = String.raw`#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
const release = process.env.TEST_RELEASE_DIR;
const statePath = path.join(release, 'state.json');
const assets = path.join(release, 'assets');
const command = args[1];
if (args[0] !== 'release') process.exit(2);
if (command === 'view') {
  if (!fs.existsSync(statePath)) process.exit(1);
  const state = JSON.parse(fs.readFileSync(statePath, 'utf8'));
  const field = args[args.indexOf('--json') + 1];
  if (args.includes('--json')) console.log(field === 'assets' ? fs.readdirSync(assets).length : state.isDraft);
} else if (command === 'download') {
  const files = fs.readdirSync(assets);
  if (files.length === 0) process.exit(1);
  const destination = args[args.indexOf('--dir') + 1];
  for (const file of files) fs.copyFileSync(path.join(assets, file), path.join(destination, file));
} else if (command === 'upload') {
  const failOnce = process.env.TEST_FAIL_UPLOAD_ONCE;
  for (const file of args.slice(3, args.indexOf('--repo'))) {
    const destination = path.join(assets, path.basename(file));
    if (fs.existsSync(destination) && !args.includes('--clobber')) process.exit(1);
    fs.copyFileSync(file, destination);
    if (process.env.TEST_CORRUPT_UPLOAD && file.endsWith('.tar.gz')) {
      fs.writeFileSync(destination, 'corrupt');
    }
    if (failOnce && fs.existsSync(failOnce)) {
      fs.unlinkSync(failOnce);
      process.exit(1);
    }
  }
} else if (command === 'edit') {
  fs.writeFileSync(statePath, JSON.stringify({ isDraft: false }));
} else if (command === 'create') {
  fs.writeFileSync(statePath, JSON.stringify({ isDraft: false }));
  for (const file of args.slice(3, args.indexOf('--repo'))) {
    fs.copyFileSync(file, path.join(assets, path.basename(file)));
  }
} else {
  process.exit(2);
}
`;

function fixture(existingAssets = []) {
  const root = mkdtempSync(path.join(tmpdir(), 'dev-metrics-gh-release-'));
  const dist = path.join(root, 'dist');
  const assets = path.join(root, 'release/assets');
  const bin = path.join(root, 'bin');
  mkdirSync(dist);
  mkdirSync(assets, { recursive: true });
  mkdirSync(bin);
  writeFileSync(path.join(root, 'release/state.json'), JSON.stringify({ isDraft: true }));
  writeFileSync(path.join(dist, 'binary.tar.gz'), 'archive');
  const checksum = createHash('sha256').update('archive').digest('hex');
  writeFileSync(path.join(dist, 'binary.tar.gz.sha256'), `${checksum}  binary.tar.gz\n`);
  for (const file of existingAssets) writeFileSync(path.join(assets, file), readFileSync(path.join(dist, file)));
  const gh = path.join(bin, 'gh');
  writeFileSync(gh, fakeGh);
  chmodSync(gh, 0o755);
  return { root, dist, assets, bin };
}

function run({ root, dist, bin }, extraEnv = {}) {
  const attempt = mkdtempSync(path.join(root, 'attempt-'));
  return spawnSync('bash', [publish, dist, 'v0.1.0', 'owner/repo'], {
    cwd: attempt,
    env: { ...process.env, ...extraEnv, PATH: `${bin}:${process.env.PATH}`, TEST_RELEASE_DIR: path.join(root, 'release') },
    encoding: 'utf8',
  });
}

test('replaces a partial draft with assets from the current build', () => {
  const context = fixture(['binary.tar.gz']);
  try {
    writeFileSync(path.join(context.dist, 'binary.tar.gz'), 'rebuilt archive');
    writeFileSync(path.join(context.dist, 'binary.tar.gz.sha256'), 'rebuilt checksum');
    const result = run(context);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: false });
    assert.deepEqual(readFileSync(path.join(context.assets, 'binary.tar.gz')), readFileSync(path.join(context.dist, 'binary.tar.gz')));
    assert.deepEqual(readFileSync(path.join(context.assets, 'binary.tar.gz.sha256')), readFileSync(path.join(context.dist, 'binary.tar.gz.sha256')));
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});

test('a failed draft upload can be resumed on the next run', () => {
  const context = fixture();
  const failOnce = path.join(context.root, 'fail-once');
  writeFileSync(failOnce, '1');
  try {
    assert.notEqual(run(context, { TEST_FAIL_UPLOAD_ONCE: failOnce }).status, 0);
    assert.equal(existsSync(failOnce), false);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: true });
    assert.equal(existsSync(path.join(context.assets, 'binary.tar.gz')), true);
    const resumed = run(context);
    assert.equal(resumed.status, 0, resumed.stderr);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: false });
    assert.deepEqual(readFileSync(path.join(context.assets, 'binary.tar.gz')), readFileSync(path.join(context.dist, 'binary.tar.gz')));
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});

test('does not publish a draft after a successful but corrupt upload', () => {
  const context = fixture(['binary.tar.gz']);
  try {
    const result = run(context, { TEST_CORRUPT_UPLOAD: '1' });
    assert.notEqual(result.status, 0);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: true });
    const retry = run(context);
    assert.equal(retry.status, 0, retry.stderr);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: false });
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});

test('creates a new release with every expected asset', () => {
  const context = fixture();
  unlinkSync(path.join(context.root, 'release/state.json'));
  try {
    const result = run(context);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: false });
    assert.deepEqual(readFileSync(path.join(context.assets, 'binary.tar.gz.sha256')), readFileSync(path.join(context.dist, 'binary.tar.gz.sha256')));
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});

test('refuses a draft containing an unexpected asset', () => {
  const context = fixture(['binary.tar.gz']);
  writeFileSync(path.join(context.assets, 'unexpected.zip'), 'other');
  try {
    const result = run(context);
    assert.notEqual(result.status, 0);
    assert.deepEqual(JSON.parse(readFileSync(path.join(context.root, 'release/state.json'))), { isDraft: true });
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});

test('uses verified published assets on a rerun with a different build', () => {
  const context = fixture(['binary.tar.gz', 'binary.tar.gz.sha256']);
  writeFileSync(path.join(context.root, 'release/state.json'), JSON.stringify({ isDraft: false }));
  writeFileSync(path.join(context.dist, 'binary.tar.gz'), 'rebuilt archive');
  writeFileSync(path.join(context.dist, 'binary.tar.gz.sha256'), 'rebuilt checksum');
  try {
    const result = run(context);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(path.join(context.dist, 'binary.tar.gz'), 'utf8'), 'archive');
    const checksum = createHash('sha256').update('archive').digest('hex');
    assert.equal(readFileSync(path.join(context.dist, 'binary.tar.gz.sha256'), 'utf8'), `${checksum}  binary.tar.gz\n`);
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});

test('refuses published assets with a failing checksum', () => {
  const context = fixture(['binary.tar.gz', 'binary.tar.gz.sha256']);
  writeFileSync(path.join(context.root, 'release/state.json'), JSON.stringify({ isDraft: false }));
  writeFileSync(path.join(context.assets, 'binary.tar.gz'), 'tampered');
  try {
    const result = run(context);
    assert.notEqual(result.status, 0);
    assert.equal(readFileSync(path.join(context.dist, 'binary.tar.gz'), 'utf8'), 'archive');
  } finally {
    rmSync(context.root, { recursive: true, force: true });
  }
});
