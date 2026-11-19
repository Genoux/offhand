#!/usr/bin/env bash
# Install the latest stable Offhand release without cloning or building:
#
#   curl -fsSL https://raw.githubusercontent.com/Genoux/offhand/main/packaging/get.sh | bash
#
# Flags after `bash -s --` reach packaging/install.sh. Running it again updates.
set -euo pipefail

api="${OFFHAND_RELEASES_API:-https://api.github.com/repos/Genoux/offhand/releases/latest}"
download="${OFFHAND_DOWNLOAD:-https://github.com/Genoux/offhand/releases/download}"
kept="$HOME/.local/share/offhand/packaging"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-linux ;;
  Darwin-arm64) target=aarch64-macos ;;
  *)
    echo "offhand: no release build for $(uname -s) $(uname -m) - build from source, see the README" >&2
    exit 1
    ;;
esac

# `releases/latest` skips prereleases by definition, so this is always stable.
tag="$(curl -fsSL "$api" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
if [ -z "$tag" ]; then
  echo "offhand: could not find the latest release" >&2
  exit 1
fi

name="offhand-$tag-$target"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
echo "Downloading Offhand $tag"
curl -fsSL -o "$work/$name.tar.gz" "$download/$tag/$name.tar.gz"
curl -fsSL -o "$work/$name.tar.gz.sha256" "$download/$tag/$name.tar.gz.sha256"
if command -v sha256sum >/dev/null; then
  (cd "$work" && sha256sum -c --status "$name.tar.gz.sha256")
else
  (cd "$work" && shasum -a 256 -c -s "$name.tar.gz.sha256")
fi || {
  echo "offhand: the download does not match its checksum - nothing was installed" >&2
  exit 1
}

tar -xzf "$work/$name.tar.gz" -C "$work"
bash "$work/$name/packaging/install.sh" "$@"

# The unpacked release is deleted on exit, and the macOS uninstaller finds its
# files beside itself, so the scripts are kept where a user can reach them.
rm -rf "$kept"
mkdir -p "$(dirname "$kept")"
cp -R "$work/$name/packaging" "$kept"
echo "To remove Offhand later: $kept/uninstall.sh"
