#!/usr/bin/env python3
"""Run the probe app's scenarios and print one line of results for each.

    tests/e2e/run.py                      # all of them, against Flust
    tests/e2e/run.py drag-edges steps     # some of them
    tests/e2e/run.py --reference          # against Flutter's GTK embedder

Each scenario is a `waytape run`; its run directory is kept under
`.waytape/runs/<scenario>[-reference]`, replacing the previous one. Exits
with 1 if any scenario failed.
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
    "close",
]
REFERENCE = HERE / "app/build/reference/build/linux/x64/debug/bundle/flust_e2e_probe"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("scenarios", nargs="*", default=SCENARIOS, metavar="scenario")
    parser.add_argument(
        "--reference",
        action="store_true",
        help="run Flutter's own Linux embedder (see build-reference.sh) instead of Flust",
    )
    args = parser.parse_args()

    app = HERE / "app/target/debug/flust-e2e-probe"
    build = "build-app.sh"
    if args.reference:
        app, build = REFERENCE, "build-reference.sh"
    if not app.exists():
        sys.exit(f"{app} is missing: run tests/e2e/{build} first")

    failed = 0
    for name in args.scenarios:
        out = ROOT / ".waytape/runs" / (name + ("-reference" if args.reference else ""))
        shutil.rmtree(out, ignore_errors=True)
        command = ["waytape", "run", str(HERE / "scenarios" / f"{name}.yaml"), "--quiet", "--out", str(out)]
        if args.reference:
            command += ["--app", str(app)]
        subprocess.run(command, cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        try:
            report = json.loads((out / "report.json").read_text())
        except (OSError, ValueError):
            print(f"FAILED  {name}: no report; run `{' '.join(command)}` by hand")
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
        failed += result != "passed"
        print(f"{result.upper():7} {name}" + (": " + ", ".join(notes) if notes else ""))

    print(f"{len(args.scenarios) - failed} passed, {failed} failed; run directories in .waytape/runs/")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
