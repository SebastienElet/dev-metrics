const assert = require('node:assert/strict');
const { execFileSync } = require('node:child_process');
const { createServer } = require('node:http');
const { mkdtempSync, rmSync, writeFileSync } = require('node:fs');
const { tmpdir } = require('node:os');
const path = require('node:path');
const { test } = require('node:test');

async function fixture(responseStatus, integrity, body) {
  const directory = mkdtempSync(path.join(tmpdir(), 'dev-metrics-publish-'));
  writeFileSync(path.join(directory, 'package.json'), JSON.stringify({ name: '@sebastienelet/example', version: '0.1.0', files: ['index.js'] }));
  writeFileSync(path.join(directory, 'index.js'), 'module.exports = 1;\n');
  const packed = JSON.parse(execFileSync('npm', ['pack', '--dry-run', '--json', directory], { encoding: 'utf8' }))[0];
  const server = createServer((request, response) => {
    if (responseStatus === 0) {
      setTimeout(() => response.end('{}'), 100);
      return;
    }
    response.writeHead(responseStatus, { 'content-type': 'application/json' });
    response.end(body ?? (responseStatus === 200 ? JSON.stringify({ dist: { integrity: integrity ?? packed.integrity } }) : '{}'));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return {
    directory,
    registry: `http://127.0.0.1:${server.address().port}/`,
    close: async () => {
      await new Promise(resolve => server.close(resolve));
      rmSync(directory, { recursive: true, force: true });
    },
  };
}

test('publishes a missing version', async () => {
  const { publishPackage } = await import('../publish-release.mjs');
  const context = await fixture(404);
  let published = 0;
  try {
    await publishPackage(context.directory, { registry: context.registry, publish: async () => { published++; } });
    assert.equal(published, 1);
  } finally {
    await context.close();
  }
});

test('skips a version already published with identical contents', async () => {
  const { publishPackage } = await import('../publish-release.mjs');
  const context = await fixture(200);
  let published = 0;
  try {
    await publishPackage(context.directory, { registry: context.registry, publish: async () => { published++; } });
    assert.equal(published, 0);
  } finally {
    await context.close();
  }
});

test('refuses an existing version with different contents', async () => {
  const { publishPackage } = await import('../publish-release.mjs');
  const context = await fixture(200, 'sha512-different');
  try {
    await assert.rejects(publishPackage(context.directory, { registry: context.registry, publish: async () => {} }), /different integrity/);
  } finally {
    await context.close();
  }
});

test('does not publish after a registry error', async () => {
  const { publishPackage } = await import('../publish-release.mjs');
  const context = await fixture(503);
  let published = 0;
  try {
    await assert.rejects(publishPackage(context.directory, { registry: context.registry, publish: async () => { published++; } }), /registry returned 503/);
    assert.equal(published, 0);
  } finally {
    await context.close();
  }
});

test('does not publish malformed registry metadata', async () => {
  const { publishPackage } = await import('../publish-release.mjs');
  const context = await fixture(200, undefined, 'invalid');
  let published = 0;
  try {
    await assert.rejects(publishPackage(context.directory, { registry: context.registry, publish: async () => { published++; } }), /JSON/);
    assert.equal(published, 0);
  } finally {
    await context.close();
  }
});

test('times out a stalled registry response without publishing', async () => {
  const { publishPackage } = await import('../publish-release.mjs');
  const context = await fixture(0);
  let published = 0;
  try {
    await assert.rejects(publishPackage(context.directory, { registry: context.registry, timeoutMs: 20, publish: async () => { published++; } }), /timeout|abort/i);
    assert.equal(published, 0);
  } finally {
    await context.close();
  }
});
