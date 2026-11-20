/// Flutter's windowing API on Flust.
///
/// Flutter's windowing API (`WindowController`, the `Window` widget,
/// `WindowManager`, in `package:flutter/src/widgets/_window.dart`) creates
/// windows through a `WindowingOwner`. Flutter's own one for Linux calls GTK,
/// which a Flust app does not have: this package installs one that calls
/// Flust instead.
///
/// The API is experimental: it only exists on Flutter's master channel, with
/// the windowing feature on (`flutter config --enable-windowing`, or
/// `FLUTTER_WINDOWING=true` in the environment of the build), and changes
/// without notice.
///
/// ```dart
/// Future<void> main() async {
///   await FlustWindowing.ensureInitialized();
///   runWidget(...);
/// }
/// ```
///
/// Windows can only be created by an app that Flust starts without a window
/// of its own: `BackendConfigSctk::with_startup_window(false)`.
library;

// The windowing API is internal until it is stable.
// ignore_for_file: invalid_use_of_internal_member, implementation_imports

import 'dart:ffi' as ffi;
import 'dart:ui' show Display, FlutterView;

import 'package:ffi/ffi.dart';
import 'package:flutter/services.dart';
import 'package:flutter/src/widgets/_window.dart';
import 'package:flutter/src/widgets/_window_positioner.dart';
import 'package:flutter/widgets.dart';

/// Installs Flust's [WindowingOwner].
abstract final class FlustWindowing {
  static WindowingOwnerFlust? _owner;

  /// Initializes the widgets binding, if it is not yet, and makes Flust
  /// create the windows of Flutter's windowing API. Call it, and wait for it,
  /// before any window is created.
  ///
  /// Throws an [UnsupportedError] if the app does not run on Flust, or if
  /// Flutter's windowing feature is not on.
  static Future<void> ensureInitialized() async {
    final WidgetsBinding binding = WidgetsFlutterBinding.ensureInitialized();
    if (_owner != null) {
      return;
    }

    const channel = MethodChannel('flust/windowing');
    final Map<String, int>? functions;
    try {
      functions = await channel.invokeMapMethod<String, int>('initialize');
    } on MissingPluginException {
      throw UnsupportedError(
        'flust_windowing only works in an app that runs on Flust.',
      );
    }
    final int? engineId = binding.platformDispatcher.engineId;
    if (functions == null || engineId == null) {
      throw StateError('Flust did not initialize its windowing.');
    }

    final owner = WindowingOwnerFlust._(engineId, _Native(functions), channel);
    binding.windowingOwner = owner;
    _owner = owner;
  }
}

/// Creates the windows of Flutter's windowing API through Flust. Installed
/// by [FlustWindowing.ensureInitialized].
///
/// Only regular windows and dialogs are supported so far.
class WindowingOwnerFlust extends WindowingOwner {
  WindowingOwnerFlust._(this._engineId, this._native, MethodChannel channel) {
    channel.setMethodCallHandler(_handleMethodCall);
  }

  final int _engineId;
  final _Native _native;
  final Map<int, _FlustWindow> _windows = <int, _FlustWindow>{};

  Future<void> _handleMethodCall(MethodCall call) async {
    final int viewId =
        (call.arguments as Map<Object?, Object?>)['viewId']! as int;
    final _FlustWindow? window = _windows[viewId];
    if (window == null) {
      return;
    }
    switch (call.method) {
      case 'onWindowChanged':
        window._changed();
      case 'onWindowCloseRequested':
        window._closeRequested();
      case 'onWindowDestroyed':
        window._destroyed();
    }
  }

  @override
  WindowController createWindowController({
    required WindowControllerDelegate delegate,
    Size? size,
    BoxConstraints? constraints,
    required bool resizable,
    String? title,
  }) {
    return WindowControllerFlust._(
      owner: this,
      delegate: delegate,
      size: size,
      constraints: constraints,
      title: title,
    );
  }

