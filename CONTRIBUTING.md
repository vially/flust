# Contributing guideline to Flust

Thank you for your interest in contributing to Flust! We have many areas
that could use some help.

- Reporting and fixing platform specific bugs.
- Desktop intergration: We provide several plugins to intergrate with native
  desktop UI, such as clipboard, dialog... but more is needed.
- Flutter Desktop GUI: Flutter is mainly for mobile. Desktop app need desktop
  widgets, such as context menu.
- Documentation.

## Structure
This project is cargo workspace with multiple targets.

- `flust-winit` creates an event loop and a window and interfaces with the
  `flust-engine` crate.

- `flust-engine` is the library that make flutter run. It provides a
  `MethodChannel` struct to iterop with flutter and dart.

    When an engine instance is run, a `platform_message_callback` is passed to
    flutter engine using C ffi. A registry listens to flutter `MethodChannel`
    calls. Various plugins are registered with the registry using the `add_plugin`
    method. When flutter makes a native platform request, the callback is called
    and processed by a registered plugin.

- `flust-engine-sys` is the crate for ffi with flutter engine C apis. It
  generates bindings using bindgen.

## Reference
- [Custom-Flutter-Engine-Embedders](https://github.com/flutter/flutter/wiki/Custom-Flutter-Engine-Embedders)
- [Desktop Embedding for Flutter](https://github.com/google/flutter-desktop-embedding)
