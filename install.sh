#!/usr/bin/env sh
# Yopes installer: fetches the latest GitHub release for Linux, verifies the
# SHA256SUMS checksum, installs yopes into a user directory, and adds it to PATH.
#
# Remote one-liner (defaults):
#   curl -fsSL https://raw.githubusercontent.com/Jamicaw56/YopesCLI/main/install.sh | sh
#
# Or pass options:
#   curl -fsSL https://raw.githubusercontent.com/Jamicaw56/YopesCLI/main/install.sh | sh -s -- --dest ~/.local/bin --no-path

set -eu

REPO="Jamicaw56/YopesCLI"
ASSET="yopes-Linux-X64.tar.gz"
DEST="${XDG_BIN_HOME:-$HOME/.local/bin}"
NO_PATH=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        -d|--dest)
            [ "$#" -ge 2 ] || { echo "Error: $1 requires a directory argument." >&2; exit 1; }
            DEST="$2"
            shift 2
            ;;
        --no-path)
            NO_PATH=1
            shift
            ;;
        -h|--help)
            echo "Usage: install.sh [--dest <dir>] [--no-path]"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"
if [ "$OS" != "Linux" ]; then
    echo "Error: install.sh is for Linux (detected $OS). Download the matching archive from https://github.com/$REPO/releases" >&2
    exit 1
fi
case "$ARCH" in
    x86_64|amd64) ;;
    *)
        echo "Error: unsupported architecture '$ARCH'. Prebuilt Linux binaries currently target x86_64." >&2
        exit 1
        ;;
esac

# An already-installed yopes updates itself in place; replacing a different
# install would leave two copies shadowing each other on PATH.
if EXISTING="$(command -v yopes 2>/dev/null)"; then
    case "$EXISTING" in
        "$DEST"/*) ;;
        *)
            echo "Already installed: $EXISTING"
            echo "Run \`yopes update\` to update that install."
            exit 0
            ;;
    esac
fi

# Pick an HTTP client.
if command -v curl >/dev/null 2>&1; then
    HTTP="curl"
elif command -v wget >/dev/null 2>&1; then
    HTTP="wget"
else
    echo "Error: curl or wget is required." >&2
    exit 1
fi

TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

http_get() {
    url="$1"
    if [ "$HTTP" = "curl" ]; then
        if [ -n "$TOKEN" ]; then
            curl -fsSL \
                -H "Accept: application/vnd.github+json" \
                -H "X-GitHub-Api-Version: 2022-11-28" \
                -H "Authorization: Bearer $TOKEN" \
                "$url"
        else
            curl -fsSL \
                -H "Accept: application/vnd.github+json" \
                -H "X-GitHub-Api-Version: 2022-11-28" \
                "$url"
        fi
    else
        if [ -n "$TOKEN" ]; then
            wget -qO- \
                --header="Accept: application/vnd.github+json" \
                --header="X-GitHub-Api-Version: 2022-11-28" \
                --header="Authorization: Bearer $TOKEN" \
                "$url"
        else
            wget -qO- \
                --header="Accept: application/vnd.github+json" \
                --header="X-GitHub-Api-Version: 2022-11-28" \
                "$url"
        fi
    fi
}

http_download() {
    url="$1"
    out="$2"
    if [ "$HTTP" = "curl" ]; then
        curl -fsSL -o "$out" "$url"
    else
        wget -qO "$out" "$url"
    fi
}

printf "Looking up the latest release..."
RELEASE_JSON="$(http_get "https://api.github.com/repos/$REPO/releases/latest")"
TAG="$(printf '%s' "$RELEASE_JSON" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
if [ -z "$TAG" ]; then
    echo ""
    echo "Error: could not determine latest release tag." >&2
    exit 1
fi
echo " $TAG"

ASSET_URL="https://github.com/$REPO/releases/download/$TAG/$ASSET"
CHECKSUMS_URL="https://github.com/$REPO/releases/download/$TAG/SHA256SUMS"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/yopes-install.XXXXXX")"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

ARCHIVE="$TMP_DIR/$ASSET"
CHECKSUMS="$TMP_DIR/SHA256SUMS"

printf "Downloading %s..." "$ASSET"
http_download "$ASSET_URL" "$ARCHIVE"
http_download "$CHECKSUMS_URL" "$CHECKSUMS"
echo " done"

# Verify SHA-256 checksum before extracting or installing anything.
EXPECTED="$(awk -v asset="$ASSET" '
    {
        name = $2
        sub(/^\*/, "", name)
        if (name == asset) {
            print tolower($1)
            exit
        }
    }
' "$CHECKSUMS")"
if [ -z "$EXPECTED" ]; then
    echo "Error: SHA256SUMS has no entry for $ASSET." >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "$ARCHIVE" | awk '{print tolower($1)}')"
elif command -v shasum >/dev/null 2>&1; then
    ACTUAL="$(shasum -a 256 "$ARCHIVE" | awk '{print tolower($1)}')"
else
    echo "Error: sha256sum or shasum is required to verify the download." >&2
    exit 1
fi

if [ "$ACTUAL" != "$EXPECTED" ]; then
    echo "Error: checksum mismatch for $ASSET ($ACTUAL, expected $EXPECTED)." >&2
    exit 1
fi
echo "Checksum verified."

printf "Installing to %s..." "$DEST"
mkdir -p "$TMP_DIR/extracted"
tar -xzf "$ARCHIVE" -C "$TMP_DIR/extracted"
if [ ! -f "$TMP_DIR/extracted/yopes" ]; then
    echo ""
    echo "Error: the downloaded archive does not contain yopes." >&2
    exit 1
fi
mkdir -p "$DEST"
install -m 755 "$TMP_DIR/extracted/yopes" "$DEST/yopes"
echo " done"

if [ "$NO_PATH" -eq 0 ]; then
    case ":$PATH:" in
        *":$DEST:"*) ;;
        *)
            LINE="export PATH=\"$DEST:\$PATH\""
            SHELL_NAME="$(basename "${SHELL:-}")"
            case "$SHELL_NAME" in
                zsh)  RC_FILE="$HOME/.zshrc" ;;
                bash) RC_FILE="$HOME/.bashrc" ;;
                *)    RC_FILE="$HOME/.profile" ;;
            esac
            if [ ! -f "$RC_FILE" ] || ! grep -Fq "$DEST" "$RC_FILE"; then
                printf '\n# Added by yopes installer\n%s\n' "$LINE" >> "$RC_FILE"
                echo "Added $DEST to $RC_FILE."
                echo "Run \`source $RC_FILE\` or open a new terminal, then run \`yopes\`."
            fi
            ;;
    esac
fi

echo "Installed yopes $TAG."
