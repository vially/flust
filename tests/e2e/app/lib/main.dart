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
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

void main() {
  if (Platform.environment['PROBE_SEMANTICS'] == '1') {
    runApp(const SemanticsProbe());
  } else {
    runApp(const Probe());
  }
}

/// A heading, a button that counts how often it was activated, the count as
/// a live region and a checkbox. What happens to them is printed, so a test
/// can tell that an action arrived.
class SemanticsProbe extends StatefulWidget {
  const SemanticsProbe({super.key});

  @override
  State<SemanticsProbe> createState() => _SemanticsProbeState();
}

class _SemanticsProbeState extends State<SemanticsProbe> {
  int _count = 0;
  bool _checked = false;

  void _increment() {
    setState(() => _count++);
    print('probe: semantics count $_count');
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
  const Probe({super.key});

  @override
  State<Probe> createState() => _ProbeState();
}

class _ProbeState extends State<Probe> with SingleTickerProviderStateMixin {
  final bool _animate = Platform.environment['PROBE_ANIMATE'] == '1';
  final ProbeState _state = ProbeState();
  Ticker? _ticker;
  Timer? _alive;

  @override
  void initState() {
    super.initState();
    print('probe: started animate=$_animate');
    if (_animate) {
      _ticker = createTicker((_) => _state.requestFrame())..start();
    }
    // A timer does not depend on vsync, so this keeps printing while the
    // window is hidden and no frames are produced.
    _alive = Timer.periodic(const Duration(milliseconds: 500), (_) {
      print('probe: alive frames=${_state.frames}');
    });
  }

  @override
  void dispose() {
    _ticker?.dispose();
    _alive?.cancel();
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
        print('probe: tap ${_state.taps} at '
            '${at.dx.round()},${at.dy.round()}');
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
      print('probe: size ${width}x$height dpr $devicePixelRatio');
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
