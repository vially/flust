import 'dart:convert';
import 'dart:io';

import 'package:flutter_tools_core/flutter_tools_core.dart';
import 'package:flutter_tools_extension/flutter_tools_extension.dart';

import 'project.dart';

// Flust apps are host builds for now: the Dart side is built exactly like a
// Linux desktop app (same kernel, same AOT snapshot), only the runner differs.
final String _kTargetPlatform = hostTargetPlatform;

/// `flutter build flust`: the Dart bundle, the Cargo binary and the engine
/// library, laid out the way Flust loads them:
///
/// ```
/// bundle/<binary>
/// bundle/data/flutter_assets/
/// bundle/data/icudtl.dat
/// bundle/lib/libapp.so              (profile and release)
/// bundle/lib/libflutter_engine.so
/// ```
final class FlustBuildTarget extends BuildTarget {
  FlustBuildTarget()
    : super(
        description: 'Build a Flust (Rust, Wayland) application.',
        outputDir: '${BuiltInPlaceholders.outputDir}/flust/${BuiltInPlaceholders.buildMode}',
        targetPlatform: _kTargetPlatform,
      );

  @override
  String get name => FlustBuildService.buildTargetName;

  @override
  Set<ExtensionArtifact> get artifacts => const <ExtensionArtifact>{BuiltInArtifacts.icuData};

  @override
  Future<ExtensionBuildResult> build(ExtensionBuildContext context) async {
    final projectDir = Directory.fromUri(context.projectRoot);
    final Directory? runnerDir = findRunnerDirectory(projectDir);
    if (runnerDir == null) {
      return ExtensionBuildResult(
        success: false,
        errorMessage: 'No Cargo.toml in ${projectDir.path}/flust or ${projectDir.path}.',
      );
    }
    final String flutterRoot = Directory.fromUri(context.flutterRoot).path;
    final bundleDir = Directory('${Directory.fromUri(context.outputDir).path}/bundle');
    final dataDir = Directory('${bundleDir.path}/data');
    final libDir = Directory('${bundleDir.path}/lib');
    final String mode = context.buildMode.cliName;

    // 1. The Dart side, through this extension's assemble targets: kernel and
    //    assets in debug, plus the AOT snapshot (libapp.so) otherwise.
    final dartOut = Directory('${Directory.fromUri(context.outputDir).path}/dart');
    final String flutter = '$flutterRoot/bin/flutter';
    stdout.writeln('Building the Dart bundle ($mode)...');
    final int assembleExitCode = await _run(flutter, <String>[
      'assemble',
      '--no-version-check',
      '--output=${dartOut.path}',
      '-dTargetPlatform=$_kTargetPlatform',
      '-dBuildMode=$mode',
      '-dTargetFile=${context.mainPath}',
      '-dTrackWidgetCreation=${context.trackWidgetCreation}',
      '-dTreeShakeIcons=${context.treeShakeIcons}',
      '-dDartObfuscation=${context.dartObfuscation}',
      if (context.flavor case final String flavor when flavor.isNotEmpty) '-dFlavor=$flavor',
      if (context.buildName case final String buildName) '-dBuildName=$buildName',
      if (context.buildNumber case final String buildNumber) '-dBuildNumber=$buildNumber',
      if (context.splitDebugInfo case final Uri splitDebugInfo)
        '-dSplitDebugInfo=${splitDebugInfo.toFilePath()}',
      if (context.dartDefines.isNotEmpty)
        '--DartDefines=${_encodeDartDefines(context.dartDefines)}',
      if (context.extraFrontEndOptions.isNotEmpty)
        '--ExtraFrontEndOptions=${context.extraFrontEndOptions.join(',')}',
      if (context.extraGenSnapshotOptions.isNotEmpty)
        '--ExtraGenSnapshotOptions=${context.extraGenSnapshotOptions.join(',')}',
      FlustDartBundle.targetName(context.buildMode),
    ], workingDirectory: projectDir.path);
    if (assembleExitCode != 0) {
      return ExtensionBuildResult(
        success: false,
        errorMessage: 'flutter assemble failed with exit code $assembleExitCode.',
      );
    }

    // 2. The Rust side. `flust-engine-sys` picks the engine library matching
    //    the SDK at FLUTTER_ROOT. Flutter's own storage only has it in debug
    //    mode, but it has it for every engine version (master included).
    stdout.writeln('Running cargo build...');
    // The engine library's mode follows FLUST_BUILD_MODE, not Cargo's profile.
    final environment = <String, String>{
      'FLUTTER_ROOT': flutterRoot,
      'FLUST_BUILD_MODE': mode,
      if (!Platform.environment.containsKey('FLUTTER_ENGINE_DOWNLOAD_SOURCE') &&
          context.buildMode == BuildMode.debug)
        'FLUTTER_ENGINE_DOWNLOAD_SOURCE': 'flutter-infra',
    };
    final (int cargoExitCode, List<String> executables, List<String> engineLibraryDirs) =
        await _cargoBuild(
      release: context.buildMode != BuildMode.debug,
      workingDirectory: runnerDir.path,
      environment: environment,
    );
    if (cargoExitCode != 0) {
      return ExtensionBuildResult(
        success: false,
        errorMessage: 'cargo build failed with exit code $cargoExitCode.',
      );
    }
    if (executables.length != 1) {
      return ExtensionBuildResult(
        success: false,
        errorMessage: 'Expected one binary from cargo build, got: $executables.',
      );
    }
    final executable = File(executables.single);

    // 3. The bundle.
    if (bundleDir.existsSync()) {
      bundleDir.deleteSync(recursive: true);
    }
    dataDir.createSync(recursive: true);
    libDir.createSync(recursive: true);
    final flutterAssets = Directory('${dataDir.path}/flutter_assets');
    _copyDirectory(Directory('${dartOut.path}/flutter_assets'), flutterAssets);
    if (context.buildMode == BuildMode.debug) {
      File('${dartOut.path}/$_kKernelBlob').copySync('${flutterAssets.path}/$_kKernelBlob');
    } else {
      File('${dartOut.path}/lib/$_kLibApp').copySync('${libDir.path}/$_kLibApp');
    }
    final String? icuData = context.resolvedArtifacts[BuiltInArtifacts.icuData.name];
    if (icuData == null) {
      return const ExtensionBuildResult(success: false, errorMessage: 'Missing icuData artifact.');
    }
    File(icuData).copySync('${dataDir.path}/icudtl.dat');

    final File? engine = _findEngineLibrary(engineLibraryDirs);
    if (engine == null) {
      return ExtensionBuildResult(
        success: false,
        errorMessage: 'libflutter_engine.so not found in the linked paths of cargo build: '
            '$engineLibraryDirs.',
      );
    }
    engine.copySync('${libDir.path}/libflutter_engine.so');

    final String binaryName = executable.uri.pathSegments.last;
    final File bundled = executable.copySync('${bundleDir.path}/$binaryName');
    return ExtensionBuildResult(success: true, executableUri: bundled.absolute.uri);
  }
}

