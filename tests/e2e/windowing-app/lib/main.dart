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
// (a dialog prints only its size and whether it is activated, a popup or a
// tooltip `<name>: offset <x>,<y> size <width>x<height>`), and takes keys,
// while it has the keyboard focus:
//
// - `n`: open a new window, `window <n>`, 320x240;
// - `d`: open a modal dialog of the window, `dialog <n>`, 300x200;
// - `e`: open a dialog without a parent, `dialog <n>`, 300x200;
// - `p`: open a popup of the window, `popup <n>`, 160x100, below the
//   rectangle (20, 20, 100, 30) of the window, flipped above it if it does
//   not fit, and slid sideways;
// - `o`: open a tooltip of the window, `tooltip <n>`, 120x40, placed like
//   a popup;
// - `j`: move the rectangle the window's tooltips are placed against 50 to
//   the right;
// - `m`: maximize the window, or restore it (not dialogs);
// - `f`: make the window fullscreen, or restore it (not dialogs);
// - `t`: rename the window to `<title> renamed`;
// - `s`: ask for the size 400x300;
// - `c`: limit the size to 300x250 at least and 500x400 at most;
// - `w`: destroy the window.
//
// A tap in a window prints `<title>: tap`, and a tap in the main window
// opens a new window. A window that is asked to close is destroyed (the
// default of the delegates).

// The windowing API is internal until it is stable.
// ignore_for_file: invalid_use_of_internal_member, implementation_imports

import 'dart:ui' show AppExitType;

import 'package:flust_windowing/flust_windowing.dart';
import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:flutter/src/widgets/_window_positioner.dart';
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

/// Unregisters a dialog that is destroyed, so that it is no longer built.
class ProbeDialogDelegate with DialogWindowControllerDelegate {
  ProbeDialogDelegate(this.onDestroyed);

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
int _dialogCount = 0;
int _popupCount = 0;
int _tooltipCount = 0;

/// The names of popups and tooltips, which have no title.
final Map<BaseWindowController, String> _names =
    <BaseWindowController, String>{};

String _title(BaseWindowController controller) => switch (controller) {
  final WindowController window => window.title,
  final DialogWindowController dialog => dialog.title,
  _ => _names[controller] ?? '?',
};

/// The rectangle popups and tooltips are placed against, by window.
final Map<BaseWindowController, Rect> _anchors = <BaseWindowController, Rect>{};

Rect _anchorOf(BaseWindowController window) =>
    _anchors[window] ?? const Rect.fromLTWH(20, 20, 100, 30);

const WindowPositioner _positioner = WindowPositioner(
  parentAnchor: WindowPositionerAnchor.bottomLeft,
  childAnchor: WindowPositionerAnchor.topLeft,
  constraintAdjustment: WindowPositionerConstraintAdjustment(
    flipY: true,
    slideX: true,
  ),
);

class ProbePopupDelegate with PopupWindowControllerDelegate {
  ProbePopupDelegate(this.onDestroyed);

  final VoidCallback onDestroyed;

  @override
  void onWindowDestroyed() {
    super.onWindowDestroyed();
    onDestroyed();
  }
}

class ProbeTooltipDelegate with TooltipWindowControllerDelegate {
  ProbeTooltipDelegate(this.onDestroyed);

  final VoidCallback onDestroyed;

