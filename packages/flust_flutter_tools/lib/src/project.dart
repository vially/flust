import 'dart:ffi';
import 'dart:io';

/// The Cargo package that builds a Flust app's binary (its "runner").
///
/// Looked up in `flust/` first, the platform directory the `flutter create`
/// template will generate, then at the project root, where the examples keep
/// it today.
Directory? findRunnerDirectory(Directory projectDirectory) {
  final String root = projectDirectory.path.replaceFirst(RegExp(r'/+$'), '');
  for (final String path in <String>['$root/flust', root]) {
    if (File('$path/Cargo.toml').existsSync()) {
      return Directory(path);
    }
  }
  return null;
}

/// The Flutter target platform of the host: Flust apps are built for the
/// machine they run on.
String get hostTargetPlatform => switch (Abi.current()) {
  Abi.linuxArm64 => 'linux-arm64',
  Abi.linuxRiscv64 => 'linux-riscv64',
  _ => 'linux-x64',
};
