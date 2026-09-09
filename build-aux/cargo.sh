#!/bin/sh
set -eu

source_root="$1"
build_root="$2"
output="$3"
target="$4"
shift 4

case "$output" in
  /*) ;;
  *) output="$PWD/$output" ;;
esac

if [ -z "${CARGO_HOME+x}" ]; then
  export CARGO_HOME="${build_root}/cargo-home"
fi
export CARGO_TARGET_DIR="${build_root}/target"

if [ "${CARGO_NET_OFFLINE-}" = true ]; then
  set -- --offline "$@"
fi

cd "$source_root"
cargo build "$@"
cp "${CARGO_TARGET_DIR}/${target}/marshal" "$output"
