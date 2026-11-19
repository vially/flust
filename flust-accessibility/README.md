# flust-accessibility

Exposes the semantics of the views of a Flutter engine to assistive technologies.

A Flust app can be used with assistive technologies, such as the Orca screen reader, on Linux with the `flust-sctk` backend. There is nothing to switch on in the app: what a Flutter app describes with its widgets and with `Semantics` is what a screen reader gets.

Flust has no accessibility support with the `flust-winit` backend.

The work is tracked in https://forge.lab.vially.dev/vially/flust/issues/6.

## What works

- **Reading the app.** Text, buttons, checkboxes, radio buttons, switches, sliders, links, images, headings with their level, text fields and scrollable areas, each with its label, value, hint and tooltip, and whether it is checked, selected, expanded, disabled or required.
- **Focus.** The widget that has the keyboard focus is the one the screen reader is on.
- **Activating things.** A screen reader can activate what can be tapped, move the focus to a widget, bring a widget into view and move a slider.
- **Text fields.** The text, the caret and the selection can be read, and the text and the selection can be set.
- **Announcements.** What an app announces with `SemanticsService.sendAnnouncement` is spoken, and so are changes in a `Semantics(liveRegion: true)`.
- **Desktop settings.** The app is told when the desktop asks for more contrast, for less motion or for no animations (`MediaQuery.highContrastOf`, `MediaQuery.disableAnimationsOf` and the like), how much text is to be scaled, and whether an assistive technology is in use (`MediaQuery.accessibleNavigationOf`).

## How it works

```
Flutter engine ──semantics updates──► flust-engine ──► flust-accessibility ──► AccessKit ──► AT-SPI ──► screen reader
               ◄──semantics actions──              ◄──                      ◄──           ◄──        ◄──
```

- `flust-engine` receives the semantics tree of a view from the engine, and the changes to it (`flust_engine::semantics`).
- This crate keeps that tree and translates it to an [AccessKit](https://accesskit.dev) tree. AccessKit puts it on AT-SPI, the accessibility bus of Linux desktops. What a screen reader asks for comes back the same way and is passed on to the engine as a semantics action.
- `flust-plugins` has the `flutter/accessibility` channel, which announcements come in through.
- `flust-sctk` ties these to the window, and reads the desktop settings from the settings portal.

Keeping the semantics tree up to date costs the framework time on every frame. So the engine is only asked for it while accessibility is switched on for the session, which a screen reader does when it starts. The app logs `semantics enabled: true` when that happens, and `semantics enabled: false` when it ends.

## Trying it

With a screen reader: start Orca, then the app.

Without one, [`tests/e2e/tools/atspi_tree.py`](../tests/e2e/tools/atspi_tree.py) prints what an application has on the accessibility bus. Accessibility has to be on for the session for there to be anything:

```sh
busctl --user set-property org.a11y.Bus /org/a11y/bus org.a11y.Status IsEnabled b true
tests/e2e/tools/atspi_tree.py my-app      # the name of the executable
```

It prints something like:

```
application 'my-app' []
  frame 'My App' [focused focusable enabled active]
    heading 'Inbox' [enabled]
    button 'Compose' description='Writes a new message' [enabled] actions=click
    entry 'Search' [focused focusable enabled] text='' caret=0
```

That setting is kept: it stays on, for every application, until it is set to `false` again. [`tests/e2e/README.md`](../tests/e2e/README.md) describes how the end-to-end test gets around that.

[Accerciser](https://gitlab.gnome.org/GNOME/accerciser) shows the same tree in a window.

## Limitations

- **Roles are approximate.** The engine does not pass on the role a widget gives itself (`SemanticsRole`: tab, menu, dialog, table and so on), only a set of flags. A widget that none of the flags describes is a group or a piece of text to a screen reader.
- **Some actions can not be asked for.** Long press, dismiss, custom semantics actions, scrolling in a direction, expanding and collapsing, and moving the caret by character or word have no way through AT-SPI as AccessKit implements it. Flust translates them where AccessKit has a counterpart, so they start to work when it passes them on.
- **A slider moves one step at a time.** Flutter can be asked to increase or decrease a slider, not to set it to a value. Setting a value moves it one step towards it. A slider only has a number at all if its value is one, such as `50%`.
- **Positions are relative to the window.** A Wayland client does not know where on the screen its window is. Tools that draw a highlight around the widget a screen reader is on may draw it in the wrong place.
- **No positions inside text.** The engine gives the text of a text field, not where its characters are. A screen magnifier can not follow the caret.
- **Lines of a text field are its paragraphs.** Where a long line is wrapped on screen is not known either, so moving by line moves to the next line break.
- **Parts of a string in another language** (`AttributedString` with a `LocaleStringAttribute`), and parts to be spelled out, are read like the rest of it.
- **One window.** A Flust app has one view today. The accessibility code keeps its state per view, but has not been used with more than one.
- **Not tried with a screen reader yet.** `tests/e2e/scenarios/semantics.yaml` checks a heading, a button, a checkbox, a text field, an announcement and the on and off switching through AT-SPI. The rest is covered by unit tests. Nobody has listened to any of it.

## A note on the engine

The engine does not fill in the size of the struct that holds the flags of a semantics node (`FlutterSemanticsFlags.struct_size`, as of Flutter 3.50). Flust takes a size of zero to mean all of the struct. An embedder that checks the size before reading a field gets nodes without flags: no buttons, no checkboxes, no focus.