  @override
  DialogWindowController createDialogWindowController({
    required DialogWindowControllerDelegate delegate,
    Size? size,
    BoxConstraints? constraints,
    required bool resizable,
    BaseWindowController? parent,
    String? title,
  }) {
    return DialogWindowControllerFlust._(
      owner: this,
      delegate: delegate,
      size: size,
      constraints: constraints,
      title: title,
      parent: parent,
    );
  }

  @override
  TooltipWindowController createTooltipWindowController({
    required TooltipWindowControllerDelegate delegate,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
    required BaseWindowController parent,
  }) {
    throw UnimplementedError('Tooltip windows are not supported by Flust yet.');
  }

  @override
  PopupWindowController createPopupWindowController({
    required PopupWindowControllerDelegate delegate,
    required BoxConstraints constraints,
    required Rect anchorRect,
    required WindowPositioner positioner,
    required BaseWindowController parent,
  }) {
    throw UnimplementedError('Popup windows are not supported by Flust yet.');
  }

  @override
  SatelliteWindowController createSatelliteWindowController({
    required SatelliteWindowControllerDelegate delegate,
    required BaseWindowController parent,
    required WindowPositioner initialPositioner,
    Rect? initialAnchorRect,
    Size? size,
    BoxConstraints? constraints,
    required bool resizable,
    String? title,
  }) {
    throw UnimplementedError(
      'Satellite windows are not supported by Flust yet.',
    );
  }
}

/// What regular windows and dialogs created by Flust have in common.
///
/// Wayland limits what a window can do on its own:
///
/// * `activate` does nothing: Wayland only lets a window be activated with a
///   token from the window that has the focus, which Flust does not ask for
///   yet.
/// * `isMinimized` is always false: compositors do not say whether a window
///   is minimized, and a minimized window can not be brought back by the
///   app.
/// * `setSize` is a request: the compositor decides, and does not change the
///   size of a maximized, fullscreen or tiled window.
///
/// A window created with a `shrinkWrap` constructor is not sized to its
/// content yet: it gets the app's default size, as with Flutter's own Linux
/// embedder.
///
/// Mixed into subclasses of `BaseWindowController`, which is sealed, so it
/// can not be named here.
mixin _FlustWindow on ChangeNotifier {
  late final WindowingOwnerFlust _owner;
  String _title = '';
  bool _isDestroyed = false;

  _Native get _native => _owner._native;
  int get _engineId => _owner._engineId;
  int get _viewId => rootView.viewId;

  // From `BaseWindowController`.
  FlutterView get rootView;
  set rootView(FlutterView view);

  /// Creates the window, and its view, which the engine has when this
  /// returns. A window with a `modalParent` is a modal dialog of it.
  void _create(
    WindowingOwnerFlust owner, {
    Size? size,
    BoxConstraints? constraints,
    String? title,
    BaseWindowController? modalParent,
  }) {
    _owner = owner;
    _title = title ?? '';
    final int viewId = _withString(
      title,
      (ffi.Pointer<Utf8> title) => _native.createWindow(
        _engineId,
        size?.width ?? double.nan,
        size?.height ?? double.nan,
        constraints?.minWidth ?? 0,
        constraints?.minHeight ?? 0,
        constraints?.maxWidth ?? double.infinity,
        constraints?.maxHeight ?? double.infinity,
        title,
        modalParent?.rootView.viewId ?? -1,
      ),
    );
    if (viewId < 0) {
      throw StateError(_createWindowFailure(viewId));
    }

    final FlutterView? view = WidgetsBinding.instance.platformDispatcher.view(
      id: viewId,
    );
    if (view == null) {
      throw StateError(
        'Flust created window $viewId, but the engine has no view for it.',
      );
    }
    rootView = view;
    owner._windows[viewId] = this;
  }

  Size get contentSize {
    if (_isDestroyed) {
      return Size.zero;
    }
    return Size(
      _native.getContentWidth(_engineId, _viewId),
      _native.getContentHeight(_engineId, _viewId),
    );
  }

  int get _state => _isDestroyed ? 0 : _native.getState(_engineId, _viewId);

  String get title => _title;

  bool get isActivated => _state & _Native.stateActivated != 0;

  bool get isMinimized => false;

  bool get isDestroyed => _isDestroyed;

  void setSize(Size size) {
    _checkNotDestroyed();
    _native.setSize(_engineId, _viewId, size.width, size.height);
  }

  void setConstraints(BoxConstraints constraints) {
    _checkNotDestroyed();
    _native.setConstraints(
      _engineId,
      _viewId,
      constraints.minWidth,
      constraints.minHeight,
      constraints.maxWidth,
      constraints.maxHeight,
    );
  }

  void setTitle(String title) {
    _checkNotDestroyed();
    _title = title;
    _withString(
      title,
      (ffi.Pointer<Utf8> title) => _native.setTitle(_engineId, _viewId, title),
    );
    notifyListeners();
  }

  void activate() {
    _checkNotDestroyed();
    _native.activate(_engineId, _viewId);
  }

  void setMinimized(bool minimized) {
    _checkNotDestroyed();
    _native.setMinimized(_engineId, _viewId, minimized);
  }

  /// Destroys the window, and its dialogs first. Its view is gone when this
  /// returns, and the delegate has been told, as with Flutter's own
  /// embedders.
  void destroy() {
    if (_isDestroyed) {
      return;
    }
    // Other platforms destroy a window's dialogs with it. Wayland does not.
    final List<DialogWindowControllerFlust> dialogs = _owner._windows.values
        .whereType<DialogWindowControllerFlust>()
        .where((DialogWindowControllerFlust dialog) => dialog.parent == this)
        .toList();
    for (final dialog in dialogs) {
      dialog.destroy();
    }
    _native.destroyWindow(_engineId, _viewId);
    _destroyed();
  }

  void _changed() {
    if (!_isDestroyed) {
      notifyListeners();
    }
  }

  void _closeRequested() {
    if (!_isDestroyed) {
      _onCloseRequested();
    }
  }

  void _destroyed() {
    if (_isDestroyed) {
      return;
    }
    _isDestroyed = true;
    _owner._windows.remove(_viewId);
    _onDestroyed();
    notifyListeners();
  }

  /// Tells the delegate that the compositor asked for the window to close.
  void _onCloseRequested();

  /// Tells the delegate that the window is destroyed.
  void _onDestroyed();

  void _checkNotDestroyed() {
    if (_isDestroyed) {
      throw StateError('Window has been destroyed.');
    }
  }
}

