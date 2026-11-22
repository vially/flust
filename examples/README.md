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
repository's, the way an app's runner would be: it has its own
`Cargo.lock` and `target/`, and builds the same way out of this
repository.

## Running

You need a Rust toolchain, a Wayland session, and Flutter with
[the flust tool extension](../packages/flust_flutter_tools/README.md#setup)
set up. From an example's directory:

```sh
flutter pub get
flutter run -d flust              # or --profile, --release
```

`flutter build flust` builds the same bundle without running it, in
`build/flust/<mode>/bundle`.
