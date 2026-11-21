#!/usr/bin/env bash
# Build the probe app (tests/e2e/app) with `flutter build flust`, into
# app/build/flust/debug/bundle, where the scenarios run it from.
#
#     build-app.sh              # the probe app
#     build-app.sh windowing    # the app with windows of its own
#
# `flutter` has to be a Flutter SDK with tool extensions and the flust tool
# extension's setup done: see packages/flust_flutter_tools/README.md.
#
# The app with windows of its own (tests/e2e/windowing-app) uses Flutter's
# windowing API, which only Flutter's master channel has, and is built with
# the windowing feature on.
set -euo pipefail

here="$(dirname "$(readlink -f "$0")")"
export FLUTTER_TOOL_EXTENSIONS=true
case "${1:-probe}" in
probe)
    cd "$here/app"
    ;;
windowing)
    export FLUTTER_WINDOWING=true
    cd "$here/windowing-app"
    ;;
*)
    echo "usage: $0 [probe|windowing]" >&2
    exit 1
    ;;
esac

# The tool finds the extension in the app's resolved dependencies, so
# resolve them before `flutter build flust` is parsed.
flutter pub get
flutter build flust --debug

echo "built: $(pwd)/build/flust/debug/bundle"