/// A regular window, created by Flust. See [_FlustWindow] for what Wayland
/// limits.
class WindowControllerFlust extends WindowController with _FlustWindow {
  WindowControllerFlust._({
    required WindowingOwnerFlust owner,
    required WindowControllerDelegate delegate,
    Size? size,
    BoxConstraints? constraints,
    String? title,
  }) : _delegate = delegate,
       super.empty() {
    _create(owner, size: size, constraints: constraints, title: title);
  }

  final WindowControllerDelegate _delegate;

  @override
  bool get isMaximized => _state & _Native.stateMaximized != 0;

  @override
  bool get isFullscreen => _state & _Native.stateFullscreen != 0;

  @override
  void setMaximized(bool maximized) {
    _checkNotDestroyed();
    _native.setMaximized(_engineId, _viewId, maximized);
  }

  @override
  void setFullscreen(bool fullscreen, {Display? display}) {
    _checkNotDestroyed();
    _native.setFullscreen(_engineId, _viewId, fullscreen);
  }

  @override
  void _onCloseRequested() => _delegate.onWindowCloseRequested(this);

  @override
  void _onDestroyed() => _delegate.onWindowDestroyed();
}

/// A dialog window, created by Flust. See [_FlustWindow] for what Wayland
/// limits.
///
/// A dialog with a parent is modal: the compositor keeps it above its
/// parent, and the parent takes no input until the dialog is destroyed. It
/// is destroyed with its parent. A dialog without a parent is a regular
/// window on Wayland.
class DialogWindowControllerFlust extends DialogWindowController
    with _FlustWindow {
  DialogWindowControllerFlust._({
    required WindowingOwnerFlust owner,
    required DialogWindowControllerDelegate delegate,
    Size? size,
    BoxConstraints? constraints,
    String? title,
    BaseWindowController? parent,
  }) : _delegate = delegate,
       _parent = parent,
       super.empty() {
    if (parent != null && (parent is! _FlustWindow || parent.isDestroyed)) {
      throw ArgumentError.value(
        parent,
        'parent',
        'Not a window of Flust that is still open',
      );
    }
    _create(
      owner,
      size: size,
      constraints: constraints,
      title: title,
      modalParent: parent,
    );
  }

  final DialogWindowControllerDelegate _delegate;
  final BaseWindowController? _parent;

  @override
  BaseWindowController? get parent => _parent;

  @override
  void _onCloseRequested() => _delegate.onWindowCloseRequested(this);

  @override
  void _onDestroyed() => _delegate.onWindowDestroyed();
}