  @override
  void onWindowDestroyed() {
    super.onWindowDestroyed();
    onDestroyed();
  }
}

/// Opens a popup of `parent`.
void openPopup(BuildContext context, BaseWindowController parent) {
  final WindowRegistry registry = WindowRegistry.of(context);
  _popupCount += 1;
  final name = 'popup $_popupCount';
  late final PopupWindowController controller;
  late final WindowEntry entry;
  controller = PopupWindowController(
    parent: parent,
    anchorRect: _anchorOf(parent),
    positioner: _positioner,
    delegate: ProbePopupDelegate(() {
      print('$name: destroyed');
      registry.unregister(entry);
    }),
  );
  _names[controller] = name;
  entry = WindowEntry(
    controller: controller,
    builder: (BuildContext context) => ProbeWindow(
      controller: controller,
      color: const Color(0xff8040c0),
      size: const Size(160, 100),
    ),
  );
  registry.register(entry);
}

/// Opens a tooltip of `parent`.
void openTooltip(BuildContext context, BaseWindowController parent) {
  final WindowRegistry registry = WindowRegistry.of(context);
  _tooltipCount += 1;
  final name = 'tooltip $_tooltipCount';
  late final TooltipWindowController controller;
  late final WindowEntry entry;
  controller = TooltipWindowController(
    parent: parent,
    anchorRect: _anchorOf(parent),
    positioner: _positioner,
    delegate: ProbeTooltipDelegate(() {
      print('$name: destroyed');
      registry.unregister(entry);
    }),
  );
  _names[controller] = name;
  _tooltips.add(controller);
  entry = WindowEntry(
    controller: controller,
    builder: (BuildContext context) => ProbeWindow(
      controller: controller,
      color: const Color(0xff40c0c0),
      size: const Size(120, 40),
    ),
  );
  registry.register(entry);
}

final List<TooltipWindowController> _tooltips = <TooltipWindowController>[];

/// Moves the rectangle the tooltips of `window` are placed against.
void moveTooltips(BaseWindowController window) {
  final Rect anchor = _anchorOf(window).translate(50, 0);
  _anchors[window] = anchor;
  for (final TooltipWindowController tooltip in _tooltips) {
    if (tooltip.parent == window && !tooltip.isDestroyed) {
      tooltip.updatePosition(anchorRect: anchor);
    }
  }
}

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

/// Opens a new dialog, modal to `parent` if there is one.
void openDialog(BuildContext context, BaseWindowController? parent) {
  final WindowRegistry registry = WindowRegistry.of(context);
  _dialogCount += 1;
  late final DialogWindowController controller;
  late final WindowEntry entry;
  controller = DialogWindowController(
    size: const Size(300, 200),
    title: 'dialog $_dialogCount',
    parent: parent,
    delegate: ProbeDialogDelegate(() {
      print('${controller.title}: destroyed');
      registry.unregister(entry);
    }),
  );
  entry = WindowEntry(
    controller: controller,
    builder: (BuildContext context) =>
        ProbeWindow(controller: controller, color: const Color(0xffe0c020)),
  );
  registry.register(entry);
}

class ProbeWindow extends StatefulWidget {
  const ProbeWindow({
    super.key,
    required this.controller,
    required this.color,
    this.size,
  });

  final BaseWindowController controller;
  final Color color;

  /// The size of the content, for a window sized to it.
  final Size? size;

  @override
  State<ProbeWindow> createState() => _ProbeWindowState();
}

class _ProbeWindowState extends State<ProbeWindow> {
  String? _printed;

  BaseWindowController get controller => widget.controller;

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
    final sizeText = '${size.width.round()}x${size.height.round()}';
    final String state = switch (controller) {
      final WindowController window =>
        '${window.title}: size $sizeText activated ${window.isActivated} '
            'maximized ${window.isMaximized} fullscreen ${window.isFullscreen}',
      final DialogWindowController dialog =>
        '${dialog.title}: size $sizeText activated ${dialog.isActivated}',
      final PopupWindowController popup =>
        '${_title(popup)}: offset ${popup.offsetFromParent.dx.round()},'
            '${popup.offsetFromParent.dy.round()} size $sizeText',
      _ => '${_title(controller)}: size $sizeText',
    };
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
    final BaseWindowController controller = this.controller;
    const constraints = BoxConstraints(
      minWidth: 300,
      minHeight: 250,
      maxWidth: 500,
      maxHeight: 400,
    );
    if (key == LogicalKeyboardKey.keyN) {
      openWindow(context);
    } else if (key == LogicalKeyboardKey.keyD) {
      openDialog(context, controller);
    } else if (key == LogicalKeyboardKey.keyE) {
      openDialog(context, null);
    } else if (key == LogicalKeyboardKey.keyP) {
      openPopup(context, controller);
    } else if (key == LogicalKeyboardKey.keyO) {
      openTooltip(context, controller);
    } else if (key == LogicalKeyboardKey.keyJ) {
      moveTooltips(controller);
    } else if (key == LogicalKeyboardKey.keyM &&
        controller is WindowController) {
      controller.setMaximized(!controller.isMaximized);
    } else if (key == LogicalKeyboardKey.keyF &&
        controller is WindowController) {
      controller.setFullscreen(!controller.isFullscreen);
    } else if (key == LogicalKeyboardKey.keyT) {
      switch (controller) {
        case final WindowController window:
          window.setTitle('${window.title} renamed');
        case final DialogWindowController dialog:
          dialog.setTitle('${dialog.title} renamed');
        default:
      }
    } else if (key == LogicalKeyboardKey.keyS) {
      switch (controller) {
        case final WindowController window:
          window.setSize(const Size(400, 300));
        case final DialogWindowController dialog:
          dialog.setSize(const Size(400, 300));
        default:
      }
    } else if (key == LogicalKeyboardKey.keyC) {
      switch (controller) {
        case final WindowController window:
          window.setConstraints(constraints);
        case final DialogWindowController dialog:
          dialog.setConstraints(constraints);
        default:
      }
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
      // A tooltip takes no input, and no focus.
      autofocus: controller is! TooltipWindowController,
      onKeyEvent: _onKey,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () {
          print('${_title(controller)}: tap');
          if (_title(controller) == 'main') {
            openWindow(context);
          }
        },
        child: ColoredBox(
          color: widget.color,
          child: switch (widget.size) {
            final Size size => SizedBox.fromSize(size: size),
            null => const SizedBox.expand(),
          },
        ),
      ),
    );
  }
}
