#!/usr/bin/env python3
"""Check how the app answers the compositor, from its Wayland protocol trace.

Reads the trace libwayland writes with `WAYLAND_DEBUG=1` (the scenarios set
it; it ends up in `app.log`) and follows the app's toplevel surface through
it: which sizes it was configured with, which configure it acknowledged,
and what it then committed.

A commit after `xdg_surface.ack_configure` is the app's answer to that
configure: it tells the compositor "this is my surface for the state you
asked for". The checks:

- **stale answers**: the first commit after an acknowledged configure must
  carry a buffer of the configured size. A commit without a new buffer, or
  with one of another size, answers the configure with old content.
- **scale**: at every commit the buffer's size must be a multiple of the
  buffer scale, and a new scale must come with a buffer drawn for it.
- **pacing**: buffers must not be committed faster than the output shows
  them. More than --refresh (plus a quarter) in any one second is counted,
  once per such second.
- **protocol errors**: none.

It also counts, without judging: configures, commits without a buffer, and
how long configures took to be answered with a buffer of their size.

Works on any app whose toplevel is one wl_surface with EGL or shm buffers.
Run by waytape as an analyzer, or by hand with a run directory. Exits with
1 if there are more violations than --max-violations allows.
"""

import argparse
import os
import re
import sys
from pathlib import Path

# "     2.771 err | [14:03:06.483099] {queue}  -> wl_surface#12.attach(wl_buffer#38, 0, 0)"
LINE = re.compile(
    r"^\s*(?P<time>[\d.]+) \w+ \| \[[\d:. ]+\] (?:\{[^}]*\} )?\s*(?P<out>-> )?"
    r"(?P<object>[\w]+#\d+)\.(?P<message>\w+)\((?P<args>.*)\)\s*$"
)
OBJECT = re.compile(r"(?:new id )?(\w+#\d+)")


