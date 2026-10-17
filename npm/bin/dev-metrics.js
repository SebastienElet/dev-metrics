#!/usr/bin/env node
const { spawnSync } = require('node:child_process');
const platforms = require('../platforms.json');

const platform = platforms[`${process.platform}-${process.arch}`];
if (!platform) {
  console.error(`dev-metrics: unsupported platform ${process.platform}-${process.arch}`);
  process.exit(1);
}

let binary;
try {
  binary = require.resolve(`${platform.name}/bin/${platform.binary}`);
} catch {
  console.error(`dev-metrics: missing ${platform.name}; install with optionalDependencies enabled`);
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  console.error(`dev-metrics: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
  process.exit(1);
}
process.exit(result.status ?? 1);
