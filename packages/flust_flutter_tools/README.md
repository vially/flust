# flust_flutter_tools

A [tool extension] for the `flutter` command: with it, a Flutter app with a
Flust runner is built and run like any other Flutter app.

```sh
flutter run -d flust              # debug: hot reload, hot restart, DevTools
flutter run -d flust --profile
flutter run -d flust --release
flutter build flust [--release]   # build/flust/<mode>/bundle
```

It provides:

- the `flust` device, the local Wayland session (listed when
  `WAYLAND_DISPLAY` is set);
- `flutter build flust`, which builds the Dart side (kernel, or AOT
  `libapp.so` in profile and release), then the runner with Cargo, and lays
  out a bundle that runs on its own:

  ```
  build/flust/<mode>/bundle/<binary>
  build/flust/<mode>/bundle/data/flutter_assets/
  build/flust/<mode>/bundle/data/icudtl.dat
  build/flust/<mode>/bundle/lib/libapp.so            (profile and release)
  build/flust/<mode>/bundle/lib/libflutter_engine.so
  ```

- a `Flust` section in `flutter doctor`.

`flutter run` options that become engine switches (`--start-paused`,
`--dart-flags`, `--no-enable-impeller`, …) reach the engine, and
`--dart-entrypoint-args` reach the binary, which passes them to Dart if the
runner calls `with_dart_entrypoint_args`.

Tool extensions are an experiment of Flutter's master channel ([RFC 510],
tracked in https://github.com/flutter/flutter/issues/190755), and parts of
them are still open pull requests. Until they land, the extension runs on a
fork of Flutter that carries them: see [Setup](#setup). Progress on the
Flust side is tracked in https://forge.lab.vially.dev/vially/flust/issues/24.

## Using it in an app

Add the package to the app's `dev_dependencies`:

```yaml
dev_dependencies:
  flust_flutter_tools:
    path: <flust repository>/packages/flust_flutter_tools
```

and run `flutter pub get`: the `flutter` tool finds the extension in the
app's resolved dependencies, so until they are resolved, `flutter run -d
flust` doesn't know the device and `flutter build flust` doesn't exist.

The runner is the Cargo package in `flust/` (with `flust/Cargo.toml`) or,
if there is none, at the project root, as in [`examples/`](../../examples).
Its binary loads the bundle from next to itself, which is what
`Application::builder()` does by default.

## Setup

1. **A Flutter fork with tool extensions.** The fork is the
   [`flust/main`](https://github.com/vially/flutter/tree/flust/main) branch
   of [vially/flutter](https://github.com/vially/flutter): Flutter's master
   channel at a pinned commit, plus the open pull requests the extension
   needs (see [`tools/flutter-fork`](../../tools/flutter-fork)). Clone
   Flutter itself, then check out that branch from the fork:

   ```sh
   git clone https://github.com/flutter/flutter.git ~/code/flutter-tool-ext
   cd ~/code/flutter-tool-ext
   git remote rename origin upstream
   git remote add fork https://github.com/vially/flutter.git
   git fetch fork flust/main
   git switch -c flust/main fork/flust/main
   ```

   The branch must not be called `master` or `main`, and the Flutter
   remote must be called `upstream`: the engine version is derived from the
   merge-base with `upstream/master`.

2. **Make it the `flutter` on your `PATH`**, and enable tool extensions (a
   global setting that only master-channel SDKs read):

   ```sh
   ln -sfn ~/code/flutter-tool-ext/bin/flutter ~/.local/bin/flutter
   flutter config --enable-tool-extensions
   ```

3. **Link it from the flust repository**, where this package resolves the
   unpublished extension API (`flutter_tools_extension`,
   `flutter_tools_core`) from:

   ```sh
   tools/flutter-fork link ~/code/flutter-tool-ext
   ```

   This creates `.flutter-sdk` at the repository root (not tracked).

4. **Engine libraries.** Debug engines are downloaded for any engine
   version on the first build. Profile and release engines are only
   published for released Flutter versions, so for the fork they have to be
   built once per engine version: about 15 minutes of compiling for both
   modes, after a first download of the engine's dependencies (a ~26 GB
   checkout, reused by later builds):

   ```sh
   tools/flutter-fork build-engine
   ```

`tools/flutter-fork status` shows the fork's base, its engine version, the
state of each upstream pull request it carries, and which engine libraries
are in the cache.

## Updating the fork

The fork's base stays pinned until it is moved on purpose: the engine
version follows the base, and each new engine version needs its profile and
release engines built again.

```sh
tools/flutter-fork rebase              # onto upstream/master
tools/flutter-fork rebase <commit>     # or onto a given upstream commit
tools/flutter-fork build-engine        # if it says the engine changed
```

`rebase` drops the carried pull requests that have been merged upstream and
keeps a backup ref (`refs/flust-fork/backup/…`) of the previous stack. Push
the result with `git push --force-with-lease fork flust/main`.

## Development

The `flutter` tool compiles the extension from this directory each time it
changes (into the app's `.dart_tool/flutter_tools/extensions/`), so an edit
here is picked up by the next `flutter` command; nothing needs rebuilding.
`flutter run -v` shows the extension's trace output.

```sh
cd packages/flust_flutter_tools
flutter pub get
dart analyze
```

[tool extension]: https://github.com/flutter/rfc/pull/40
[RFC 510]: https://github.com/flutter/rfc/pull/40
