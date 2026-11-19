#!/usr/bin/env python3
"""Reads an application through AT-SPI, the accessibility API that screen
readers use.

    atspi_tree.py APP                     print the tree of the application
    atspi_tree.py APP --expect NODE ...   fail unless it has these nodes
    atspi_tree.py APP --click NAME        activate the node called NAME
    atspi_tree.py APP --set-text NAME TEXT
                                          replace the text of a text field
    atspi_tree.py APP --select NAME START END
                                          select a part of its text
    atspi_tree.py APP --expect-text NAME TEXT
                                          fail unless that is its text

APP is the name the application has on the accessibility bus: the name of
its executable. Waits for it to show up, and for it to have a window.

NODE is `role:name`, or `role:name:state,state` for a node that also has to
be in these states. Roles and states are spelled as the tree prints them. A
node that is not there yet is waited for.
"""

import argparse
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

STATES = [
    ("focused", Atspi.StateType.FOCUSED),
    ("focusable", Atspi.StateType.FOCUSABLE),
    ("checked", Atspi.StateType.CHECKED),
    ("checkable", Atspi.StateType.CHECKABLE),
    ("enabled", Atspi.StateType.ENABLED),
    ("selected", Atspi.StateType.SELECTED),
    ("expanded", Atspi.StateType.EXPANDED),
    ("active", Atspi.StateType.ACTIVE),
]


def find_app(name):
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is not None and app.get_name() == name:
            return app
    return None


def wait_for_app(name, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        app = find_app(name)
        if app is not None and app.get_child_count() > 0:
            return app
        time.sleep(0.2)
    sys.exit(f"no application called {name!r} with a window on the accessibility bus")


def walk(node, depth=0):
    yield node, depth
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from walk(child, depth + 1)


def describe(node):
    parts = [node.get_role_name(), repr(node.get_name())]
    description = node.get_description()
    if description:
        parts.append(f"description={description!r}")
    states = states_of(node)
    parts.append("[" + " ".join(name for name, _ in STATES if name in states) + "]")
    try:
        actions = [node.get_localized_name(i) for i in range(node.get_n_actions())]
    except Exception:
        actions = []
    if actions:
        parts.append("actions=" + ",".join(actions))
    text = text_of(node)
    if text is not None:
        parts.append(f"text={text!r} caret={Atspi.Text.get_caret_offset(node)}")
        if Atspi.Text.get_n_selections(node) > 0:
            selection = Atspi.Text.get_selection(node, 0)
            parts.append(f"selection={selection.start_offset},{selection.end_offset}")
    try:
        extents = node.get_extents(Atspi.CoordType.WINDOW)
        parts.append(f"at={extents.x},{extents.y} size={extents.width}x{extents.height}")
    except Exception:
        pass
    return " ".join(parts)


def states_of(node):
    states = node.get_state_set()
    return {name for name, state in STATES if states.contains(state)}


def has_node(app, spec):
    # A name can have a colon in it. What follows the last one is only a
    # list of states if all of it is.
    role, _, name = spec.partition(":")
    wanted = set()
    head, colon, tail = name.rpartition(":")
    if colon and set(tail.split(",")) <= {state for state, _ in STATES}:
        name, wanted = head, set(tail.split(","))
    return any(
        node.get_role_name() == role and node.get_name() == name and wanted <= states_of(node)
        for node, _ in walk(app)
    )


def text_of(node):
    """The text of a node that has any, such as a text field."""
    if node.get_text_iface() is None:
        return None
    return Atspi.Text.get_text(node, 0, Atspi.Text.get_character_count(node))


def named(app, name):
    for node, _ in walk(app):
        if node.get_name() == name:
            return node
    sys.exit(f"no node called {name!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("app")
    parser.add_argument("--expect", nargs="+", metavar="NODE", default=[])
    parser.add_argument("--click", metavar="NAME")
    parser.add_argument("--set-text", nargs=2, metavar=("NAME", "TEXT"))
    parser.add_argument("--select", nargs=3, metavar=("NAME", "START", "END"))
    parser.add_argument("--expect-text", nargs=2, metavar=("NAME", "TEXT"))
    parser.add_argument("--timeout", type=float, default=4.0)
    args = parser.parse_args()

    app = wait_for_app(args.app, args.timeout)

    if args.click is not None:
        node = named(app, args.click)
        if node.get_n_actions() < 1 or not node.do_action(0):
            sys.exit(f"{args.click!r} could not be activated")
        return

    if args.set_text is not None:
        name, text = args.set_text
        if not Atspi.EditableText.set_text_contents(named(app, name), text):
            sys.exit(f"the text of {name!r} could not be set")
        return

    if args.select is not None:
        name, start, end = args.select
        node = named(app, name)
        # A text field without a selection has none to change.
        start, end = int(start), int(end)
        if not (Atspi.Text.set_selection(node, 0, start, end) or Atspi.Text.add_selection(node, start, end)):
            sys.exit(f"the selection of {name!r} could not be set")
        return

    # Changes take a moment to arrive.
    deadline = time.monotonic() + args.timeout
    missing = args.expect

    def has_text():
        if args.expect_text is None:
            return True
        name, text = args.expect_text
        return any(node.get_name() == name and text_of(node) == text for node, _ in walk(app))

    while (missing or not has_text()) and time.monotonic() < deadline:
        missing = [spec for spec in missing if not has_node(app, spec)]
        if missing or not has_text():
            time.sleep(0.1)
    if not has_text():
        missing = [*missing, "the text {1!r} in {0!r}".format(*args.expect_text)]

    for node, depth in walk(app):
        print("  " * depth + describe(node))
    if missing:
        sys.exit("missing: " + ", ".join(missing))


if __name__ == "__main__":
    main()
