### Overview

<sup>Since: 25.02</sup>

You can switch a container to present windows as tabs, rather than as splits.
All tabs in a tabbed container have the same window size, so this is useful to get more vertical space.

This is one of the container layout modes in the i3-style tiling system, alongside SplitH, SplitV, and Stacked.

![Terminal with a tab indicator on the left.](https://github.com/user-attachments/assets/0e94ac0d-796d-4f85-a264-c105ef41c13f)

Use this bind to toggle a container between split and tabbed layout:

```kdl
binds {
   Mod+W { toggle-column-tabbed-display; }
}
```

All other binds remain the same: switch tabs with `focus-window-down/up`, navigate with directional focus commands.

Tabbed containers can go full-screen with multiple windows.

### Tab indicator

Tabbed containers show a tab indicator on the side.
You can click on the indicator to switch tabs.

See the [`tab-indicator` section in the layout section](./Configuration:-Layout.md#tab-indicator) to configure it.

By default, the indicator draws "outside" the container, so it can overlay other windows or go off-screen.
The `place-within-column` flag puts the indicator "inside" the container, adjusting the window size to make space for it.
This is especially useful for thicker tab indicators, or when you have very small gaps.

| Default | `place-within-column` |
| --- | --- |
| ![A screenshot showing 4 windows, with the middle column being focused. The tab indicator overflows onto the left column](https://github.com/user-attachments/assets/c2f51f50-3d87-403a-8beb-cbbe5ec5c880) | ![A screenshot showing 4 windows, with the middle column being focused. The tab indicator is contained within its respective column](https://github.com/user-attachments/assets/f1797cd0-d518-4be6-95b4-3540523c4370) |
