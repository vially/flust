// The app the end-to-end tests drive. Every frame it draws says, in pixels a
// script can read back from a screen recording, what the frame is:
//
// - a square of a different colour in each corner, so a frame that is
//   offset, cropped or stretched in its window shows up as a corner that is
//   not where the window's corner is;
// - the size the frame was laid out for, as a row of black and white cells;
// - how many frames have been painted and how many taps were received, as a
//   second row.
//
// Everything is drawn on whole physical pixels, without anti-aliasing. The
// rows and the corners need a window at least 264 pixels wide. The
// layout is described in `tests/e2e/README.md` and read back by
// `tests/e2e/analyzers/probe.py`: change them together.
//
// Environment:
//
// - `PROBE_ANIMATE=1`: paint a new frame on every vsync. Without it a frame
//   is painted only when something changes (the size, a tap).
// - `PROBE_SEMANTICS=1`: show a few controls instead, for the tests that read
//   the app through the accessibility API of the platform.
// - `PROBE_SHAPES=1`: draw shapes instead, for the test of what the renderer
//   draws (`ShapesPainter`).
// - `PROBE_EXIT=1`: cancel every request to exit (a close of the window),
//   and exit with code 3 on a tap, for the test of the exit protocol.
// - `PROBE_WINDOWS=1`: start without a window, and open two, each with a
//   probe of its own, through Flust's test hook for windows
//   (`flust-sctk/src/testing.rs`). A window that is asked to close is
//   destroyed, and the app exits when none is left. The runner starts the
//   app without a window then.
// - `PROBE_WINDOWS=startup`: the usual probe, in the startup window, which
//   also tries to open a window, and prints how that was refused.
import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/scheduler.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

void main() {
  if (Platform.environment['PROBE_WINDOWS'] == '1') {
    runWidget(const WindowsProbe());
    return;
  }
  if (Platform.environment['PROBE_SEMANTICS'] == '1') {
    runApp(const SemanticsProbe());
  } else if (Platform.environment['PROBE_SHAPES'] == '1') {
    runApp(const SizedBox.expand(child: CustomPaint(painter: ShapesPainter())));
  } else {
    runApp(const Probe());
  }
}

/// A heading, a button that counts how often it was activated, the count as
/// a live region, a checkbox and a text field. Activating the button also
/// announces the count. What happens to them is printed, so a test
/// can tell that an action arrived.
class SemanticsProbe extends StatefulWidget {
  const SemanticsProbe({super.key});

  @override
  State<SemanticsProbe> createState() => _SemanticsProbeState();
}