String _createWindowFailure(int failure) {
  return switch (failure) {
    -1 =>
      'Flust only creates windows for an app that it starts without a window '
          'of its own. Start the app with `BackendConfigSctk::with_startup_window(false)`.',
    -2 =>
      'Flust only creates windows for an app that renders into a framebuffer: '
          'start the app with `FlutterOpenGLTargetType::Framebuffer`.',
    -3 => 'Flust only creates windows while the engine runs.',
    -5 => 'Flust has no open window to be the parent of the window.',
    _ => 'Flust failed to create a window: its log says why.',
  };
}

/// Calls `f` with `string` as a C string, or with a null pointer.
T _withString<T>(String? string, T Function(ffi.Pointer<Utf8>) f) {
  final ffi.Pointer<Utf8> pointer = string?.toNativeUtf8() ?? ffi.nullptr;
  try {
    return f(pointer);
  } finally {
    if (pointer != ffi.nullptr) {
      malloc.free(pointer);
    }
  }
}

/// The functions of Flust that Dart calls, from their addresses. Their
/// signatures are those of `flust-sctk/src/windowing.rs`. Every function
/// takes the id of the engine first, and all but `createWindow` the id of
/// the window's view next. `createWindow` takes the view id of the window
/// that the new one is a modal dialog of last, or -1.
class _Native {
  _Native(Map<String, int> functions)
    : createWindow =
          ffi.Pointer<ffi.NativeFunction<_CreateWindowNative>>.fromAddress(
            _address(functions, 'createWindow'),
          ).asFunction<_CreateWindow>(),
      destroyWindow = ffi.Pointer<ffi.NativeFunction<_ViewNative>>.fromAddress(
        _address(functions, 'destroyWindow'),
      ).asFunction<_View>(),
      getContentWidth =
          ffi.Pointer<ffi.NativeFunction<_GetDoubleNative>>.fromAddress(
            _address(functions, 'getContentWidth'),
          ).asFunction<_GetDouble>(),
      getContentHeight =
          ffi.Pointer<ffi.NativeFunction<_GetDoubleNative>>.fromAddress(
            _address(functions, 'getContentHeight'),
          ).asFunction<_GetDouble>(),
      getState = ffi.Pointer<ffi.NativeFunction<_GetStateNative>>.fromAddress(
        _address(functions, 'getState'),
      ).asFunction<_GetState>(),
      setTitle = ffi.Pointer<ffi.NativeFunction<_SetStringNative>>.fromAddress(
        _address(functions, 'setTitle'),
      ).asFunction<_SetString>(),
      setSize = ffi.Pointer<ffi.NativeFunction<_SetSizeNative>>.fromAddress(
        _address(functions, 'setSize'),
      ).asFunction<_SetSize>(),
      setConstraints =
          ffi.Pointer<ffi.NativeFunction<_SetConstraintsNative>>.fromAddress(
            _address(functions, 'setConstraints'),
          ).asFunction<_SetConstraints>(),
      activate = ffi.Pointer<ffi.NativeFunction<_ViewNative>>.fromAddress(
        _address(functions, 'activate'),
      ).asFunction<_View>(),
      setMaximized =
          ffi.Pointer<ffi.NativeFunction<_SetBoolNative>>.fromAddress(
            _address(functions, 'setMaximized'),
          ).asFunction<_SetBool>(),
      setMinimized =
          ffi.Pointer<ffi.NativeFunction<_SetBoolNative>>.fromAddress(
            _address(functions, 'setMinimized'),
          ).asFunction<_SetBool>(),
      setFullscreen =
          ffi.Pointer<ffi.NativeFunction<_SetBoolNative>>.fromAddress(
            _address(functions, 'setFullscreen'),
          ).asFunction<_SetBool>();

