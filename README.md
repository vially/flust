# Flust

[![Crates.io][crates-badge]][crates-url]
[![flutter version][flutter-badge]][flutter-url]
[![Discord chat][discord-badge]][discord-url]
[![MIT licensed][mit-badge]][mit-url]

Build flutter desktop app in dart & rust.

![flutter-app-template][flutter-app-template]

# Get Started

## Install requirements

- [Rust](https://www.rust-lang.org/tools/install)

- [flutter sdk](https://flutter.io)

## Develop
- install the `cargo` `flutter` command

    `cargo install cargo-flutter`
    
- create your new project from the template

    `git clone https://github.com/flutter-rs/flutter-app-template`

- To develop with cli hot-reloading:

    `cd flutter-app-template`
    
    `cargo flutter run`

## Distribute
- To build distribution, use:
    `cargo flutter --format appimage build --release`

# Contribution
To contribute to Flust, please see [CONTRIBUTING](CONTRIBUTING.md).

# ChangeLog
[CHANGELOG](CHANGELOG.md).

[flutter-badge]: https://img.shields.io/badge/flutter-v1.9.1-blueviolet.svg
[flutter-url]: https://flutter.dev/
[discord-badge]: https://img.shields.io/discord/743549843632423053?label=discord
[discord-url]: https://discord.gg/WwdAE6p
[crates-badge]: https://img.shields.io/crates/v/flust-engine.svg
[crates-url]: https://crates.io/crates/flust-engine
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: LICENSE-MIT
[flutter-app-template]: https://user-images.githubusercontent.com/741807/72476798-5a99e280-37ee-11ea-9e08-b0175ae21ad6.png

# Acknowledgements

[`flutter-rs`](https://github.com/flutter-rs/flutter-rs) - For providing the solid foundation that Flust was originally forked from and builds on top of. The Flust project would not be possible without the awesome work of the `flutter-rs` contributors.

Flust started as a couple of changes on top of the `flutter-rs` project, with the intention of merging them back into upstream. However, by the time the patches turned into something that might be usable upstream, the `flutter-rs` project was [no longer maintained](https://github.com/flutter-rs/flutter-rs/issues/156#issuecomment-859392523) so a decision was made to fork the project and rename it to Flust.
