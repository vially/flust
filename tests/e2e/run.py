#!/usr/bin/env python3
"""Run the probe app's scenarios and print one line of results for each.

    tests/e2e/run.py                      # all of them, against Flust
    tests/e2e/run.py drag-edges steps     # some of them
    tests/e2e/run.py --reference          # against Flutter's GTK embedder
    tests/e2e/run.py --target surface     # rendering straight into the window
    tests/e2e/run.py --repeat 5 storm     # five times, to catch what is rare

Each scenario is a `waytape run`; its run directory is kept under
`.waytape/runs/<scenario>[-reference|-surface]`, replacing the previous
one (with --repeat, the last failed run of a scenario is the one kept).
Exits with 1 if any run failed.
"""

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
# The scenarios for the probe app; the others run the template app.
SCENARIOS = [
    "startup",
    "drag-edges",
    "drag-speeds",
    "drag-animated",
    "steps",
    "storm",
    "states",
    "input",
    "hidden",
    "scale",
    "scale-change",
    "close",
    "semantics",
]
# These read the app through the accessibility API of the desktop, which has
# to be switched on for that. They get a session bus of their own, so that
# it is not switched on for the applications of whoever runs this.
PRIVATE_BUS = {"semantics"}
# They wait for what Flust logs, and say nothing about the reference.
FLUST_ONLY = {"semantics"}
REFERENCE = HERE / "app/build/reference/build/linux/x64/debug/bundle/flust_e2e_probe"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("scenarios", nargs="*", default=SCENARIOS, metavar="scenario")
    parser.add_argument(
        "--reference",
        action="store_true",
        help="run Flutter's own Linux embedder (see build-reference.sh) instead of Flust",
    )
    parser.add_argument(
        "--target",
        choices=["framebuffer", "surface"],
        default="framebuffer",
        help="what Flust has the engine render into: a framebuffer that is then copied to the window "
        "(the default), or the window's surface itself",
    )
    parser.add_argument("--repeat", type=int, default=1, metavar="N", help="run each scenario N times")
    args = parser.parse_args()
    if args.reference and args.target != "framebuffer":
        parser.error("--target is for Flust, not for the reference")

    app = HERE / "app/target/debug/flust-e2e-probe"
    build = "build-app.sh"
    if args.reference:
        app, build = REFERENCE, "build-reference.sh"
    if not app.exists():
        sys.exit(f"{app} is missing: run tests/e2e/{build} first")

    suffix = "-reference" if args.reference else "-surface" if args.target == "surface" else ""
    runs = failed = 0
    for name in args.scenarios:
        if args.reference and name in FLUST_ONLY:
            print(f"SKIPPED {name}: not for the reference")
            continue
        out = ROOT / ".waytape/runs" / (name + suffix)
        command = ["waytape", "run", str(HERE / "scenarios" / f"{name}.yaml"), "--quiet", "--out", str(out)]
        if args.reference:
            command += ["--app", str(app)]
        if args.target == "surface":
            command += ["--env", "PROBE_TARGET=surface"]
        if name in PRIVATE_BUS:
            # The accessibility bus is a second bus. Its launcher starts
            # `dbus-broker` by default, which only runs under systemd.
            command = ["env", "ATSPI_DBUS_IMPLEMENTATION=dbus-daemon", "dbus-run-session", "--", *command]

        for attempt in range(args.repeat):
            # Keep a failed run for inspection unless this one fails as well.
            scratch = out.with_name(out.name + ".new")
            shutil.rmtree(scratch, ignore_errors=True)
            command[command.index("--out") + 1] = str(scratch)
            subprocess.run(command, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            runs += 1

            label = name + (f" [{attempt + 1}/{args.repeat}]" if args.repeat > 1 else "")
            try:
                report = json.loads((scratch / "report.json").read_text())
            except (OSError, ValueError):
                shown = " ".join(command).replace(str(scratch), str(out))
                print(f"FAILED  {label}: no report; run `{shown}` by hand")
                failed += 1
                continue
            notes = [
                f'step {step["index"]} ({step["step"]})' for step in report["steps"] if step["result"] == "failed"
            ]
            for analyzer in report["analyzers"]:
                if analyzer["exit_code"] != 0:
                    # "python3 ../analyzers/protocol.py --scale 2" -> "protocol"
                    script = next(word for word in analyzer["command"].split() if word.endswith(".py"))
                    notes.append(Path(script).stem)
            result = report["result"]
            print(f"{result.upper():7} {label}" + (": " + ", ".join(notes) if notes else ""), flush=True)

            kept_failure = out.exists() and json.loads((out / "report.json").read_text())["result"] != "passed"
            if result != "passed":
                failed += 1
            if result != "passed" or attempt == 0 or not kept_failure:
                shutil.rmtree(out, ignore_errors=True)
                scratch.rename(out)
            else:
                shutil.rmtree(scratch)

    print(f"{runs - failed} passed, {failed} failed; run directories in .waytape/runs/")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
