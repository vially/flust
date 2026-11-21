/// Tool extension for Flust (https://forge.lab.vially.dev/vially/flust),
/// a Rust Flutter embedder for Wayland built on the embedder API.
///
/// A Flust app is a Flutter project with a Cargo package at its root (a
/// `Cargo.toml` next to `pubspec.yaml`) whose binary loads the Flutter bundle
/// from next to itself: `data/flutter_assets`, `data/icudtl.dat`,
/// `lib/libapp.so` and `lib/libflutter_engine.so`.
library flust_flutter_tools;

import 'package:flutter_tools_extension/flutter_tools_extension.dart';

import 'src/build.dart';
import 'src/device.dart';
import 'src/diagnostics.dart';

/// Isolate entrypoint for the Flust tool extension.
Future<void> flustExtensionEntryPoint(Object? sendPort) {
  return ToolExtensionEntryPoint.run(
    sendPort,
    <ToolExtensionService>[FlustDiagnostics(), FlustDeviceService(), FlustBuildService()],
    extensionName: 'flust_flutter_tools',
    supportedPlatforms: const <String>{'linux'},
  );
}
