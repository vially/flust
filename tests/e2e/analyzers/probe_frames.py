#!/usr/bin/env python3
"""Check that every displayed frame of the probe app is right for its window.

In each frame the compositor displayed while the scenario's steps ran, the
app's picture must be exactly where the window is and exactly its size, be
complete, and have been drawn for that size. A frame fails when the picture
is:

- `size(...)`: drawn for another size than the window's, i.e. the app is
  behind (or ahead of) a resize. Sway centres a picture that is too small
  and cuts off one that is too large;
- `offset(...)`: the right size in the wrong place;
- `damaged(...)`: the right size in the right place, but not all there:
  a corner or the code rows are missing or moved, e.g. a frame copied
  into a buffer of another size, shifted, cropped or scaled;
- `no-content`: not there at all.

Run by waytape as an analyzer, or by hand with a run directory. Writes
`probe-frames.tsv` into the run directory and exits with 1 if more frames
are flagged than --max-flagged allows.
"""

import argparse
import sys

import probe


def longest_run(frames):
    """The longest stretch of consecutive flagged frames: (count, seconds)."""
    best, start, count = (0, 0.0), None, 0
    for frame in frames:
        if frame.problems:
            start = frame.time if start is None else start
            count += 1
            best = max(best, (count, frame.time - start))
        else:
            start, count = None, 0
    return best


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    probe.run_dir_argument(parser)
    parser.add_argument(
        "--max-flagged",
        type=int,
        default=0,
        help="how many flagged frames to accept before failing (default: 0)",
    )
    args = parser.parse_args()
    if args.run_dir is None:
        parser.error("give a run directory, or run as a waytape analyzer")

    frames = probe.load_run(args.run_dir, args.scale, scale_from=probe.scale_changes(args))
    if not frames:
        print("probe_frames: no frames were recorded while the steps ran", file=sys.stderr)
        return 1

    with open(args.run_dir / "probe-frames.tsv", "w") as table:
        table.write("frame\ttime\tstep\tsection\twindow\tcontent\tencoded\tcounter\ttaps\tproblems\n")
        for frame in frames:
            encoded = "-" if frame.encoded is None else "{}x{}".format(*frame.encoded)
            table.write(
                "\t".join(
                    str(value)
                    for value in (
                        frame.index,
                        f"{frame.time:.3f}",
                        frame.step,
                        frame.section,
                        probe.rect_text(frame.window),
                        probe.rect_text(frame.content),
                        encoded,
                        "-" if frame.counter is None else frame.counter,
                        "-" if frame.taps is None else frame.taps,
                        " ".join(frame.problems) or "-",
                    )
                )
                + "\n"
            )

    flagged = [frame for frame in frames if frame.problems]
    for frame in flagged[:5]:
        print(
            f"frame {frame.index} t={frame.time:.3f} window={probe.rect_text(frame.window)} "
            + " ".join(frame.problems)
        )
    if len(flagged) > 5:
        print(f"... and {len(flagged) - 5} more, in probe-frames.tsv")

    # The summary last: waytape copies the end of this output into summary.txt.
    print("section: flagged/frames, longest run of flagged frames")
    for name, group in probe.sections(frames).items():
        bad = sum(1 for frame in group if frame.problems)
        count, seconds = longest_run(group)
        print(f"  {name}: {bad}/{len(group)}, {count} frames ({seconds * 1000:.0f} ms)")
    kinds = {}
    for frame in flagged:
        for problem in frame.problems:
            kind = problem.split("(")[0]
            kinds[kind] = kinds.get(kind, 0) + 1
    by_kind = ", ".join(f"{kind}: {count}" for kind, count in sorted(kinds.items())) or "none"
    print(f"problems: {by_kind}")
    print(f"frames not matching their window: {len(flagged)} of {len(frames)}")
    return 1 if len(flagged) > args.max_flagged else 0


if __name__ == "__main__":
    sys.exit(main())