class _SemanticsProbeState extends State<SemanticsProbe>
    with WidgetsBindingObserver {
  int _count = 0;
  bool _checked = false;
  final TextEditingController _text = TextEditingController(text: 'Hello');
  final FocusNode _textFocus = FocusNode();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _text.addListener(() {
      print('probe: semantics text "${_text.text}" selection '
          '${_text.selection.baseOffset},${_text.selection.extentOffset}');
    });
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _text.dispose();
    _textFocus.dispose();
    super.dispose();
  }

  @override
  void didChangeAccessibilityFeatures() {
    final ui.AccessibilityFeatures features =
        WidgetsBinding.instance.platformDispatcher.accessibilityFeatures;
    print('probe: semantics accessible navigation '
        '${features.accessibleNavigation}');
  }

  void _increment() {
    setState(() => _count++);
    print('probe: semantics count $_count');
    SemanticsService.sendAnnouncement(
      View.of(context),
      'Counted to $_count',
      TextDirection.ltr,
    );
  }

  void _toggle() {
    setState(() => _checked = !_checked);
    print('probe: semantics checked $_checked');
  }

  @override
  Widget build(BuildContext context) {
    const TextStyle style = TextStyle(color: oneColor, fontSize: 20);
    return Directionality(
      textDirection: TextDirection.ltr,
      child: ColoredBox(
        color: backgroundColor,
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            spacing: 16,
            children: <Widget>[
              Semantics(
                header: true,
                headingLevel: 1,
                child: const Text('Semantics probe', style: style),
              ),
              Semantics(
                button: true,
                identifier: 'increment',
                hint: 'Adds one to the count',
                child: GestureDetector(
                  onTap: _increment,
                  child: const ColoredBox(
                    color: barColor,
                    child: Padding(
                      padding: EdgeInsets.all(12),
                      child: Text('Increment', style: style),
                    ),
                  ),
                ),
              ),
              Semantics(
                liveRegion: true,
                child: Text('Count: $_count', style: style),
              ),
              Semantics(
                checked: _checked,
                identifier: 'notify',
                child: GestureDetector(
                  onTap: _toggle,
                  child: Text(
                    _checked ? '[x] Notify' : '[ ] Notify',
                    style: style,
                  ),
                ),
              ),
              Semantics(
                label: 'Message',
                identifier: 'message',
                child: EditableText(
                  controller: _text,
                  focusNode: _textFocus,
                  autofocus: true,
                  style: style,
                  cursorColor: oneColor,
                  backgroundCursorColor: barColor,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Side of a corner marker, in physical pixels.
const int cornerSize = 16;

/// Side of a code cell, in physical pixels.
const int cellSize = 8;

/// Where the first code row starts, in physical pixels from the top-left.
const int codeLeft = 24;
const int codeTop = 4;

const int sizeBits = 13;
const int frameBits = 16;
const int tapBits = 8;

const Color backgroundColor = Color(0xFF282830);
const Color topLeftColor = Color(0xFFFF0000);
const Color topRightColor = Color(0xFF0000FF);
const Color bottomLeftColor = Color(0xFFFFFF00);
const Color bottomRightColor = Color(0xFF00FFFF);
const Color syncColor = Color(0xFFFF8000);
const Color oneColor = Color(0xFFFFFFFF);
const Color zeroColor = Color(0xFF000000);
const Color barColor = Color(0xFF5080C0);

class Probe extends StatefulWidget {
  const Probe({super.key, this.log = 'probe'});

  /// What the lines it prints start with.
  final String log;

  @override
  State<Probe> createState() => _ProbeState();
}

class _ProbeState extends State<Probe> with SingleTickerProviderStateMixin {
  final bool _animate = Platform.environment['PROBE_ANIMATE'] == '1';
  final bool _exit = Platform.environment['PROBE_EXIT'] == '1';
  late final ProbeState _state = ProbeState(widget.log);
  Ticker? _ticker;
  Timer? _alive;
  AppLifecycleListener? _lifecycle;

  @override
  void initState() {
    super.initState();
    print('${widget.log}: started animate=$_animate');
    if (_animate) {
      _ticker = createTicker((_) => _state.requestFrame())..start();
    }
    if (_exit) {
      _lifecycle = AppLifecycleListener(
        onExitRequested: () async {
          print('probe: exit requested, cancelling');
          return ui.AppExitResponse.cancel;
        },
      );
    }
    // A timer does not depend on vsync, so this keeps printing while the
    // window is hidden and no frames are produced.
    if (Platform.environment['PROBE_WINDOWS'] == 'startup') {
      _tryToOpenWindow();
    }
    _alive = Timer.periodic(const Duration(milliseconds: 500), (_) {
      print('${widget.log}: alive frames=${_state.frames}');
    });
  }

  /// An app with a startup window can not open others.
  Future<void> _tryToOpenWindow() async {
    try {
      await const MethodChannel('flust/testing/windows', JSONMethodCodec())
          .invokeMethod<int>('createWindow', <String, Object>{'title': 'probe 2'});
      print('${widget.log}: window opened');
    } on PlatformException catch (error) {
      print('${widget.log}: window refused: ${error.code}: ${error.message}');
    }
  }

  @override
  void dispose() {
    _ticker?.dispose();
    _alive?.cancel();
    _lifecycle?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final double devicePixelRatio = MediaQuery.devicePixelRatioOf(context);
    return Listener(
      behavior: HitTestBehavior.opaque,
      onPointerDown: (PointerDownEvent event) {
        _state.tap();
        final Offset at = event.localPosition;
        print('${widget.log}: tap ${_state.taps} at '
            '${at.dx.round()},${at.dy.round()}');
        if (_exit) {
          print('probe: exiting with code 3');
          ServicesBinding.instance.exitApplication(ui.AppExitType.required, 3);
        }
      },
      child: SizedBox.expand(
        child: CustomPaint(
          painter: ProbePainter(_state, devicePixelRatio),
        ),
      ),
    );
  }
}

class ProbeState extends ChangeNotifier {
  ProbeState(this.log);

  /// What the lines it prints start with.
  final String log;

  /// Frames painted so far.
  int frames = 0;
  int taps = 0;
  ui.Size? _lastSize;

  void requestFrame() => notifyListeners();

  void tap() {
    taps++;
    notifyListeners();
  }

  void painted(int width, int height, double devicePixelRatio) {
    frames++;
    final ui.Size size = ui.Size(width.toDouble(), height.toDouble());
    if (size != _lastSize) {
      _lastSize = size;
      print('$log: size ${width}x$height dpr $devicePixelRatio');
    }
  }
}

class ProbePainter extends CustomPainter {
  ProbePainter(this.state, this.devicePixelRatio) : super(repaint: state);

  final ProbeState state;
  final double devicePixelRatio;

  @override
  void paint(Canvas canvas, Size size) {
    // Draw in physical pixels.
    final int width = (size.width * devicePixelRatio).round();
    final int height = (size.height * devicePixelRatio).round();
    state.painted(width, height, devicePixelRatio);
    canvas.scale(1 / devicePixelRatio);

    final Paint paint = Paint()..isAntiAlias = false;
    void fill(int x, int y, int w, int h, Color color) {
      paint.color = color;
      canvas.drawRect(
        Rect.fromLTWH(x.toDouble(), y.toDouble(), w.toDouble(), h.toDouble()),
        paint,
      );
    }

    fill(0, 0, width, height, backgroundColor);

    // Something that visibly moves from one frame to the next, for a person
    // looking at the recording. The scripts read the frame counter instead.
    const int barWidth = 48;
    final int barTravel = width - barWidth;
    if (barTravel > 0) {
      final int phase = (state.frames * 4) % (2 * barTravel);
      final int x = phase < barTravel ? phase : 2 * barTravel - phase;
      fill(x, height ~/ 2 - 12, barWidth, 24, barColor);
    }

    // First row: sync, width, height, sync.
    int x = codeLeft;
    void cell(int row, Color color) {
      fill(x, codeTop + row * (cellSize + 4), cellSize, cellSize, color);
      x += cellSize;
    }

    void bits(int row, int value, int count) {
      for (int bit = count - 1; bit >= 0; bit--) {
        cell(row, (value >> bit) & 1 == 1 ? oneColor : zeroColor);
      }
    }

    cell(0, syncColor);
    bits(0, width, sizeBits);
    bits(0, height, sizeBits);
    cell(0, syncColor);

    // Second row: sync, frame counter, tap counter, sync.
    x = codeLeft;
    cell(1, syncColor);
    bits(1, state.frames, frameBits);
    bits(1, state.taps, tapBits);
    cell(1, syncColor);

    // The same in words, for a person.
    final TextPainter label = TextPainter(
      text: TextSpan(
        text: '${width}x$height  frame ${state.frames}  taps ${state.taps}',
        style: const TextStyle(color: Color(0xFFD0D0D0), fontSize: 14),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    canvas.save();
    canvas.scale(devicePixelRatio);
    label.paint(canvas, Offset(codeLeft / devicePixelRatio, 36));
    canvas.restore();
    label.dispose();

    // The corners last, so nothing covers them.
    fill(0, 0, cornerSize, cornerSize, topLeftColor);
    fill(width - cornerSize, 0, cornerSize, cornerSize, topRightColor);
    fill(0, height - cornerSize, cornerSize, cornerSize, bottomLeftColor);
    fill(width - cornerSize, height - cornerSize, cornerSize, cornerSize,
        bottomRightColor);
  }

  @override
  bool shouldRepaint(ProbePainter oldDelegate) =>
      oldDelegate.devicePixelRatio != devicePixelRatio;
}

/// Shapes that a renderer can get wrong without anything failing: paths that
/// are not convex, clips that are not rectangles, and edges that should be
/// antialiased, once directly and once in a layer (drawn offscreen and then
/// composited). `analyzers/shapes.py` reads them back: the positions and
/// colours there mirror the ones here.
///
/// Everything is in physical pixels, on a white background, from the top
/// left corner of the window.
class ShapesPainter extends CustomPainter {
  const ShapesPainter();

  static const Color white = Color(0xFFFFFFFF);
  static const Color blue = Color(0xFF0000FF);
  static const Color red = Color(0xFFFF0000);
  static const Color green = Color(0xFF008000);
  static const Color purple = Color(0xFF800080);
  static const Color black = Color(0xFF000000);

  /// A five-pointed star, drawn in one stroke, so that it crosses itself.
  static Path star(Rect bounds) {
    final Path path = Path();
    for (int i = 0; i < 5; i++) {
      final double angle = -math.pi / 2 + i * 4 * math.pi / 5;
      final Offset point = bounds.center +
          Offset(math.cos(angle), math.sin(angle)) * bounds.shortestSide / 2;
      i == 0
          ? path.moveTo(point.dx, point.dy)
          : path.lineTo(point.dx, point.dy);
    }
    return path..close();
  }

  /// A 160x160 square, rotated by 30 degrees about the centre of `bounds`.
  static void rotatedSquare(Canvas canvas, Rect bounds, Paint paint) {
    canvas.save();
    canvas.translate(bounds.center.dx, bounds.center.dy);
    canvas.rotate(math.pi / 6);
    canvas.drawRect(
        Rect.fromCenter(center: Offset.zero, width: 160, height: 160), paint);
    canvas.restore();
  }

  @override
  void paint(Canvas canvas, Size size) {
    final double devicePixelRatio =
        WidgetsBinding.instance.platformDispatcher.views.first.devicePixelRatio;
    canvas.scale(1 / devicePixelRatio);
    print('probe: shapes');

    canvas.drawPaint(Paint()..color = white);

    // A star filled with the non-zero rule: all of it, the middle too.
    canvas.drawPath(
        star(const Rect.fromLTWH(20, 20, 120, 120)), Paint()..color = blue);

    // The same with the even-odd rule: the middle is left out.
    canvas.drawPath(
        star(const Rect.fromLTWH(160, 20, 120, 120))
          ..fillType = PathFillType.evenOdd,
        Paint()..color = red);

    // A square clipped to a rounded rectangle: the corners are left out.
    canvas.save();
    canvas.clipRRect(RRect.fromRectAndRadius(
        const Rect.fromLTWH(300, 20, 120, 120), const Radius.circular(40)));
    canvas.drawPaint(Paint()..color = green);
    canvas.restore();

    // A square clipped to a circle, through a path.
    canvas.save();
    canvas.clipPath(Path()..addOval(const Rect.fromLTWH(440, 20, 120, 120)));
    canvas.drawPaint(Paint()..color = purple);
    canvas.restore();

    // Squares with slanted edges, which antialiasing blends into the
    // background: one drawn directly, one in a layer.
    rotatedSquare(
        canvas, const Rect.fromLTWH(20, 180, 240, 240), Paint()..color = black);
    canvas.saveLayer(const Rect.fromLTWH(300, 180, 240, 240),
        Paint()..color = const Color(0xFE000000));
    rotatedSquare(canvas, const Rect.fromLTWH(300, 180, 240, 240),
        Paint()..color = black);
    canvas.restore();
  }

  @override
  bool shouldRepaint(ShapesPainter oldDelegate) => false;
}

/// Opens two windows, `probe 1` and `probe 2`, each a [Probe] that prints
/// with its name, and closes them when they are asked to. Exits when no
/// window is left. Each prints the keys it receives (`probe 1: key A`).
class WindowsProbe extends StatefulWidget {
  const WindowsProbe({super.key});

  @override
  State<WindowsProbe> createState() => _WindowsProbeState();
}

class _WindowsProbeState extends State<WindowsProbe> {
  static const MethodChannel _windows =
      MethodChannel('flust/testing/windows', JSONMethodCodec());

  /// The probe name of each open window, by view id.
  final Map<int, String> _names = <int, String>{};

  @override
  void initState() {
    super.initState();
    _windows.setMethodCallHandler(_handle);
    _open('probe 1', 624, 464);
    _open('probe 2', 400, 300);
  }

  Future<void> _open(String name, int width, int height) async {
    final int? viewId = await _windows.invokeMethod<int>(
      'createWindow',
      <String, Object>{'title': name, 'width': width, 'height': height},
    );
    print('$name: window created, view $viewId');
    setState(() => _names[viewId!] = name);
  }

  Future<Object?> _handle(MethodCall call) async {
    final int viewId = (call.arguments as Map<Object?, Object?>)['viewId']! as int;
    final String name = _names[viewId] ?? 'view $viewId';
    switch (call.method) {
      case 'onWindowCloseRequested':
        print('$name: close requested');
        await _windows.invokeMethod<void>('destroyWindow', <String, Object>{'viewId': viewId});
      case 'onWindowDestroyed':
        print('$name: window destroyed');
        setState(() => _names.remove(viewId));
        if (_names.isEmpty) {
          print('probe: no window left, exiting');
          ServicesBinding.instance.exitApplication(ui.AppExitType.required);
        }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final ui.PlatformDispatcher dispatcher = WidgetsBinding.instance.platformDispatcher;
    return ViewCollection(
      views: <Widget>[
        for (final MapEntry<int, String> window in _names.entries)
          if (dispatcher.view(id: window.key) case final ui.FlutterView view)
            View(
              key: ValueKey<int>(window.key),
              view: view,
              // Keys name no window: they go to the one that has focus.
              child: Focus(
                autofocus: true,
                onKeyEvent: (FocusNode node, KeyEvent event) {
                  if (event is KeyDownEvent) {
                    print('${window.value}: key ${event.logicalKey.keyLabel}');
                  }
                  return KeyEventResult.handled;
                },
                child: Probe(log: window.value),
              ),
            ),
      ],
    );
  }
}