/// [BuildService] contributing `flutter build flust`.
final class FlustBuildService extends BuildService {
  static const String buildTargetName = 'flust';

  @override
  List<BuildTarget> get buildTargets => <BuildTarget>[FlustBuildTarget()];

  @override
  List<AssembleTarget> get assembleTargets => <AssembleTarget>[
    FlustDartBundle(BuildMode.debug),
    FlustDartBundle(BuildMode.profile),
    FlustDartBundle(BuildMode.release),
  ];
}

const String _kKernelBlob = 'kernel_blob.bin';
const String _kLibApp = 'libapp.so';

/// `flutter assemble flust_dart_bundle_<mode>`: the Dart side of a Flust
/// bundle, without the GTK runner that Flutter's Linux bundle targets unpack.
///
/// ```text
/// <output>/flutter_assets  -> {BUILD_DIR}/flutter_assets (symlink)
/// <output>/kernel_blob.bin    (debug)
/// <output>/lib/libapp.so      (profile and release)
/// ```
///
/// There is no built-in AOT target yet, so profile and release depend on
/// Flutter's internal `aot_snapshot_<mode>` targets by name
/// (https://forge.lab.vially.dev/vially/flust/issues/29). The host registers
/// them for android-arm, so the cache hashes that `gen_snapshot`, but they run
/// the one for `-dTargetPlatform`.
final class FlustDartBundle extends AssembleTarget {
  FlustDartBundle(this.buildMode)
    : super(
        description: 'The Dart side of a Flust bundle: assets, plus the kernel or AOT snapshot.',
        targetPlatform: _kTargetPlatform,
      );

  final BuildMode buildMode;

  static String targetName(BuildMode buildMode) => 'flust_dart_bundle_${buildMode.cliName}';

  @override
  String get name => targetName(buildMode);

  @override
  List<String> get dependencies => <String>[
    BuiltInTargets.copyAssets,
    if (buildMode == BuildMode.debug)
      BuiltInTargets.kernelSnapshot
    else
      'aot_snapshot_${buildMode.cliName}',
  ];

  @override
  List<ExtensionSource> get inputs => <ExtensionSource>[
    ExtensionSource.pattern('${BuiltInPlaceholders.buildDir}/${_snapshot.$1}'),
  ];

  @override
  List<ExtensionSource> get outputs => <ExtensionSource>[
    ExtensionSource.pattern('${BuiltInPlaceholders.outputDir}/${_snapshot.$2}'),
  ];

  /// The snapshot in the build directory, and where it goes in the output.
  (String, String) get _snapshot =>
      buildMode == BuildMode.debug ? ('app.dill', _kKernelBlob) : ('app.so', 'lib/$_kLibApp');