  static const int stateActivated = 1 << 0;
  static const int stateMaximized = 1 << 1;
  static const int stateFullscreen = 1 << 2;

  final _CreateWindow createWindow;
  final _View destroyWindow;
  final _GetDouble getContentWidth;
  final _GetDouble getContentHeight;
  final _GetState getState;
  final _SetString setTitle;
  final _SetSize setSize;
  final _SetConstraints setConstraints;
  final _View activate;
  final _SetBool setMaximized;
  final _SetBool setMinimized;
  final _SetBool setFullscreen;

  static int _address(Map<String, int> functions, String name) {
    final int? address = functions[name];
    if (address == null) {
      throw StateError('Flust has no windowing function `$name`.');
    }
    return address;
  }
}

typedef _CreateWindowNative = ffi.Int64 Function(
  ffi.Int64 engineId,
  ffi.Double width,
  ffi.Double height,
  ffi.Double minWidth,
  ffi.Double minHeight,
  ffi.Double maxWidth,
  ffi.Double maxHeight,
  ffi.Pointer<Utf8> title,
  ffi.Int64 modalParent,
);
typedef _CreateWindow = int Function(
  int engineId,
  double width,
  double height,
  double minWidth,
  double minHeight,
  double maxWidth,
  double maxHeight,
  ffi.Pointer<Utf8> title,
  int modalParent,
);
typedef _ViewNative = ffi.Void Function(ffi.Int64 engineId, ffi.Int64 viewId);
typedef _View = void Function(int engineId, int viewId);
typedef _GetDoubleNative = ffi.Double Function(
  ffi.Int64 engineId,
  ffi.Int64 viewId,
);
typedef _GetDouble = double Function(int engineId, int viewId);
typedef _GetStateNative = ffi.Uint32 Function(
  ffi.Int64 engineId,
  ffi.Int64 viewId,
);
typedef _GetState = int Function(int engineId, int viewId);
typedef _SetStringNative = ffi.Void Function(
  ffi.Int64 engineId,
  ffi.Int64 viewId,
  ffi.Pointer<Utf8> value,
);
typedef _SetString = void Function(
  int engineId,
  int viewId,
  ffi.Pointer<Utf8> value,
);
typedef _SetSizeNative = ffi.Void Function(
  ffi.Int64 engineId,
  ffi.Int64 viewId,
  ffi.Double width,
  ffi.Double height,
);
typedef _SetSize = void Function(
  int engineId,
  int viewId,
  double width,
  double height,
);
typedef _SetConstraintsNative = ffi.Void Function(
  ffi.Int64 engineId,
  ffi.Int64 viewId,
  ffi.Double minWidth,
  ffi.Double minHeight,
  ffi.Double maxWidth,
  ffi.Double maxHeight,
);
typedef _SetConstraints = void Function(
  int engineId,
  int viewId,
  double minWidth,
  double minHeight,
  double maxWidth,
  double maxHeight,
);
typedef _SetBoolNative = ffi.Void Function(
  ffi.Int64 engineId,
  ffi.Int64 viewId,
  ffi.Bool value,
);
typedef _SetBool = void Function(int engineId, int viewId, bool value);