class Surface:
    """The pending and current state of the toplevel's wl_surface."""

    def __init__(self):
        self.buffer = None  # (width, height) of the current buffer
        self.scale = 1
        self.pending_buffer = None
        self.pending_scale = None


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("run_dir", type=Path, nargs="?", default=os.environ.get("WAYTAPE_RUN_DIR"))
    parser.add_argument(
        "--max-violations",
        type=int,
        default=0,
        help="how many violations to accept before failing (default: 0)",
    )
    parser.add_argument("--refresh", type=int, default=60, help="the output's refresh rate (default: 60)")
    args = parser.parse_args()
    if args.run_dir is None:
        parser.error("give a run directory, or run as a waytape analyzer")

    buffers = {}  # wl_buffer#N -> (width, height)
    surface_id = xdg_surface_id = toplevel_id = None
    surface = Surface()
    toplevel_size = None  # from the last xdg_toplevel.configure
    configures = {}  # serial -> (time, size)
    acked = None  # (serial, time, size) acknowledged and not yet committed
    waiting = []  # (time, size) of configures not yet followed by their buffer
    latencies = []
    counts = {"configures": 0, "commits": 0, "bare commits": 0, "buffers": 0}
    violations = []
    seen = False
    recent = []  # times of the buffer commits of the last second
    paced_until = 0.0

    def violation(time, text):
        violations.append(f"t={time:.3f} {text}")

    with open(args.run_dir / "app.log", errors="replace") as log:
        for line in log:
            match = LINE.match(line)
            if match is None:
                continue
            seen = True
            time = float(match["time"])
            target, message, arguments = match["object"], match["message"], match["args"]
            outgoing = match["out"] is not None

            if message == "error" and target.startswith("wl_display#") and not outgoing:
                violation(time, f"protocol error: {arguments}")
            elif message == "get_xdg_surface" and xdg_surface_id is None:
                xdg_surface_id, surface_id = OBJECT.findall(arguments)[:2]
            elif message == "get_toplevel" and target == xdg_surface_id:
                toplevel_id = OBJECT.findall(arguments)[0]
            elif message in ("create_immed", "create_buffer") and outgoing:
                # zwp_linux_buffer_params_v1.create_immed(new id, width, height, format, flags)
                # wl_shm_pool.create_buffer(new id, offset, width, height, stride, format)
                fields = [field.strip() for field in arguments.split(",")]
                size = fields[1:3] if message == "create_immed" else fields[2:4]
                buffers[OBJECT.findall(fields[0])[0]] = (int(size[0]), int(size[1]))
            elif target == toplevel_id and message == "configure" and not outgoing:
                width, height = (int(field) for field in arguments.split(",")[:2])
                toplevel_size = (width, height)
            elif target == xdg_surface_id and message == "configure" and not outgoing:
                counts["configures"] += 1
                configures[int(arguments)] = (time, toplevel_size)
            elif target == xdg_surface_id and message == "ack_configure":
                serial = int(arguments)
                if serial in configures:
                    acked = (serial, *configures[serial])
            elif target == surface_id and message == "attach":
                name = OBJECT.findall(arguments)
                surface.pending_buffer = buffers.get(name[0]) if name else None
            elif target == surface_id and message == "set_buffer_scale":
                surface.pending_scale = int(arguments)
            elif target == surface_id and message == "commit":
                counts["commits"] += 1
                new_buffer = surface.pending_buffer
                if new_buffer is None:
                    counts["bare commits"] += 1
                else:
                    counts["buffers"] += 1
                    recent = [at for at in recent if at > time - 1] + [time]
                    if len(recent) > args.refresh * 1.25 and time >= paced_until:
                        violation(
                            time,
                            f"{len(recent)} buffers committed in one second, for an output at {args.refresh} Hz",
                        )
                        # At most one of these per second, however long it lasts.
                        paced_until = time + 1
                    surface.buffer = new_buffer
                    surface.pending_buffer = None
                if surface.pending_scale is not None:
                    if new_buffer is None and surface.pending_scale != surface.scale:
                        violation(
                            time,
                            f"buffer scale {surface.pending_scale} committed without a buffer drawn for it",
                        )
                    surface.scale = surface.pending_scale
                    surface.pending_scale = None
                if surface.buffer is not None and any(side % surface.scale for side in surface.buffer):
                    violation(
                        time,
                        "buffer {}x{} is not a multiple of buffer scale {}".format(*surface.buffer, surface.scale),
                    )

                if acked is not None:
                    serial, configured_at, size = acked
                    acked = None
                    # A size of 0x0 leaves the size to the app.
                    if size is not None and 0 not in size:
                        expected = (size[0] * surface.scale, size[1] * surface.scale)
                        if new_buffer is None and surface.buffer == expected:
                            pass  # nothing to redraw: the surface already has that size
                        elif new_buffer is None:
                            have = "no buffer yet" if surface.buffer is None else "{}x{}".format(*surface.buffer)
                            violation(
                                time,
                                "configure {} ({}x{}) answered by a commit without a new buffer "
                                "(attached: {})".format(serial, *size, have),
                            )
                            waiting.append((configured_at, expected))
                        elif new_buffer != expected:
                            violation(
                                time,
                                "configure {} ({}x{}) answered with a {}x{} buffer".format(serial, *size, *new_buffer),
                            )
                            waiting.append((configured_at, expected))
                        else:
                            latencies.append(time - configured_at)
                if new_buffer is not None:
                    # Configures answered late: by this buffer, or overtaken.
                    for configured_at, expected in waiting:
                        if expected == new_buffer:
                            latencies.append(time - configured_at)
                    waiting = [entry for entry in waiting if entry[1] != new_buffer and entry[0] > time - 1]

    if not seen or surface_id is None:
        print("protocol: no Wayland trace of a toplevel in app.log; set WAYLAND_DEBUG=1 in app.env", file=sys.stderr)
        return 1

    for text in violations[:5]:
        print(text)
    if len(violations) > 5:
        print(f"... and {len(violations) - 5} more")
    print(
        "configures: {configures}, commits: {commits} "
        "({buffers} with a buffer, {bare commits} without)".format(**counts)
    )
    if latencies:
        latencies.sort()
        print(
            "configure to its buffer: median {:.0f} ms, worst {:.0f} ms, over {} configures".format(
                latencies[len(latencies) // 2] * 1000, latencies[-1] * 1000, len(latencies)
            )
        )
    print(f"protocol violations: {len(violations)}")
    return 1 if len(violations) > args.max_violations else 0


if __name__ == "__main__":
    sys.exit(main())
