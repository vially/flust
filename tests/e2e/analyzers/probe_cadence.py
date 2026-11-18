#!/usr/bin/env python3
"""Check that the probe app keeps delivering frames, in order and on time.

The probe app numbers the frames it paints. Reading that number back from
every frame the compositor displayed while the scenario's steps ran shows:

- **dropped** frames: numbers that were never displayed. The app painted
  them and the embedder (or the engine, on its behalf) threw them away.
- **stalls**: stretches during which the picture did not advance. Measured
  between two displayed frames with different numbers.
- frames displayed **out of order**: a number lower than one shown before.

For scenarios that run the app with `PROBE_ANIMATE=1`, where a frame is
painted on every vsync and so a new one is due on every output refresh.

Run by waytape as an analyzer, or by hand with a run directory. Writes
nothing; exits with 1 when a limit is exceeded.
"""

import argparse
import sys

import probe


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    probe.run_dir_argument(parser)
    parser.add_argument(
        "--max-stall-ms",
        type=float,
        default=100,
        help="the longest the picture may stand still (default: 100)",
    )
    parser.add_argument(
        "--max-dropped",
        type=float,
        default=5,
        help="how many painted frames may go undisplayed, in percent (default: 5)",
    )
    args = parser.parse_args()
    if args.run_dir is None:
        parser.error("give a run directory, or run as a waytape analyzer")

    frames = [frame for frame in probe.load_run(args.run_dir, args.scale) if frame.counter is not None]
    if len(frames) < 2:
        print("probe_cadence: fewer than two readable frames while the steps ran", file=sys.stderr)
        return 1

    failures = []
    print("section: displayed, dropped, longest stall")
    total_shown = total_dropped = 0
    worst_stall = 0.0
    out_of_order = 0
    for name, group in probe.sections(frames).items():
        shown = dropped = 0
        stall = 0.0
        last = group[0]
        for frame in group[1:]:
            if frame.counter == last.counter:
                continue  # the same picture again, e.g. only the border changed
            if frame.counter < last.counter:
                out_of_order += 1
                print(
                    f"frame {frame.index} t={frame.time:.3f} shows app frame {frame.counter} "
                    f"after {last.counter}"
                )
            else:
                dropped += frame.counter - last.counter - 1
            shown += 1
            stall = max(stall, frame.time - last.time)
            last = frame
        span = group[-1].time - group[0].time
        rate = f"{shown / span:.0f}/s" if span > 0 else "-"
        print(f"  {name}: {shown} ({rate}), {dropped}, {stall * 1000:.0f} ms")
        total_shown += shown
        total_dropped += dropped
        worst_stall = max(worst_stall, stall)

    painted = total_shown + total_dropped
    dropped_percent = 100 * total_dropped / painted if painted else 0.0
    if out_of_order:
        failures.append(f"{out_of_order} frames displayed out of order")
    if worst_stall * 1000 > args.max_stall_ms:
        failures.append(f"stalled for {worst_stall * 1000:.0f} ms (limit {args.max_stall_ms:.0f})")
    if dropped_percent > args.max_dropped:
        failures.append(f"dropped {dropped_percent:.0f}% of frames (limit {args.max_dropped:.0f}%)")

    print(
        f"app frames displayed: {total_shown}, dropped: {total_dropped} ({dropped_percent:.0f}%), "
        f"longest stall: {worst_stall * 1000:.0f} ms"
    )
    print("cadence: " + ("; ".join(failures) if failures else "ok"))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
