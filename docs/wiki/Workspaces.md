### Overview

Tiri uses i3/sway-like numeric workspaces with dynamic creation and cleanup.

The first workspace is `1`. Numeric workspaces are addressed by their number: `focus-workspace 2`
focuses workspace `2`, creating it if needed, and `move-column-to-workspace 4` creates workspace
`4` if needed and moves the column there.

Numeric workspaces are ordered by their number in the layout, IPC, and workspace protocols. For
example, if you create workspace `5` and later create workspace `2`, they will be ordered as
`1`, `2`, `5`.

Auto-created numeric workspaces are temporary: if they remain empty and become unfocused, they
disappear. Existing higher-numbered workspaces keep their numbers; workspace `3` does not become
workspace `2` just because `2` disappeared.

There's still an internal empty workspace at the end of every monitor so that layout operations can
open a new empty space. This internal workspace is not a numbered user workspace and is hidden from
workspace lists when another real workspace exists.

You can switch between visible workspaces on a monitor with `focus-workspace-down` and
`focus-workspace-up`.

Here's a visual representation that shows two monitors and their workspaces.
The left monitor has three workspaces (two with windows, plus one empty), and the right monitor has two workspaces (one with windows, plus one empty).

<picture>
    <source media="(prefers-color-scheme: dark)" srcset="./img/workspaces-dark.png">
    <img alt="Two monitors. First with three workspaces, second with two workspaces." src="./img/workspaces-light.png">
</picture>

You can move a workspace to a different monitor using binds like `move-workspace-to-monitor-left/right/up/down` and `move-workspace-to-monitor-next/previous`.
Numeric workspaces keep their numeric order; `move-workspace-up/down` does not reorder numbered
workspaces.

When you disconnect a monitor, its workspaces will automatically move to a different monitor.
But, they will also "remember" their original monitor, so when you reconnect it, the workspaces will automatically move back to it.

> [!TIP]
> From other tiling WMs, you may be used to thinking about workspaces like this: "These are all of my workspaces. I can show workspace X on my first monitor, and workspace Y on my second monitor."
> In niri, instead, think like this: "My first monitor contains these workspaces, including X and Y, and my second monitor contains these other workspaces. I can switch my first monitor to workspace X or Y. I can move workspace Y to my second monitor to show it there."

### Addressing Workspaces

When you want to have a more permanent workspace, you can create a [named workspace](./Configuration:-Named-Workspaces.md) in the config or via the `set-workspace-name` action.
You can refer to named workspaces by name, e.g. `focus-workspace "browser"`, and they won't disappear when they become empty.

### Example workflow

This is how I like to use workspaces.

I will usually have my browser on the topmost workspace, then one workspace per project (or a "thing") I'm working on.
On a single workspace I have multiple windows arranged in the tiling layout that I switch between frequently.
When the workspace gets too cluttered, I'll move some windows to a new workspace or use tabbed/stacked containers to organize them better.

I actively move workspaces up and down as I'm working on things to make what I need accessible in one motion.
For example, I usually frequently switch between the browser and whatever I'm doing, so I always move whatever I'm currently doing to right below the browser, so a single `focus-workspace-up/down` gets me where I want.
