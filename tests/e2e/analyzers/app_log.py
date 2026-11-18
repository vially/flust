#!/usr/bin/env python3
"""Check that the app did not complain: no warnings, errors or panics.

Reads `app.log` of a waytape run and fails on lines the embedder logged at
WARN or ERROR level, on Rust panics and on Flutter engine errors. Lines
that match an --allow pattern are let through.

Run by waytape as an analyzer, or by hand with a run directory.
"""

import argparse
import os
import re
import sys
from pathlib import Path

ANSI = re.compile(r"\x1b\[[0-9;]*m")
# "<time> <stream> | <timestamp>  WARN <module>: <message>", as tracing prints it.
COMPLAINT = re.compile(r"\| \S+\s+(WARN|ERROR) |panicked at|\[ERROR:flutter|Unhandled Exception")
PREFIX = re.compile(r"^\s*([\d.]+) \w+ \| (?:\S+Z\s+)?")

# Not about rendering: the template plugins do not cover every channel.
ALLOWED = [r"No plugin registered to handle messages from channel"]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("run_dir", type=Path, nargs="?", default=os.environ.get("WAYTAPE_RUN_DIR"))
    parser.add_argument("--allow", action="append", default=[], help="a regex of lines to let through")
    args = parser.parse_args()
    if args.run_dir is None:
        parser.error("give a run directory, or run as a waytape analyzer")
    allowed = [re.compile(pattern) for pattern in ALLOWED + args.allow]

    # The same message many times over is one complaint.
    complaints = {}
    with open(args.run_dir / "app.log", errors="replace") as log:
        for line in log:
            line = ANSI.sub("", line).rstrip()
            if not COMPLAINT.search(line) or any(pattern.search(line) for pattern in allowed):
                continue
            prefix = PREFIX.match(line)
            time = prefix[1] if prefix else "?"
            text = line[prefix.end() :] if prefix else line
            first, count = complaints.get(text, (time, 0))
            complaints[text] = (first, count + 1)

    for text, (first, count) in complaints.items():
        print(f"{count}x, first at t={first}: {text}")
    total = sum(count for _, count in complaints.values())
    print(f"warnings and errors in the app's log: {total}")
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())
