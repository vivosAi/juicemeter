#!/bin/sh
# Install juicemeter (CLI and agent) from the latest GitHub release, then run its setup.
#
#   curl -fsSL https://raw.githubusercontent.com/vivosAi/juicemeter/main/install.sh | sh
#
# JUICEMETER_VERSION=v0.2.0   install that release instead of the latest
# JUICEMETER_BIN_DIR=/path     install there instead of ~/.local/bin
# JUICEMETER_NO_SETUP=1        skip `juicemeter setup` at the end
# JUICEMETER_DOWNLOAD_URL=...   download the archive from here instead (mirrors, testing)
set -eu

REPO="vivosAi/juicemeter"
BIN_DIR="${JUICEMETER_BIN_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { say "juicemeter: $*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) target="macos-universal" ;;
  Linux)
    case "$(uname -m)" in
      x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
      aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
      *) fail "no prebuilt binaries for $(uname -m) yet; build from source (see the README)" ;;
    esac ;;
  *) fail "this installer is for macOS and Linux; on Windows, download the zip from https://github.com/$REPO/releases" ;;
esac

if [ -n "${JUICEMETER_VERSION:-}" ]; then
  url="https://github.com/$REPO/releases/download/$JUICEMETER_VERSION/juicemeter-$target.tar.gz"
else
  url="https://github.com/$REPO/releases/latest/download/juicemeter-$target.tar.gz"
fi

url="${JUICEMETER_DOWNLOAD_URL:-$url}"

command -v curl >/dev/null 2>&1 || fail "curl is needed"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading $url"
curl -fsSL "$url" -o "$tmp/juicemeter.tar.gz" || fail "download failed"
tar -xzf "$tmp/juicemeter.tar.gz" -C "$tmp"

mkdir -p "$BIN_DIR"
for b in juicemeter juicemeter-agent; do
  install -m 755 "$tmp/$b" "$BIN_DIR/$b"
done
say "Installed juicemeter and juicemeter-agent in $BIN_DIR"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "Add $BIN_DIR to your PATH, e.g.: echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.profile" ;;
esac

if [ -z "${JUICEMETER_NO_SETUP:-}" ]; then
  say ""
  "$BIN_DIR/juicemeter" setup
fi
