# yopes (npm)

`npm install -g @jamicaw56/yopes` installs the yopes coding CLI.

The package is a thin wrapper: `postinstall` downloads the release binary for
your platform from [GitHub Releases](https://github.com/Jamicaw56/YopesCLI/releases),
verifies its SHA256 checksum against `SHA256SUMS`, and drops it next to the
`bin` shim. Supported platforms: `win32-x64` and `linux-x64`.

The `yopes` command then runs the binary directly. If the postinstall step was
skipped (`npm install --ignore-scripts`), the shim says so and points at the
releases page.
