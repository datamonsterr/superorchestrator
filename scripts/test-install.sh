#!/bin/sh
# Tests install.sh against a fake local release directory (file:// URLs, no network).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/so-install-test.XXXXXX")
trap 'rm -rf "$work"' EXIT INT TERM

fail() { printf '%s\n' "test-install: FAIL: $*" >&2; exit 1; }

case "$(uname -m)" in x86_64|amd64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; *) arch=$(uname -m) ;; esac
case "$(uname -s)" in Linux) target="$arch-unknown-linux-gnu" ;; Darwin) target="$arch-apple-darwin" ;; *) fail "unsupported OS" ;; esac
asset="superorchestrator-$target.tar.xz"

hash_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1
  else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
}

# A fake release, laid out like dist's: <asset> containing <dir>/superorchestrator, plus <asset>.sha256.
rel="$work/release"
mkdir -p "$rel" "$work/pkg/superorchestrator-$target"
printf '#!/bin/sh\necho "superorchestrator 9.9.9"\n' > "$work/pkg/superorchestrator-$target/superorchestrator"
chmod +x "$work/pkg/superorchestrator-$target/superorchestrator"
tar -cJf "$rel/$asset" -C "$work/pkg" "superorchestrator-$target"
printf '%s *%s\n' "$(hash_of "$rel/$asset")" "$asset" > "$rel/$asset.sha256"

export SUPERORCHESTRATOR_RELEASE_URL="file://$rel"

# 1. happy path
dest="$work/bin"
out=$(SUPERORCHESTRATOR_INSTALL_DIR="$dest" sh "$root/install.sh" 0.1.0) || fail "install failed: $out"
[ -x "$dest/superorchestrator" ] || fail "binary not installed"
[ "$("$dest/superorchestrator")" = "superorchestrator 9.9.9" ] || fail "installed binary does not run"
case "$out" in *"sha256 verified"*) ;; *) fail "no verification message" ;; esac

# 2. re-install over an existing binary
SUPERORCHESTRATOR_INSTALL_DIR="$dest" sh "$root/install.sh" >/dev/null || fail "re-install failed"

# 3. corrupted download is rejected and nothing is installed
printf 'tampered' >> "$rel/$asset"
dest2="$work/bin2"
if SUPERORCHESTRATOR_INSTALL_DIR="$dest2" sh "$root/install.sh" >"$work/err" 2>&1; then fail "tampered archive was accepted"; fi
grep -q "checksum mismatch" "$work/err" || fail "no checksum error: $(cat "$work/err")"
[ ! -e "$dest2/superorchestrator" ] || fail "binary installed despite checksum mismatch"

# 4. missing asset
rm "$rel/$asset"
if SUPERORCHESTRATOR_INSTALL_DIR="$dest2" sh "$root/install.sh" >"$work/err" 2>&1; then fail "missing asset accepted"; fi
grep -q "download failed" "$work/err" || fail "no download error"

# 5. --help and bad options
sh "$root/install.sh" --help | grep -q "Usage:" || fail "--help"
if sh "$root/install.sh" --bogus >/dev/null 2>&1; then fail "unknown option accepted"; fi

echo "test-install: ok"
