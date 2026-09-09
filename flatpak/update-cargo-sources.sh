#!/bin/sh
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
lock="$root/Cargo.lock"
output="$root/flatpak/cargo-sources.json"
generator_url="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py"

if [ ! -f "$lock" ]; then
  echo "Cargo.lock is missing. Run cargo generate-lockfile first." >&2
  exit 1
fi

venv="$root/flatpak/.generator-venv"
if [ ! -x "$venv/bin/python" ]; then
  python3 -m venv "$venv"
  "$venv/bin/pip" install --disable-pip-version-check tomlkit aiohttp
fi

generator="$root/flatpak/flatpak-cargo-generator.py"
if [ ! -f "$generator" ]; then
  curl -fsSL "$generator_url" -o "$generator"
fi

"$venv/bin/python" "$generator" "$lock" -o "$output"
echo "Wrote $output"