  @override
  Future<ExtensionBuildResult> build(ExtensionAssembleContext context) async {
    // Assets are linked rather than copied: an extension target can't declare
    // copy_assets' depfile, so a copy would go stale when only an asset changes
    // (this target only reruns when the snapshot does).
    final flutterAssets = Link.fromUri(context.outputDir.resolve('flutter_assets'));
    final String assetsSource = context.buildDir.resolve('flutter_assets').toFilePath();
    final FileSystemEntityType type = FileSystemEntity.typeSync(
      flutterAssets.path,
      followLinks: false,
    );
    if (type == FileSystemEntityType.link) {
      flutterAssets.updateSync(assetsSource);
    } else {
      if (type != FileSystemEntityType.notFound) {
        // A real directory, left by an earlier version of this extension.
        Directory(flutterAssets.path).deleteSync(recursive: true);
      }
      flutterAssets.createSync(assetsSource, recursive: true);
    }

    final (String snapshot, String destination) = _snapshot;
    final source = File.fromUri(context.buildDir.resolve(snapshot));
    if (!source.existsSync()) {
      return ExtensionBuildResult(success: false, errorMessage: 'Missing ${source.path}.');
    }
    final target = File.fromUri(context.outputDir.resolve(destination));
    target.parent.createSync(recursive: true);
    source.copySync(target.path);
    return const ExtensionBuildResult(success: true);
  }
}

String _encodeDartDefines(Map<String, String> defines) => <String>[
  for (final MapEntry<String, String> entry in defines.entries)
    base64.encode(utf8.encode('${entry.key}=${entry.value}')),
].join(',');

/// The engine library that was actually linked, found through the search
/// paths `flust-engine-sys`'s build script gave Cargo. Searching the target
/// directory instead can pick up a stale library from an earlier build with a
/// different SDK, whose Dart VM then rejects the kernel.
File? _findEngineLibrary(List<String> linkedPaths) {
  for (final dir in linkedPaths) {
    final library = File('$dir/libflutter_engine.so');
    if (library.existsSync()) {
      return File(library.resolveSymbolicLinksSync());
    }
  }
  return null;
}

/// Runs `cargo build` and returns the paths of the binaries it produced and
/// the library search paths of the build scripts that link the engine.
Future<(int, List<String>, List<String>)> _cargoBuild({
  required bool release,
  required String workingDirectory,
  required Map<String, String> environment,
}) async {
  final Process process = await Process.start(
    'cargo',
    <String>['build', if (release) '--release', '--message-format=json-render-diagnostics'],
    workingDirectory: workingDirectory,
    environment: environment,
  );
  final executables = <String>[];
  final engineLibraryDirs = <String>[];
  final Future<void> stdoutDone = process.stdout
      .transform(const Utf8Decoder(allowMalformed: true))
      .transform(const LineSplitter())
      .listen((String line) {
        final Object? message;
        try {
          message = json.decode(line);
        } on FormatException {
          stdout.writeln(line);
          return;
        }
        if (message case {
          'reason': 'compiler-artifact',
          'executable': final String path,
          'target': {'kind': final List<Object?> kind},
        } when kind.contains('bin')) {
          executables.add(path);
        } else if (message case {
          'reason': 'build-script-executed',
          'linked_libs': final List<Object?> linkedLibs,
          'linked_paths': final List<Object?> linkedPaths,
        } when linkedLibs.contains('flutter_engine')) {
          // Entries may carry a `KIND=` prefix, as in `native=/path`.
          engineLibraryDirs.addAll(
            linkedPaths.whereType<String>().map((String p) => p.replaceFirst(RegExp(r'^\w+='), '')),
          );
        }
      })
      .asFuture<void>();
  final Future<void> stderrDone = process.stderr.listen(stderr.add).asFuture<void>();
  final int exitCode = await process.exitCode;
  await (stdoutDone, stderrDone).wait;
  return (exitCode, executables, engineLibraryDirs);
}

Future<int> _run(String executable, List<String> arguments, {String? workingDirectory}) async {
  final Process process = await Process.start(
    executable,
    arguments,
    workingDirectory: workingDirectory,
  );
  final (int exitCode, _, _) = await (
    process.exitCode,
    process.stdout.listen(stdout.add).asFuture<void>(),
    process.stderr.listen(stderr.add).asFuture<void>(),
  ).wait;
  return exitCode;
}

void _copyDirectory(Directory source, Directory destination) {
  destination.createSync(recursive: true);
  for (final FileSystemEntity entity in source.listSync(followLinks: false)) {
    final String name = entity.uri.pathSegments.lastWhere((String e) => e.isNotEmpty);
    if (entity is Directory) {
      _copyDirectory(entity, Directory('${destination.path}/$name'));
    } else if (entity is File) {
      entity.copySync('${destination.path}/$name');
    }
  }
}
