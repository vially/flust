import 'dart:io';

import 'package:flutter_tools_core/flutter_tools_core.dart';
import 'package:flutter_tools_extension/flutter_tools_extension.dart';

/// `flutter doctor` section for Flust: the Rust toolchain and a Wayland session.
final class FlustDiagnostics extends DiagnosticsExtension {
  @override
  String get title => 'Flust (prototype)';

  @override
  Future<List<ValidationResult>> runDiagnostics() async {
    final messages = <ValidationMessage>[];
    var type = ValidationType.success;

    for (final tool in <String>['cargo', 'rustc']) {
      try {
        final ProcessResult result = await Process.run(tool, <String>['--version']);
        if (result.exitCode == 0) {
          messages.add(ValidationMessage((result.stdout as String).trim()));
          continue;
        }
      } on ProcessException {
        // Reported below.
      }
      type = ValidationType.missing;
      messages.add(ValidationMessage.error('$tool not found; install Rust with rustup.'));
    }

    final String? wayland = Platform.environment['WAYLAND_DISPLAY'];
    if (wayland == null || wayland.isEmpty) {
      if (type == ValidationType.success) {
        type = ValidationType.partial;
      }
      messages.add(
        const ValidationMessage.hint('WAYLAND_DISPLAY is not set; Flust apps need a Wayland session.'),
      );
    } else {
      messages.add(ValidationMessage('Wayland display: $wayland'));
    }

    return <ValidationResult>[ValidationResult(type, messages)];
  }
}
