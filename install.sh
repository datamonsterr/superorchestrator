#!/bin/sh
# Superorchestrator installer. Downloads a release archive from GitHub, verifies its
# sha256 checksum and installs the binary. Nothing downloaded is ever executed by a shell.
#
# Usage: install.sh [--help] [VERSION]
#   VERSION  a release such as 0.1.0 or v0.1.0 (default: the latest release)
#
# Environment:
#   SUPERORCHESTRATOR_INSTALL_DIR   install directory (default: $HOME/.local/bin)
#   SUPERORCHESTRATOR_RELEASE_URL   base URL of the release assets, for mirrors and tests
#                                   (default: the GitHub release of the requested version)
set -eu

REPO="datamonsterr/superorchestrator"
BIN="superorchestrator"

say() { printf '%s\n' "install: $*"; }
die() { printf '%s\n' "install: error: $*" >&2; exit 1; }

usage() {
  cat <<USAGE
Install superorchestrator from a GitHub release.

Usage: install.sh [--help] [VERSION]

  VERSION   release to install, e.g. 0.1.0 or v0.1.0 (default: latest)

Environment:
  SUPERORCHESTRATOR_INSTALL_DIR   install directory (default: \$HOME/.local/bin)
  SUPERORCHESTRATOR_RELEASE_URL   base URL of the release assets (mirrors, tests)

Supported: Linux and macOS on x86_64 and aarch64.
USAGE
}

version=""
for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    -*) usage >&2; die "unknown option: $arg" ;;
    *)
      [ -z "$version" ] || die "only one VERSION argument is accepted"
      version="$arg"
      ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || die "required tool not found: $1"; }

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64|amd64) arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    *) die "unsupported CPU architecture: $arch" ;;
  esac
  case "$os" in
    Linux) echo "$arch-unknown-linux-gnu" ;;
    Darwin) echo "$arch-apple-darwin" ;;
    *) die "unsupported operating system: $os (try: cargo install --git https://github.com/$REPO)" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d ' ' -f 1
  else
    die "neither sha256sum nor shasum found; cannot verify the download"
  fi
}

fetch() { # url dest
  curl --fail --silent --show-error --location --proto '=https,file' "$1" -o "$2" \
    || die "download failed: $1"
}

need curl
need tar
need uname
need mktemp

target=$(detect_target)
asset="$BIN-$target.tar.xz"

if [ -n "${SUPERORCHESTRATOR_RELEASE_URL:-}" ]; then
  base="${SUPERORCHESTRATOR_RELEASE_URL%/}"
elif [ -z "$version" ] || [ "$version" = latest ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/v${version#v}"
fi

install_dir="${SUPERORCHESTRATOR_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/superorchestrator-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

say "target $target"
say "downloading $base/$asset"
fetch "$base/$asset" "$tmp/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256"

# The checksum file reads "<hash> *<file>" (dist) or "<hash>  <file>".
expected=$(cut -d ' ' -f 1 < "$tmp/$asset.sha256" | tr 'A-F' 'a-f')
[ -n "$expected" ] || die "empty checksum file for $asset"
actual=$(sha256_of "$tmp/$asset")
if [ "$expected" != "$actual" ]; then
  die "checksum mismatch for $asset (expected $expected, got $actual); nothing was installed"
fi
say "sha256 verified"

mkdir "$tmp/x"
tar -xJf "$tmp/$asset" -C "$tmp/x" || die "could not extract $asset (is xz installed?)"
src=$(find "$tmp/x" -type f -name "$BIN" | head -n 1)
[ -n "$src" ] || die "$BIN not found in $asset"

mkdir -p "$install_dir" || die "cannot create $install_dir"
# Copy then rename so a running binary is replaced atomically.
cp "$src" "$install_dir/.$BIN.new" || die "cannot write to $install_dir"
chmod 755 "$install_dir/.$BIN.new"
mv -f "$install_dir/.$BIN.new" "$install_dir/$BIN"
say "installed $install_dir/$BIN"

case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) say "note: $install_dir is not on your PATH; add it to your shell profile" ;;
esac
say "next: run '$BIN init'"
