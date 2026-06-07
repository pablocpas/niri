### Overview

<sup>Upstream niri: 25.02</sup>

You can switch a container to present windows as tabs, rather than as splits.
All tabs in a tabbed container have the same window size, so this is useful to get more vertical space.

This is one of the container layout modes in the i3-style tiling system, alongside SplitH, SplitV, and Stacked.

Use this bind to set the focused container to tabbed layout:

```kdl
binds {
   Mod+W { set-layout-tabbed; }
}
```

Use `set-layout-stacked` for stacked layout and `toggle-split-layout` to switch split containers between horizontal and vertical:

```kdl
binds {
   Mod+S { set-layout-stacked; }
   Mod+E { toggle-split-layout; }
}
```

All other binds remain the same: switch tabs with `focus-window-down/up`, navigate with directional focus commands, and use `focus-parent` when you want to operate on the container itself.

Tabbed containers can go full-screen with multiple windows.

### Tab bar

Tabbed and stacked containers show a tab bar above their windows.
You can click on the tab bar to switch tabs.

See the [`tab-bar` section in the layout page](./Configuration:-Layout.md#tab-bar) to configure it.

The i3/sway profile also sets `show-in-split`, which renders a single-row title bar above split-layout tiles for a more traditional i3 look.
