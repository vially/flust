#!/usr/bin/env bash
# Build the probe app (tests/e2e/app) and put its Flutter bundle next to the
# binary, where the scenarios run it from: app/target/debug.
#
#     build-app.sh              # the probe app
#     build-app.sh windowing    # the app with windows of its own
#
# The app with windows of its own (tests/e2e/windowing-app) uses Flutter's
# windowing API, which only Flutter's master channel has: `FLUTTER_ROOT` has
# to point at a checkout of it (see README.md). It is built with the
# windowing feature on, against the engine library that Flutter builds for
# that commit.
set -euo pipefail

here="$(dirname "$(readlink -f "$0")")"
case "${1:-probe}" in
probe)
    cd "$here/app"
    ;;
windowing)
    if [ -z "${FLUTTER_ROOT:-}" ]; then
        echo "FLUTTER_ROOT has to point at a checkout of Flutter's master channel" >&2
        exit 1
    fi
    export PATH="$FLUTTER_ROOT/bin:$PATH"
    export FLUTTER_WINDOWING=true
    export FLUTTER_ENGINE_DOWNLOAD_SOURCE=flutter-infra
    cd "$here/windowing-app"
    ;;
*)
    echo "usage: $0 [probe|windowing]" >&2
    exit 1
    ;;
esac

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

echo "built: $(pwd)/target/debug"
