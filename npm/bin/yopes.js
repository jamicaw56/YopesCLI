#!/usr/bin/env node
// npm bin shim: npm links this file, then it execs the release binary the
// postinstall script downloaded. Works whether or not scripts ran.
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');

const binary = path.join(__dirname, process.platform === 'win32' ? 'yopes.exe' : 'yopes');

if (!fs.existsSync(binary)) {
  console.error(
    'The yopes binary is missing. The postinstall step either failed or was skipped\n' +
      '(npm install --ignore-scripts). Run `npm rebuild yopes` or install from\n' +
      'https://github.com/Jamicaw56/YopesCLI/releases'
  );
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });
child.on('error', (error) => {
  console.error(`could not run ${binary}: ${error.message}`);
  process.exit(1);
});
child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code === null || code === undefined ? 0 : code);
  }
});
