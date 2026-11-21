import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_tools_core/flutter_tools_core.dart';
import 'package:flutter_tools_extension/flutter_tools_extension.dart';

import 'build.dart';
import 'project.dart';

class _RunningApp {
  _RunningApp({required this.appId, required this.process, required this.vmServiceCompleter});

  final String appId;
  final Process process;
  final Completer<Uri?> vmServiceCompleter;
}

/// The local Wayland session as a Flust device.
final class FlustDeviceService extends DeviceService {
  static const String deviceId = 'flust';

  // Mirrors kVMServiceMessageRegExp in package:flutter_tools/src/vmservice.dart.
  static final _vmServicePattern = RegExp(
    r'The Dart VM service is listening on ((http|//)[a-zA-Z0-9:/=_\-\.\[\]]+)',
  );

  /// Lines kept for a log reader that starts after the app printed them.
  static const int _kMaxPendingLogLines = 1000;

  final _runningApps = <String, _RunningApp>{};
  final _logStreams = <String>{};
  final _pendingLogLines = <String>[];

  @override
  Future<List<TargetDevice>> getDevices() async {
    final String? wayland = Platform.environment['WAYLAND_DISPLAY'];
    if (wayland == null || wayland.isEmpty) {
      return const <TargetDevice>[];
    }
    return <TargetDevice>[
      TargetDevice(
        category: Category.desktop,
        id: deviceId,
        name: 'Flust',
        buildTarget: FlustBuildService.buildTargetName,
        ephemeral: false,
        sdkNameAndVersion: 'Flust (Wayland: $wayland)',
        targetPlatform: hostTargetPlatform,
      ),
    ];
  }

  @override
  Future<bool> isSupportedForProject({
    required String deviceId,
    required ExtensionFlutterProject project,
  }) async {
    return deviceId == FlustDeviceService.deviceId &&
        findRunnerDirectory(Directory.fromUri(project.directory)) != null;
  }

  @override
  Future<void> launchApp({
    required String appId,
    required ExtensionDebuggingOptions debuggingOptions,
    required String deviceId,
    required Uri executableUri,
    bool prebuiltApplication = false,
  }) async {
    if (deviceId != FlustDeviceService.deviceId) {
      throw ToolExtensionException('Unknown device "$deviceId".');
    }
    final executable = File.fromUri(executableUri);
    if (!executable.existsSync()) {
      throw ToolExtensionException('Application executable does not exist: ${executable.path}');
    }
    await stopApp(deviceId: deviceId);
    _pendingLogLines.clear();

    // The binary finds the engine library through its run path (see
    // `flutter build flust`). Engine switches use the environment variables
    // of Flutter's Linux embedder.
    final environment = <String, String>{
      for (var i = 0; i < debuggingOptions.engineSwitches.length; i++)
        'FLUTTER_ENGINE_SWITCH_${i + 1}': debuggingOptions.engineSwitches[i],
      if (debuggingOptions.engineSwitches.isNotEmpty)
        'FLUTTER_ENGINE_SWITCHES': '${debuggingOptions.engineSwitches.length}',
    };

    final uriCompleter = Completer<Uri?>();
    try {
      final Process process = await Process.start(
        executable.path,
        debuggingOptions.dartEntrypointArgs,
        environment: environment,
        workingDirectory: executable.parent.path,
      );
      final runningApp = _RunningApp(
        appId: appId,
        process: process,
        vmServiceCompleter: uriCompleter,
      );
      _runningApps[deviceId] = runningApp;

      void onLine(String line) {
        final Match? match = _vmServicePattern.firstMatch(line);
        if (match != null && !uriCompleter.isCompleted) {
          uriCompleter.complete(Uri.tryParse(match.group(1)!));
        }
        _log(deviceId, line);
      }

      for (final Stream<List<int>> stream in <Stream<List<int>>>[process.stdout, process.stderr]) {
        stream
            .transform(const Utf8Decoder(allowMalformed: true))
            .transform(const LineSplitter())
            .listen(onLine);
      }
      unawaited(
        process.exitCode.then((int _) {
          if (identical(_runningApps[deviceId], runningApp)) {
            _runningApps.remove(deviceId);
          }
          if (!uriCompleter.isCompleted) {
            uriCompleter.complete();
          }
        }),
      );
    } on ProcessException catch (error) {
      if (!uriCompleter.isCompleted) {
        uriCompleter.complete();
      }
      throw ToolExtensionException('Failed to start "${executable.path}": ${error.message}');
    }
  }

  /// Sends [line] to the host's log reader, or keeps it until one starts.
  void _log(String deviceId, String line) {
    if (_logStreams.contains(deviceId)) {
      emitLogLine(deviceId: deviceId, line: line);
      return;
    }
    _pendingLogLines.add(line);
    if (_pendingLogLines.length > _kMaxPendingLogLines) {
      _pendingLogLines.removeAt(0);
    }
  }

  @override
  Future<void> startLogStream({required String deviceId}) async {
    _logStreams.add(deviceId);
    for (final String line in _pendingLogLines) {
      emitLogLine(deviceId: deviceId, line: line);
    }
    _pendingLogLines.clear();
  }

  @override
  Future<void> stopLogStream({required String deviceId}) async {
    _logStreams.remove(deviceId);
  }

  @override
  Future<Uri?> getVmServiceUri({required String deviceId}) async {
    return _runningApps[deviceId]?.vmServiceCompleter.future;
  }

  @override
  Future<void> stopApp({required String deviceId, String? appId}) async {
    final _RunningApp? runningApp = _runningApps[deviceId];
    if (runningApp == null || (appId != null && runningApp.appId != appId)) {
      return;
    }
    runningApp.process.kill();
    _runningApps.remove(deviceId);
    if (!runningApp.vmServiceCompleter.isCompleted) {
      runningApp.vmServiceCompleter.complete();
    }
    await runningApp.process.exitCode;
  }

  @override
  Future<void> shutdown() async {
    await Future.wait(
      _runningApps.keys.toList().map((String deviceId) => stopApp(deviceId: deviceId)),
    );
  }
}
