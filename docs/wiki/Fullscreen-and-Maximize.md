There are three related names in tiri: `fullscreen-window`, `maximize-column`, and `maximize-window-to-edges`.
The important part is that the current tiri behavior is more i3-like than stock niri: the two maximize actions are kept mostly as compatibility names, while the user-visible result is fullscreen-style behavior.

## Fullscreen windows

Windows can go fullscreen, usually seen with video players, presentations, or games.
You can force this via `fullscreen-window`, which is bound to <kbd>Mod</kbd><kbd>F</kbd> in the default config.

Fullscreen windows cover the entire screen.
Tiri renders a solid black backdrop behind them so fixed-size windows still sit on a fullscreen-sized surface, matching the Wayland protocol behavior.
When a fullscreen window is focused and not animating, it covers floating windows and the top layer-shell layer.
If you want notifications or launchers over fullscreen windows, configure them to use the overlay layer.

![Screenshot of a fullscreen window.](./img/fullscreen-window.png)

You can make a window open fullscreen, or prevent it from fullscreening on open, with the [`open-fullscreen`](./Configuration:-Window-Rules.md#open-fullscreen) window rule.

## Legacy maximize actions in tiri

`maximize-column` is still present in the IPC/config vocabulary, but in current tiri it does **not** just widen a column.
The default bind is <kbd>Mod</kbd><kbd>M</kbd>, and it currently toggles fullscreen-style behavior on the focused tiled window.

`maximize-window-to-edges` is bound to <kbd>Mod</kbd><kbd>Shift</kbd><kbd>M</kbd>.
In current tiri it is implemented as an alias of `fullscreen-window` for i3-like behavior.

This means that if you are looking for a "make the focused thing big" action in day-to-day usage, you should think in terms of fullscreen:

- <kbd>Mod</kbd><kbd>F</kbd>: explicit fullscreen action
- <kbd>Mod</kbd><kbd>M</kbd>: legacy maximize name, fullscreen-like result in tiri
- <kbd>Mod</kbd><kbd>Shift</kbd><kbd>M</kbd>: maximize-to-edges name, currently an alias of fullscreen

The old `column` terminology is legacy naming kept for compatibility with inherited niri APIs and config terms.

## Client maximize requests and window rules

Tiri still understands maximize-related protocol requests and window rules for compatibility with clients and inherited niri behavior.
In particular, [`open-maximized-to-edges`](./Configuration:-Window-Rules.md#open-maximized-to-edges) still exists and can affect windows that request to open maximized.

Similarly, some clients ask to be maximized or fullscreen during their initial configure sequence.
That is the best time for tiri to honor or override those requests.
If a client requests the state only after the initial configure sequence, the relevant `open-*` rules may no longer affect it because, from tiri's point of view, the window is already open.

## Common behaviors across fullscreen and maximize aliases

Fullscreen-style windows can only be in the tiling layout.
So if you fullscreen a [floating window](./Floating-Windows.md), tiri will move it into the tiling layout.
Leaving fullscreen restores it back to floating when appropriate.

These windows remain normal participants in the container tree.
You can still navigate to other windows with the regular focus and layout commands.

![Screenshot of the overview showing a fullscreen window with other windows side by side.](./img/fullscreen-window-in-overview.png)

## Windowed fullscreen

<sup>Upstream niri: 25.05</sup>

Tiri can also tell a window that it's in fullscreen without actually making it fullscreen, via the `toggle-windowed-fullscreen` action.
This is generally useful for screencasting browser-based presentations, when you want to hide the browser UI, but still have the window sized as a normal window.

When in windowed fullscreen, you can use the tiri action to maximize or unmaximize the window.
Window-side titlebar maximize buttons and gestures may not work, since the window will always think that it's in fullscreen.

See also windowed fullscreen on the [screencasting features wiki page](./Screencasting.md#windowed-fakedetached-fullscreen).


[struts]: ./Configuration:-Layout.md#struts
[gaps]: ./Configuration:-Layout.md#gaps
