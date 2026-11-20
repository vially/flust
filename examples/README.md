# Examples

Flutter apps with a Flust runner, one per directory. Each one shows one
part of Flust, and its own `README.md` says which.

| Example | Shows |
| --- | --- |
| [`hello`](hello) | The smallest app: one window, built with `Application::builder` |

These are documentation, meant to be read and copied from. The apps that
Flust's end-to-end tests drive are in [`tests/e2e`](../tests/e2e).

## Layout

Each example is a Flutter project and a Rust crate in the same directory:
`pubspec.yaml` and `lib/` for the Dart side, `Cargo.toml` and `src/` for
the runner. The crate is called `flust-example-<name>`, and the Dart package
`flust_example_<name>`.

Each example is a Cargo workspace of its own, not a member of the
repository's. Flust loads an app's Flutter bundle from next to its binary
(`data/` and `lib/`), so two apps sharing a target directory would
overwrite each other's bundle.

## Running

You need the Flutter SDK, a Rust toolchain and a Wayland session. From an
example's directory, build the Flutter bundle and the runner:

```sh
flutter build bundle --debug
cargo build
```

Then put the bundle, Flutter's ICU data and the engine library next to the
binary, and run it:

```sh
flutter_root="$(dirname "$(dirname "$(readlink -f "$(command -v flutter)")")")"
mkdir -p target/debug/data target/debug/lib
rm -rf target/debug/data/flutter_assets
cp -r build/flutter_assets target/debug/data/
cp "$flutter_root/bin/cache/artifacts/engine/linux-x64/icudtl.dat" target/debug/data/
engine="$(find target/debug -name libflutter_engine.so -not -path 'target/debug/lib/*' | head -n 1)"
ln -sf "$(readlink -f "$engine")" target/debug/lib/libflutter_engine.so

LD_LIBRARY_PATH=target/debug/lib target/debug/flust-example-<name>
```

These steps will be replaced by a single command, see
https://forge.lab.vially.dev/vially/flust/issues/23.
