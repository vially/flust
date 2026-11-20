// The app with windows of its own that the end-to-end tests drive, through
// Flutter's windowing API, as upstream's `examples/multiple_windows` does:
// `runWidget` with a `WindowManager`, and a main window whose delegate exits
// the app when it is destroyed. The only addition is the installation of
// Flust's windowing (`FlustWindowing.ensureInitialized`).
//
// Every window prints what it is whenever that changes:
//
//     <title>: size <width>x<height> activated <bool> maximized <bool> fullscreen <bool>
//
// and takes keys, while it has the keyboard focus:
//
// - `n`: open a new window, `window <n>`, 320x240;
// - `m`: maximize the window, or restore it;
// - `f`: make the window fullscreen, or restore it;
// - `t`: rename the window to `<title> renamed`;
// - `s`: ask for the size 400x300;
// - `c`: limit the size to 300x250 at least and 500x400 at most;
// - `w`: destroy the window.
//
// A tap in the main window also opens a new window. A window that is asked
// to close is destroyed (the default of the delegate).

// The windowing API is internal until it is stable.
// ignore_for_file: invalid_use_of_internal_member, implementation_imports

import 'dart:ui' show AppExitType;

import 'package:flust_windowing/flust_windowing.dart';
import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:flutter/widgets.dart';

class MainWindowDelegate with WindowControllerDelegate {
  @override
  void onWindowDestroyed() {
    super.onWindowDestroyed();
    print('main: destroyed, exiting');
    ServicesBinding.instance.exitApplication(AppExitType.required);
  }
}

/// Unregisters a window that is destroyed, so that it is no longer built.
class ProbeWindowDelegate with WindowControllerDelegate {
  ProbeWindowDelegate(this.onDestroyed);

  final VoidCallback onDestroyed;

  @override
  void onWindowDestroyed() {
    super.onWindowDestroyed();
    onDestroyed();
  }
}

Future<void> main() async {
  await FlustWindowing.ensureInitialized();
  runWidget(const WindowingProbe());
}

class WindowingProbe extends StatefulWidget {
  const WindowingProbe({super.key});

  @override
  State<WindowingProbe> createState() => _WindowingProbeState();
}

class _WindowingProbeState extends State<WindowingProbe> {
  final WindowController controller = WindowController(
    size: const Size(480, 360),
    title: 'main',
    delegate: MainWindowDelegate(),
  );

  @override
  void dispose() {
    controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return WindowManager(
      initialWindows: <WindowEntry>[
        WindowEntry(
          controller: controller,
          builder: (BuildContext context) => ProbeWindow(
            controller: controller,
            color: const Color(0xff2060c0),
          ),
        ),
      ],
    );
  }
}

int _windowCount = 0;

/// Opens a new window, registered with the `WindowManager` above `context`.
void openWindow(BuildContext context) {
  final WindowRegistry registry = WindowRegistry.of(context);
  _windowCount += 1;
  late final WindowController controller;
  late final WindowEntry entry;
  controller = WindowController(
    size: const Size(320, 240),
    title: 'window $_windowCount',
    delegate: ProbeWindowDelegate(() {
      print('${controller.title}: destroyed');
      registry.unregister(entry);
    }),
  );
  final Color color = _windowCount.isOdd
      ? const Color(0xffc04020)
      : const Color(0xff20a040);
  entry = WindowEntry(
    controller: controller,
    builder: (BuildContext context) =>
        ProbeWindow(controller: controller, color: color),
  );
  registry.register(entry);
}

class ProbeWindow extends StatefulWidget {
  const ProbeWindow({super.key, required this.controller, required this.color});

  final WindowController controller;
  final Color color;

  @override
  State<ProbeWindow> createState() => _ProbeWindowState();
}

class _ProbeWindowState extends State<ProbeWindow> {
  String? _printed;

  WindowController get controller => widget.controller;

  @override
  void initState() {
    super.initState();
    controller.addListener(_printState);
    _printState();
  }

  @override
  void dispose() {
    controller.removeListener(_printState);
    super.dispose();
  }

  void _printState() {
    if (controller.isDestroyed) {
      return;
    }
    final Size size = controller.contentSize;
    final state =
        '${controller.title}: size ${size.width.round()}x${size.height.round()} '
        'activated ${controller.isActivated} maximized ${controller.isMaximized} '
        'fullscreen ${controller.isFullscreen}';
    if (state != _printed) {
      _printed = state;
      print(state);
    }
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) {
      return KeyEventResult.ignored;
    }
    final LogicalKeyboardKey key = event.logicalKey;
    if (key == LogicalKeyboardKey.keyN) {
      openWindow(context);
    } else if (key == LogicalKeyboardKey.keyM) {
      controller.setMaximized(!controller.isMaximized);
    } else if (key == LogicalKeyboardKey.keyF) {
      controller.setFullscreen(!controller.isFullscreen);
    } else if (key == LogicalKeyboardKey.keyT) {
      controller.setTitle('${controller.title} renamed');
    } else if (key == LogicalKeyboardKey.keyS) {
      controller.setSize(const Size(400, 300));
    } else if (key == LogicalKeyboardKey.keyC) {
      controller.setConstraints(
        const BoxConstraints(
          minWidth: 300,
          minHeight: 250,
          maxWidth: 500,
          maxHeight: 400,
        ),
      );
    } else if (key == LogicalKeyboardKey.keyW) {
      controller.destroy();
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      autofocus: true,
      onKeyEvent: _onKey,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () {
          print('${controller.title}: tap');
          if (controller.title == 'main') {
            openWindow(context);
          }
        },
        child: ColoredBox(color: widget.color, child: const SizedBox.expand()),
      ),
    );
  }
}
