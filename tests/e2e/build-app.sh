#!/usr/bin/env bash
# Build the probe app (tests/e2e/app) and put its Flutter bundle next to the
# binary, where the scenarios run it from: app/target/debug.
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")/app"

flutter_root="$(dirname "$(dirname "$(readlink -f "$(command -v flutter)")")")"
icu_data="$flutter_root/bin/cache/artifacts/engine/linux-x64/icudtl.dat"

flutter build bundle --debug
cargo build

data=target/debug/data
rm -rf "$data"
mkdir -p "$data"
cp -r build/flutter_assets "$data/flutter_assets"
cp "$icu_data" "$data/icudtl.dat"

# The scenarios load the engine library from `lib`, next to the binary.
# `flust-engine-sys` links it into the target directory, at a place that
# depends on cargo's build directory layout, so look for it.
engine="$(find target/debug -name libflutter_engine.so -not -path 'target/debug/lib/*' | head -n 1)"
if [ -z "$engine" ]; then
    echo "libflutter_engine.so not found under target/debug" >&2
    exit 1
fi
mkdir -p target/debug/lib
ln -sf "$(readlink -f "$engine")" target/debug/lib/libflutter_engine.so

echo "built: $(pwd)/target/debug/flust-e2e-probe"
