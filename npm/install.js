// postinstall: fetch the release binary for this platform from GitHub
// Releases, verify its SHA256 checksum, and drop it next to bin/yopes.js.
//
// ponytail: relies on the system `curl` (ships with Windows 10+ and most
// Linux) and the system `tar` (bsdtar on Windows reads zip). Upgrade path:
// an https fetch with redirect handling plus a zip reader when one is needed.
'use strict';

const { spawnSync } = require('child_process');
const crypto = require('crypto');
const fs = require('fs');
const os = require('os');
const path = require('path');

const REPO = 'Jamicaw56/YopesCLI';

// One asset per released platform; macOS is excluded from releases.
const ASSETS = {
  'win32-x64': 'yopes-Windows-X64.zip',
  'linux-x64': 'yopes-Linux-X64.tar.gz',
};

function fail(message) {
  console.error(`yopes postinstall: ${message}`);
  console.error('Install from https://github.com/Jamicaw56/YopesCLI/releases instead.');
  process.exit(1);
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
  if (result.error) {
    fail(`could not run ${command}: ${result.error.message}`);
  }
  return result;
}

const asset = ASSETS[`${process.platform}-${process.arch}`];
if (!asset) {
  fail(
    `no release build for ${process.platform}-${process.arch}. ` +
      'Supported: win32-x64, linux-x64.'
  );
}

const version = require('./package.json').version;
const tag = `v${version}`;
const base = `https://github.com/${REPO}/releases/download/${tag}`;
const target = path.join(__dirname, 'bin', process.platform === 'win32' ? 'yopes.exe' : 'yopes');

if (fs.existsSync(target)) {
  console.log(`yopes ${version} already installed.`);
  process.exit(0);
}

const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'yopes-install-'));
try {
  const archive = path.join(temporary, asset);
  const sums = path.join(temporary, 'SHA256SUMS');

  console.log(`Downloading ${asset} (${tag})...`);
  const download = run('curl', ['-fsSL', '-o', archive, `${base}/${asset}`]);
  if (download.status !== 0) {
    fail(`download failed: ${String(download.stderr).trim()}`);
  }
  const checksums = run('curl', ['-fsSL', '-o', sums, `${base}/SHA256SUMS`]);
  if (checksums.status !== 0) {
    fail(`checksum list download failed: ${String(checksums.stderr).trim()}`);
  }

  // An unverified binary is the one thing we must never install.
  const expected = String(fs.readFileSync(sums, 'utf8'))
    .split(/\r?\n/)
    .filter((line) => line.trimEnd().endsWith(asset))
    .map((line) => line.split(/\s+/)[0].trim())
    .find((hash) => /^[0-9A-Fa-f]{64}$/.test(hash));
  if (!expected) {
    fail(`SHA256SUMS has no line for ${asset}.`);
  }
  const actual = crypto
    .createHash('sha256')
    .update(fs.readFileSync(archive))
    .digest('hex');
  if (actual !== expected) {
    fail(`checksum mismatch for ${asset} (${actual}, expected ${expected}).`);
  }

  const extracted = path.join(temporary, 'extracted');
  fs.mkdirSync(extracted, { recursive: true });
  // Windows: Expand-Archive (built-in, reads zip). The system tar is either
  // GNU tar, which cannot read zip and reads the drive colon as a remote
  // host, or bsdtar — not worth telling apart. Linux: tar -xzf.
  if (process.platform === 'win32') {
    const script = `Expand-Archive -Path '${archive}' -DestinationPath '${extracted}' -Force`;
    const unzip = run('powershell', ['-NoProfile', '-Command', script]);
    if (unzip.status !== 0) {
      fail(`extraction failed: ${String(unzip.stderr).trim()}`);
    }
  } else {
    const untar = run('tar', ['-xzf', archive, '-C', extracted]);
    if (untar.status !== 0) {
      fail(`extraction failed: ${String(untar.stderr).trim()}`);
    }
  }

  const binary = path.join(extracted, process.platform === 'win32' ? 'yopes.exe' : 'yopes');
  if (!fs.existsSync(binary)) {
    fail('the release archive does not contain the yopes binary.');
  }
  fs.copyFileSync(binary, target);
  if (process.platform !== 'win32') {
    fs.chmodSync(target, 0o755);
  }
  console.log(`Installed yopes ${version}.`);
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
