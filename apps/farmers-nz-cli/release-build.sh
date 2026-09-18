#!/usr/bin/env bash
# Build stage: produce the release artifacts and name them for upload.
#
# SHA256SUMS must cover every platform -- `farmers update` refuses a download it
# has no line for. Nothing cross-compiles Linux and macOS, so CI builds each on
# its own runner and stages them here before the checksums are written.
#
# STAGING_ROOT/<package> holds those artifacts in the release job. Without it
# this is a local run and this host's binary is the whole release.
set -euo pipefail

: "${DISPAT_PACKAGE:?must be run by dispat}"
: "${DISPAT_NEW_VERSION:?must be run by dispat}"
: "${DISPAT_OUTPUT:?must be run by dispat}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"

# Fresh every time: `tar` only adds, so a leftover tarball from an earlier
# version would ship as though this build had produced it.
rm -rf release
mkdir -p release

staged="${STAGING_ROOT:-}/${DISPAT_PACKAGE}"
if [[ -n "${STAGING_ROOT:-}" && -d "$staged" ]]; then
  while IFS= read -r -d '' file; do
    base="$(basename "$file")"
    # A clash means a runner ignored its platform; one binary would win silently.
    if [[ -e "release/$base" ]]; then
      echo "error: two platforms both produced $base" >&2
      exit 1
    fi
    cp "$file" "release/$base"
  done < <(find "$staged" -type f -print0)
else
  cargo build --release
  tar -czf "release/$DISPAT_PACKAGE-$DISPAT_NEW_VERSION-$os-$arch.tar.gz" -C target/release farmers
fi

shopt -s nullglob
artifacts=(release/*)
shopt -u nullglob
if [[ ${#artifacts[@]} -eq 0 ]]; then
  echo "error: nothing to release for $DISPAT_PACKAGE $DISPAT_NEW_VERSION" >&2
  exit 1
fi

# One file over every platform's artifacts, from one implementation.
sums() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
( cd release && sums -- * > SHA256SUMS )

# dispat uploads the absolute paths this names; $PWD is the package folder.
echo "DISPAT_EXPORT_GITHUB=$(ls -d "$PWD/release/"* | tr '\n' ' ')" >> "$DISPAT_OUTPUT"
echo "attaching $(ls -1 release | wc -l | tr -d ' ') asset(s) to $DISPAT_PACKAGE $DISPAT_NEW_VERSION"
