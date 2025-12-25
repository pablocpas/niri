# Contexto: Estructuras de Datos de i3 y Sway para Tiling Window Manager

Este documento recopila información sobre las estructuras de datos y algoritmos
de i3 y Sway para implementar un tiling window manager basado en niri.

**Objetivo**: Convertir niri (scrolling WM) en un tiling WM estilo i3/Sway usando slotmap en Rust

---


## 1. Conceptos Fundamentales


### 1.1 Árbol de Contenedores (Container Tree)

Tanto i3 como Sway utilizan una estructura de árbol para organizar ventanas y espacios de trabajo:

- **Root Container**: Raíz del árbol (normalmente representa el display/output)
- **Output Container**: Representa una pantalla física
- **Workspace Container**: Espacio de trabajo (1-N)
- **Split Container**: Contenedor de división (horizontal/vertical)
- **Window Container**: Hoja del árbol, ventana real

```
Root
├── Output 1
│   ├── Workspace 1
│   │   ├── Split (H)
│   │   │   ├── Window A
│   │   │   └── Window B
│   │   └── Window C
│   └── Workspace 2
│       └── Window D
└── Output 2
    └── Workspace 3
```

**Clave para Rust/slotmap**:
- Cada nodo es una entrada en un SlotMap
- Los nodos guardan SlotMap keys de sus hijos/padres
- Permite referencias seguras sin lifetimes complicados

## 2. Análisis de i3 (C)


### 2.1 Estructura de Datos Principal: Con (Container)

**Definición de la estructura Container en i3**:

```
i3/include/data.h:struct Con {
```


### 2.2 Headers Clave de i3

**tree.h** - Operaciones del árbol:

**Archivo**: `i3/include/tree.h`

```c
/*
 * vim:ts=4:sw=4:expandtab
 *
 * i3 - an improved tiling window manager
 * © 2009 Michael Stapelberg and contributors (see also: LICENSE)
 *
 * tree.c: Everything that primarily modifies the layout tree data structure.
 *
 */
#pragma once

#include <config.h>

extern Con *croot;
/* TODO: i am not sure yet how much access to the focused container should
 * be permitted to source files */
extern Con *focused;
TAILQ_HEAD(all_cons_head, Con);
extern struct all_cons_head all_cons;

/**
 * Initializes the tree by creating the root node, adding all RandR outputs
 * to the tree (that means randr_init() has to be called before) and
 * assigning a workspace to each RandR output.
 *
 */
void tree_init(xcb_get_geometry_reply_t *geometry);

/**
 * Opens an empty container in the current container
 *
 */
Con *tree_open_con(Con *con, i3Window *window);

/**
 * Splits (horizontally or vertically) the given container by creating a new
 * container which contains the old one and the future ones.
 *
 */
void tree_split(Con *con, orientation_t orientation);

/**
 * Moves focus one level up. Returns true if focus changed.
 *
 */
bool level_up(void);

/**
 * Moves focus one level down. Returns true if focus changed.
 *
 */
bool level_down(void);

/**
 * Renders the tree, that is rendering all outputs using render_con() and
 * pushing the changes to X11 using x_push_changes().
 *
 */
void tree_render(void);

/**
 * Changes focus in the given direction
 *
 */
void tree_next(Con *con, direction_t direction);

/**
 * Get the previous / next sibling
 *
 */
Con *get_tree_next_sibling(Con *con, position_t direction);

/**
 * Closes the given container including all children.
 * Returns true if the container was killed or false if just WM_DELETE was sent
 * and the window is expected to kill itself.
 *
 * The dont_kill_parent flag is specified when the function calls itself
 * recursively while deleting a containers children.
 *
 */
bool tree_close_internal(Con *con, kill_window_t kill_window, bool dont_kill_parent);

/**
 * Loads tree from ~/.i3/_restart.json (used for in-place restarts).
 *
 */
bool tree_restore(const char *path, xcb_get_geometry_reply_t *geometry);

/**
 * tree_flatten() removes pairs of redundant split containers, e.g.:
 *       [workspace, horizontal]
 *   [v-split]           [child3]
 *   [h-split]
 * [child1] [child2]
 * In this example, the v-split and h-split container are redundant.
 * Such a situation can be created by moving containers in a direction which is
 * not the orientation of their parent container. i3 needs to create a new
 * split container then and if you move containers this way multiple times,
 * redundant chains of split-containers can be the result.
 *
 */
void tree_flatten(Con *child);

```

**con.h** - Definiciones de contenedores:

**Archivo**: `i3/include/con.h`

```c
/*
 * vim:ts=4:sw=4:expandtab
 *
 * i3 - an improved tiling window manager
 * © 2009 Michael Stapelberg and contributors (see also: LICENSE)
 *
 * con.c: Functions which deal with containers directly (creating containers,
 *        searching containers, getting specific properties from containers,
 *        …).
 *
 */
#pragma once

#include <config.h>

/**
 * Create a new container (and attach it to the given parent, if not NULL).
 * This function only initializes the data structures.
 *
 */
Con *con_new_skeleton(Con *parent, i3Window *window);

/**
 * A wrapper for con_new_skeleton, to retain the old con_new behaviour
 *
 */
Con *con_new(Con *parent, i3Window *window);

/**
 * Frees the specified container.
 *
 */
void con_free(Con *con);

/**
 * Sets input focus to the given container. Will be updated in X11 in the next
 * run of x_push_changes().
 *
 */
void con_focus(Con *con);

/**
 * Sets input focus to the given container and raises it to the top.
 *
 */
void con_activate(Con *con);

/**
 * Activates the container like in con_activate but removes fullscreen
 * restrictions and properly warps the pointer if needed.
 *
 */
void con_activate_unblock(Con *con);

/**
 * Closes the given container.
 *
 */
void con_close(Con *con, kill_window_t kill_window);

/**
 * Returns true when this node is a leaf node (has no children)
 *
 */
bool con_is_leaf(Con *con);

/**
 * Returns true when this con is a leaf node with a managed X11 window (e.g.,
 * excluding dock containers)
 */
bool con_has_managed_window(Con *con);

/**
 * Returns true if a container should be considered split.
 *
 */
bool con_is_split(Con *con);

/**
 * This will only return true for containers which have some parent with
 * a tabbed / stacked parent of which they are not the currently focused child.
 *
 */
bool con_is_hidden(Con *con);

/**
 * Returns true if the container is maximized in the given orientation.
 *
 * If the container is floating or fullscreen, it is not considered maximized.
 * Otherwise, it is maximized if it doesn't share space with any other
 * container in the given orientation. For example, if a workspace contains
 * a single splitv container with three children, none of them are considered
 * vertically maximized, but they are all considered horizontally maximized.
 *
 * Passing "maximized" hints to the application can help it make the right
 * choices about how to draw its borders. See discussion in
 * https://github.com/i3/i3/pull/2380.
 *
 */
bool con_is_maximized(Con *con, orientation_t orientation);

/**
 * Returns whether the container or any of its children is sticky.
 *
 */
bool con_is_sticky(Con *con);

/**
 * Returns true if this node has regular or floating children.
 *
 */
bool con_has_children(Con *con);

/**
 * Returns true if this node accepts a window (if the node swallows windows,
 * it might already have swallowed enough and cannot hold any more).
 *
 */
bool con_accepts_window(Con *con);

/**
 * Gets the output container (first container with CT_OUTPUT in hierarchy) this
 * node is on.
 *
 */
Con *con_get_output(Con *con);

/**
 * Gets the workspace container this node is on.
 *
 */
Con *con_get_workspace(Con *con);

/**
 * Searches parents of the given 'con' until it reaches one with the specified
 * 'orientation'. Aborts when it comes across a floating_con.
 *
 */
Con *con_parent_with_orientation(Con *con, orientation_t orientation);

/**
 * Returns the first fullscreen node below this node.
 *
 */
Con *con_get_fullscreen_con(Con *con, fullscreen_mode_t fullscreen_mode);

/**
 * Returns the fullscreen node that covers the given workspace if it exists.
 * This is either a CF_GLOBAL fullscreen container anywhere or a CF_OUTPUT
 * fullscreen container in the workspace.
 *
 */
Con *con_get_fullscreen_covering_ws(Con *ws);

/**
 * Returns true if the container is internal, such as __i3_scratch
 *
 */
bool con_is_internal(Con *con);

/**
 * Returns true if the node is floating.
 *
 */
bool con_is_floating(Con *con);

/**
 * Returns true if the container is a docked container.
 *
 */
bool con_is_docked(Con *con);

/**
 * Checks if the given container is either floating or inside some floating
 * container. It returns the FLOATING_CON container.
 *
 */
Con *con_inside_floating(Con *con);

/**
 * Checks if the given container is inside a focused container.
 *
 */
bool con_inside_focused(Con *con);

/**
 * Checks if the container has the given parent as an actual parent.
 *
 */
bool con_has_parent(Con *con, Con *parent);

/**
 * Returns the container with the given client window ID or NULL if no such
 * container exists.
 *
 */
Con *con_by_window_id(xcb_window_t window);

/**
 * Returns the container with the given container ID or NULL if no such
 * container exists.
 *
 */
Con *con_by_con_id(long target);

/**
 * Returns true if the given container (still) exists.
 * This can be used, e.g., to make sure a container hasn't been closed in the meantime.
 *
 */
bool con_exists(Con *con);

/**
 * Returns the container with the given frame ID or NULL if no such container
 * exists.
 *
 */
Con *con_by_frame_id(xcb_window_t frame);

/**
 * Returns the container with the given mark or NULL if no such container
 * exists.
 *
 */
Con *con_by_mark(const char *mark);

/**
 * Start from a container and traverse the transient_for linked list. Returns
 * true if target window is found in the list. Protects againsts potential
 * cycles.
 *
 */
bool con_find_transient_for_window(Con *start, xcb_window_t target);

/**
 * Returns true if and only if the given containers holds the mark.
 *
 */
bool con_has_mark(Con *con, const char *mark);

/**
 * Toggles the mark on a container.
 * If the container already has this mark, the mark is removed.
 * Otherwise, the mark is assigned to the container.
 *
 */
void con_mark_toggle(Con *con, const char *mark, mark_mode_t mode);

/**
 * Assigns a mark to the container.
 *
 */
void con_mark(Con *con, const char *mark, mark_mode_t mode);

/**
 * Removes marks from containers.
 * If con is NULL, all containers are considered.
 * If name is NULL, this removes all existing marks.
 * Otherwise, it will only remove the given mark (if it is present).
 *
 */
void con_unmark(Con *con, const char *name);

/**
 * Returns the first container below 'con' which wants to swallow this window
 * TODO: priority
 *
 */
Con *con_for_window(Con *con, i3Window *window, Match **store_match);

/**
 * Iterate over the container's focus stack and return an array with the
 * containers inside it, ordered from higher focus order to lowest.
 *
 */
Con **get_focus_order(Con *con);

/**
 * Clear the container's focus stack and re-add it using the provided container
 * array. The function doesn't check if the provided array contains the same
 * containers with the previous focus stack but will not add floating containers
 * in the new focus stack if container is not a workspace.
 *
 */
void set_focus_order(Con *con, Con **focus_order);

/**
 * Returns the number of children of this container.
 *
 */
int con_num_children(Con *con);

/**
 * Returns the number of visible non-floating children of this container.
 * For example, if the container contains a hsplit which has two children,
 * this will return 2 instead of 1.
 */
int con_num_visible_children(Con *con);

/**
 * Count the number of windows (i.e., leaf containers).
 *
 */
int con_num_windows(Con *con);

/**
 * Attaches the given container to the given parent. This happens when moving
 * a container or when inserting a new container at a specific place in the
 * tree.
 *
 * ignore_focus is to just insert the Con at the end (useful when creating a
 * new split container *around* some containers, that is, detaching and
 * attaching them in order without wanting to mess with the focus in between).
 *
 */
void con_attach(Con *con, Con *parent, bool ignore_focus);

/**
 * Detaches the given container from its current parent
 *
 */
void con_detach(Con *con);

/**
 * Updates the percent attribute of the children of the given container. This
 * function needs to be called when a window is added or removed from a
 * container.
 *
 */
void con_fix_percent(Con *con);

/**
 * Toggles fullscreen mode for the given container. Fullscreen mode will not be
 * entered when there already is a fullscreen container on this workspace.
 *
 */
void con_toggle_fullscreen(Con *con, int fullscreen_mode);

/**
 * Enables fullscreen mode for the given container, if necessary.
 *
 */
void con_enable_fullscreen(Con *con, fullscreen_mode_t fullscreen_mode);

/**
 * Disables fullscreen mode for the given container, if necessary.
 *
 */
void con_disable_fullscreen(Con *con);

/**
 * Moves the given container to the currently focused container on the given
 * workspace.
 *
 * The fix_coordinates flag will translate the current coordinates (offset from
 * the monitor position basically) to appropriate coordinates on the
 * destination workspace.
 * Not enabling this behaviour comes in handy when this function gets called by
 * floating_maybe_reassign_ws, which will only "move" a floating window when it
 * *already* changed its coordinates to a different output.
 *
 * The dont_warp flag disables pointer warping and will be set when this
 * function is called while dragging a floating window.
 *
 * If ignore_focus is set, the container will be moved without modifying focus
 * at all.
 *
 * TODO: is there a better place for this function?
 *
 */
void con_move_to_workspace(Con *con, Con *workspace, bool fix_coordinates,
                           bool dont_warp, bool ignore_focus);

/**
 * Moves the given container to the currently focused container on the
 * visible workspace on the given output.
 *
 */
void con_move_to_output(Con *con, Output *output, bool fix_coordinates);

/**
 * Moves the given container to the currently focused container on the
 * visible workspace on the output specified by the given name.
 * The current output for the container is used to resolve relative names
 * such as left, right, up, down.
 *
 */
bool con_move_to_output_name(Con *con, const char *name, bool fix_coordinates);

bool con_move_to_target(Con *con, Con *target);
/**
 * Moves the given container to the given mark.
 *
 */
bool con_move_to_mark(Con *con, const char *mark);

/**
 * Returns the orientation of the given container (for stacked containers,
 * vertical orientation is used regardless of the actual orientation of the
 * container).
 *
 */
orientation_t con_orientation(Con *con);

/**
 * Returns the container which will be focused next when the given container
 * is not available anymore. Called in tree_close_internal and con_move_to_workspace
 * to properly restore focus.
 *
 */
Con *con_next_focused(Con *con);

/**
 * Returns the focused con inside this client, descending the tree as far as
 * possible. This comes in handy when attaching a con to a workspace at the
 * currently focused position, for example.
 *
 */
Con *con_descend_focused(Con *con);

/**
 * Returns the focused con inside this client, descending the tree as far as
 * possible. This comes in handy when attaching a con to a workspace at the
 * currently focused position, for example.
 *
 * Works like con_descend_focused but considers only tiling cons.
 *
 */
Con *con_descend_tiling_focused(Con *con);

/**
 * Returns the leftmost, rightmost, etc. container in sub-tree. For example, if
 * direction is D_LEFT, then we return the rightmost container and if direction
 * is D_RIGHT, we return the leftmost container.  This is because if we are
 * moving D_LEFT, and thus want the rightmost container.
 */
Con *con_descend_direction(Con *con, direction_t direction);

/**
 * Returns whether the window decoration (title bar) should be drawn into the
 * X11 frame window of this container (default) or into the X11 frame window of
 * the parent container (for stacked/tabbed containers).
 *
 */
bool con_draw_decoration_into_frame(Con *con);

/**
 * Returns a "relative" Rect which contains the amount of pixels that need to
 * be added to the original Rect to get the final position (obviously the
 * amount of pixels for normal, 1pixel and borderless are different).
 *
 */
Rect con_border_style_rect(Con *con);

/**
 * Returns adjacent borders of the window. We need this if hide_edge_borders is
 * enabled.
 */
adjacent_t con_adjacent_borders(Con *con);

/**
 * Use this function to get a container’s border style. This is important
 * because when inside a stack, the border style is always BS_NORMAL.
 * For tabbed mode, the same applies, with one exception: when the container is
 * borderless and the only element in the tabbed container, the border is not
 * rendered.
 *
 * For children of a CT_DOCKAREA, the border style is always none.
 *
 */
int con_border_style(Con *con);

/**
 * Sets the given border style on con, correctly keeping the position/size of a
 * floating window.
 *
 */
void con_set_border_style(Con *con, border_style_t border_style, int border_width);

/**
 * This function changes the layout of a given container. Use it to handle
 * special cases like changing a whole workspace to stacked/tabbed (creates a
 * new split container before).
 *
 */
void con_set_layout(Con *con, layout_t layout);

/**
 * This function toggles the layout of a given container. toggle_mode can be
 * either 'default' (toggle only between stacked/tabbed/last_split_layout),
 * 'split' (toggle only between splitv/splith) or 'all' (toggle between all
 * layouts).
 *
 */
void con_toggle_layout(Con *con, const char *toggle_mode);

/**
 * Determines the minimum size of the given con by looking at its children (for
 * split/stacked/tabbed cons). Will be called when resizing floating cons
 *
 */
Rect con_minimum_size(Con *con);

/**
 * Returns true if changing the focus to con would be allowed considering
 * the fullscreen focus constraints. Specifically, if a fullscreen container or
 * any of its descendants is focused, this function returns true if and only if
 * focusing con would mean that focus would still be visible on screen, i.e.,
 * the newly focused container would not be obscured by a fullscreen container.
 *
 * In the simplest case, if a fullscreen container or any of its descendants is
 * fullscreen, this functions returns true if con is the fullscreen container
 * itself or any of its descendants, as this means focus wouldn't escape the
 * boundaries of the fullscreen container.
 *
 * In case the fullscreen container is of type CF_OUTPUT, this function returns
 * true if con is on a different workspace, as focus wouldn't be obscured by
 * the fullscreen container that is constrained to a different workspace.
 *
 * Note that this same logic can be applied to moving containers. If a
 * container can be focused under the fullscreen focus constraints, it can also
 * become a parent or sibling to the currently focused container.
 *
 */
bool con_fullscreen_permits_focusing(Con *con);

/**
 * Checks if the given container has an urgent child.
 *
 */
bool con_has_urgent_child(Con *con);

/**
 * Make all parent containers urgent if con is urgent or clear the urgent flag
 * of all parent containers if there are no more urgent children left.
 *
 */
void con_update_parents_urgency(Con *con);

/**
 * Set urgency flag to the container, all the parent containers and the workspace.
 *
 */
void con_set_urgency(Con *con, bool urgent);

/**
 * Create a string representing the subtree under con.
 *
 */
char *con_get_tree_representation(Con *con);

/**
 * force parent split containers to be redrawn
 *
 */
void con_force_split_parents_redraw(Con *con);

/**
 * Returns the window title considering the current title format.
 *
 */
i3String *con_parse_title_format(Con *con);

/**
 * Swaps the two containers.
 *
 */
bool con_swap(Con *first, Con *second);

/**
 * Returns given container's rect size depending on its orientation.
 * i.e. its width when horizontal, its height when vertical.
 *
 */
uint32_t con_rect_size_in_orientation(Con *con);

/**
 * Merges container specific data that should move with the window (e.g. marks,
 * title format, and the window itself) into another container, and closes the
 * old container.
 *
 */
void con_merge_into(Con *old, Con *new);

/**
 * Returns true if the container is within any stacked/tabbed split container.
 *
 */
bool con_inside_stacked_or_tabbed(Con *con);

```

**data.h** - Estructuras de datos fundamentales:

**Archivo**: `i3/include/data.h`

```c
/*
 * vim:ts=4:sw=4:expandtab
 *
 * i3 - an improved tiling window manager
 * © 2009 Michael Stapelberg and contributors (see also: LICENSE)
 *
 * include/data.h: This file defines all data structures used by i3
 *
 */
#pragma once

#define PCRE2_CODE_UNIT_WIDTH 8

#define SN_API_NOT_YET_FROZEN 1
#include <libsn/sn-launcher.h>

#include <xcb/randr.h>
#include <pcre2.h>
#include <sys/time.h>
#include <cairo/cairo.h>

#include "queue.h"

/*
 * To get the big concept: There are helper structures like struct
 * Workspace_Assignment. Every struct which is also defined as type (see
 * forward definitions) is considered to be a major structure, thus important.
 *
 * The following things are all stored in a 'Con', from very high level (the
 * biggest Cons) to very small (a single window):
 *
 * 1) X11 root window (as big as all your outputs combined)
 * 2) output (like LVDS1)
 * 3) content container, dockarea containers
 * 4) workspaces
 * 5) split containers
 * ... (you can arbitrarily nest split containers)
 * 6) X11 window containers
 *
 */

/* Forward definitions */
typedef struct Binding Binding;
typedef struct Rect Rect;
typedef struct xoutput Output;
typedef struct Con Con;
typedef struct Match Match;
typedef struct Assignment Assignment;
typedef struct Window i3Window;
typedef struct gaps_t gaps_t;
typedef struct mark_t mark_t;

/******************************************************************************
 * Helper types
 *****************************************************************************/
typedef enum { D_LEFT,
               D_RIGHT,
               D_UP,
               D_DOWN } direction_t;
typedef enum { NO_ORIENTATION = 0,
               HORIZ,
               VERT } orientation_t;
typedef enum { BEFORE,
               AFTER } position_t;
typedef enum {
    BS_NONE = 0,
    BS_PIXEL = 1,
    BS_NORMAL = 2,
} border_style_t;

/** parameter to specify whether tree_close_internal() and x_window_kill() should kill
 * only this specific window or the whole X11 client */
typedef enum { DONT_KILL_WINDOW = 0,
               KILL_WINDOW = 1,
               KILL_CLIENT = 2 } kill_window_t;

/** describes if the window is adjacent to the output (physical screen) edges. */
typedef enum { ADJ_NONE = 0,
               ADJ_LEFT_SCREEN_EDGE = (1 << 0),
               ADJ_RIGHT_SCREEN_EDGE = (1 << 1),
               ADJ_UPPER_SCREEN_EDGE = (1 << 2),
               ADJ_LOWER_SCREEN_EDGE = (1 << 4) } adjacent_t;

typedef enum { SMART_GAPS_OFF,
               SMART_GAPS_ON,
               SMART_GAPS_INVERSE_OUTER } smart_gaps_t;

typedef enum { HEBM_NONE = ADJ_NONE,
               HEBM_VERTICAL = ADJ_LEFT_SCREEN_EDGE | ADJ_RIGHT_SCREEN_EDGE,
               HEBM_HORIZONTAL = ADJ_UPPER_SCREEN_EDGE | ADJ_LOWER_SCREEN_EDGE,
               HEBM_BOTH = HEBM_VERTICAL | HEBM_HORIZONTAL,
               HEBM_SMART = (1 << 5),
               HEBM_SMART_NO_GAPS = (1 << 6) } hide_edge_borders_mode_t;

typedef enum { MM_REPLACE,
               MM_ADD } mark_mode_t;

/**
 * Container layouts. See Con::layout.
 */
typedef enum {
    L_DEFAULT = 0,
    L_STACKED = 1,
    L_TABBED = 2,
    L_DOCKAREA = 3,
    L_OUTPUT = 4,
    L_SPLITV = 5,
    L_SPLITH = 6
} layout_t;

/**
 * Binding input types. See Binding::input_type.
 */
typedef enum {
    B_KEYBOARD = 0,
    B_MOUSE = 1
} input_type_t;

/**
 * Bitmask for matching XCB_XKB_GROUP_1 to XCB_XKB_GROUP_4.
 */
typedef enum {
    I3_XKB_GROUP_MASK_ANY = 0,
    I3_XKB_GROUP_MASK_1 = (1 << 0),
    I3_XKB_GROUP_MASK_2 = (1 << 1),
    I3_XKB_GROUP_MASK_3 = (1 << 2),
    I3_XKB_GROUP_MASK_4 = (1 << 3)
} i3_xkb_group_mask_t;

/**
 * The lower 16 bits contain a xcb_key_but_mask_t, the higher 16 bits contain
 * an i3_xkb_group_mask_t. This type is necessary for the fallback logic to
 * work when handling XKB groups (see ticket #1775) and makes the code which
 * locates keybindings upon KeyPress/KeyRelease events simpler.
 */
typedef uint32_t i3_event_state_mask_t;

/**
 * Mouse pointer warping modes.
 */
typedef enum {
    POINTER_WARPING_OUTPUT = 0,
    POINTER_WARPING_NONE = 1
} warping_t;

struct gaps_t {
    int inner;
    int top;
    int right;
    int bottom;
    int left;
};

typedef enum {
    GAPS_INNER = (1 << 0),
    GAPS_TOP = (1 << 1),
    GAPS_RIGHT = (1 << 2),
    GAPS_BOTTOM = (1 << 3),
    GAPS_LEFT = (1 << 4),
    GAPS_VERTICAL = (GAPS_TOP | GAPS_BOTTOM),
    GAPS_HORIZONTAL = (GAPS_RIGHT | GAPS_LEFT),
    GAPS_OUTER = (GAPS_VERTICAL | GAPS_HORIZONTAL),
} gaps_mask_t;

/**
 * Focus wrapping modes.
 */
typedef enum {
    FOCUS_WRAPPING_OFF = 0,
    FOCUS_WRAPPING_ON = 1,
    FOCUS_WRAPPING_FORCE = 2,
    FOCUS_WRAPPING_WORKSPACE = 3
} focus_wrapping_t;

/**
 * Stores a rectangle, for example the size of a window, the child window etc.
 *
 * Note that x and y can contain signed values in some cases (for example when
 * used for the coordinates of a window, which can be set outside of the
 * visible area, but not when specifying the position of a workspace for the
 * _NET_WM_WORKAREA hint). Not declaring x/y as int32_t saves us a lot of
 * typecasts.
 *
 */
struct Rect {
    uint32_t x;
    uint32_t y;
    uint32_t width;
    uint32_t height;
};

/**
 * Stores the reserved pixels on each screen edge read from a
 * _NET_WM_STRUT_PARTIAL.
 *
 */
struct reservedpx {
    uint32_t left;
    uint32_t right;
    uint32_t top;
    uint32_t bottom;
};

/**
 * Stores a width/height pair, used as part of deco_render_params to check
 * whether the rects width/height have changed.
 *
 */
struct width_height {
    uint32_t w;
    uint32_t h;
};

/**
 * Stores the parameters for rendering a window decoration. This structure is
 * cached in every Con and no re-rendering will be done if the parameters have
 * not changed (only the pixmaps will be copied).
 *
 */
struct deco_render_params {
    struct Colortriple *color;
    int border_style;
    struct width_height con_rect;
    struct width_height con_window_rect;
    Rect con_deco_rect;
    color_t background;
    layout_t parent_layout;
    bool con_is_leaf;
};

/**
 * Stores which workspace (by name or number) goes to which output and its gaps config.
 *
 */
struct Workspace_Assignment {
    char *name;
    char *output;
    gaps_t gaps;
    gaps_mask_t gaps_mask;

    TAILQ_ENTRY(Workspace_Assignment) ws_assignments;
};

struct Ignore_Event {
    int sequence;
    int response_type;
    time_t added;

    SLIST_ENTRY(Ignore_Event) ignore_events;
};

/**
 * Stores internal information about a startup sequence, like the workspace it
 * was initiated on.
 *
 */
struct Startup_Sequence {
    /** startup ID for this sequence, generated by libstartup-notification */
    char *id;
    /** workspace on which this startup was initiated */
    char *workspace;
    /** libstartup-notification context for this launch */
    SnLauncherContext *context;
    /** time at which this sequence should be deleted (after it was marked as
     * completed) */
    time_t delete_at;

    TAILQ_ENTRY(Startup_Sequence) sequences;
};

/**
 * Regular expression wrapper. It contains the pattern itself as a string (like
 * ^foo[0-9]$) as well as a pointer to the compiled PCRE expression and the
 * pcre_extra data returned by pcre_study().
 *
 * This makes it easier to have a useful logfile, including the matching or
 * non-matching pattern.
 *
 */
struct regex {
    char *pattern;
    pcre2_code *regex;
};

/**
 * Stores a resolved keycode (from a keysym), including the modifier mask. Will
 * be passed to xcb_grab_key().
 *
 */
struct Binding_Keycode {
    xcb_keycode_t keycode;
    i3_event_state_mask_t modifiers;
    TAILQ_ENTRY(Binding_Keycode) keycodes;
};

/******************************************************************************
 * Major types
 *****************************************************************************/

/**
 * Holds a keybinding, consisting of a keycode combined with modifiers and the
 * command which is executed as soon as the key is pressed (see
 * src/config_parser.c)
 *
 */
struct Binding {
    /* The type of input this binding is for. (Mouse bindings are not yet
     * implemented. All bindings are currently assumed to be keyboard bindings.) */
    input_type_t input_type;

    /** If true, the binding should be executed upon a KeyRelease event, not a
     * KeyPress (the default). */
    enum {
        /* This binding will only be executed upon KeyPress events */
        B_UPON_KEYPRESS = 0,
        /* This binding will be executed either upon a KeyRelease event, or… */
        B_UPON_KEYRELEASE = 1,
        /* …upon a KeyRelease event, even if the modifiers don’t match. This
         * state is triggered from get_binding() when the corresponding
         * KeyPress (!) happens, so that users can release the modifier keys
         * before releasing the actual key. */
        B_UPON_KEYRELEASE_IGNORE_MODS = 2,
    } release;

    /** If this is true for a mouse binding, the binding should be executed
     * when the button is pressed over the window border. */
    bool border;

    /** If this is true for a mouse binding, the binding should be executed
     * when the button is pressed over any part of the window, not just the
     * title bar (default). */
    bool whole_window;

    /** If this is true for a mouse binding, the binding should only be
     * executed if the button press was not on the titlebar. */
    bool exclude_titlebar;

    /** Keycode to bind */
    uint32_t keycode;

    /** Bitmask which is applied against event->state for KeyPress and
     * KeyRelease events to determine whether this binding applies to the
     * current state. */
    i3_event_state_mask_t event_state_mask;

    /** Symbol the user specified in configfile, if any. This needs to be
     * stored with the binding to be able to re-convert it into a keycode
     * if the keyboard mapping changes (using Xmodmap for example) */
    char *symbol;

    /** Only in use if symbol != NULL. Contains keycodes which generate the
     * specified symbol. Useful for unbinding and checking which binding was
     * used when a key press event comes in. */
    TAILQ_HEAD(keycodes_head, Binding_Keycode) keycodes_head;

    /** Command, like in command mode */
    char *command;

    TAILQ_ENTRY(Binding) bindings;
};

/**
 * Holds a command specified by either an:
 * - exec-line
 * - exec_always-line
 * in the config (see src/config.c)
 *
 */
struct Autostart {
    /** Command, like in command mode */
    char *command;
    /** no_startup_id flag for start_application(). Determines whether a
     * startup notification context/ID should be created. */
    bool no_startup_id;
    TAILQ_ENTRY(Autostart) autostarts;
    TAILQ_ENTRY(Autostart) autostarts_always;
};

struct output_name {
    char *name;
    SLIST_ENTRY(output_name) names;
};

/**
 * An Output is a physical output on your graphics driver. Outputs which
 * are currently in use have (output->active == true). Each output has a
 * position and a mode. An output usually corresponds to one connected
 * screen (except if you are running multiple screens in clone mode).
 *
 */
struct xoutput {
    /** Output id, so that we can requery the output directly later */
    xcb_randr_output_t id;

    /** Whether the output is currently active (has a CRTC attached with a
     * valid mode) */
    bool active;

    /** Internal flags, necessary for querying RandR screens (happens in
     * two stages) */
    bool changed;
    bool to_be_disabled;
    bool primary;

    /** List of names for the output.
     * An output always has at least one name; the first name is
     * considered the primary one. */
    SLIST_HEAD(names_head, output_name) names_head;

    /** Pointer to the Con which represents this output */
    Con *con;

    /** x, y, width, height */
    Rect rect;

    TAILQ_ENTRY(xoutput) outputs;
};

/**
 * A 'Window' is a type which contains an xcb_window_t and all the related
 * information (hints like _NET_WM_NAME for that window).
 *
 */
struct Window {
    xcb_window_t id;

    /** Holds the xcb_window_t (just an ID) for the leader window (logical
     * parent for toolwindows and similar floating windows) */
    xcb_window_t leader;
    xcb_window_t transient_for;

    /** Pointers to the Assignments which were already ran for this Window
     * (assignments run only once) */
    uint32_t nr_assignments;
    Assignment **ran_assignments;

    char *class_class;
    char *class_instance;

    /** The name of the window. */
    i3String *name;

    /** The WM_WINDOW_ROLE of this window (for example, the pidgin buddy window
     * sets "buddy list"). Useful to match specific windows in assignments or
     * for_window. */
    char *role;

    /** WM_CLIENT_MACHINE of the window */
    char *machine;

    /** Flag to force re-rendering the decoration upon changes */
    bool name_x_changed;

    /** Whether the application used _NET_WM_NAME */
    bool uses_net_wm_name;

    /** Whether the application needs to receive WM_TAKE_FOCUS */
    bool needs_take_focus;

    /** Whether this window accepts focus. We store this inverted so that the
     * default will be 'accepts focus'. */
    bool doesnt_accept_focus;

    /** The _NET_WM_WINDOW_TYPE for this window. */
    xcb_atom_t window_type;

    /** The _NET_WM_DESKTOP for this window. */
    uint32_t wm_desktop;

    /** Whether the window says it is a dock window */
    enum { W_NODOCK = 0,
           W_DOCK_TOP = 1,
           W_DOCK_BOTTOM = 2 } dock;

    /** When this window was marked urgent. 0 means not urgent */
    struct timeval urgent;

    /** Pixels the window reserves. left/right/top/bottom */
    struct reservedpx reserved;

    /** Depth of the window */
    uint16_t depth;

    /* the wanted size of the window, used in combination with size
     * increments (see below). */
    int base_width;
    int base_height;

    /* minimum increment size specified for the window (in pixels) */
    int width_increment;
    int height_increment;

    /* Minimum size specified for the window. */
    int min_width;
    int min_height;

    /* Maximum size specified for the window. */
    int max_width;
    int max_height;

    /* aspect ratio from WM_NORMAL_HINTS (MPlayer uses this for example) */
    double min_aspect_ratio;
    double max_aspect_ratio;

    /** Window icon, as Cairo surface */
    cairo_surface_t *icon;

    /** The window has a nonrectangular shape. */
    bool shaped;
    /** The window has a nonrectangular input shape. */
    bool input_shaped;

    /* Time when the window became managed. Used to determine whether a window
     * should be swallowed after initial management. */
    time_t managed_since;

    /* The window has been swallowed. */
    bool swallowed;
};

/**
 * A "match" is a data structure which acts like a mask or expression to match
 * certain windows or not. For example, when using commands, you can specify a
 * command like this: [title="*Firefox*"] kill. The title member of the match
 * data structure will then be filled and i3 will check each window using
 * match_matches_window() to find the windows affected by this command.
 *
 */
struct Match {
    /* Set if a criterion was specified incorrectly. */
    char *error;

    struct regex *title;
    struct regex *application;
    struct regex *class;
    struct regex *instance;
    struct regex *mark;
    struct regex *window_role;
    struct regex *workspace;
    struct regex *machine;
    xcb_atom_t window_type;
    enum {
        U_DONTCHECK = -1,
        U_LATEST = 0,
        U_OLDEST = 1
    } urgent;
    enum {
        M_DONTCHECK = -1,
        M_NODOCK = 0,
        M_DOCK_ANY = 1,
        M_DOCK_TOP = 2,
        M_DOCK_BOTTOM = 3
    } dock;
    xcb_window_t id;
    enum { WM_ANY = 0,
           WM_TILING_AUTO,
           WM_TILING_USER,
           WM_TILING,
           WM_FLOATING_AUTO,
           WM_FLOATING_USER,
           WM_FLOATING } window_mode;
    Con *con_id;
    bool match_all_windows;

    /* Where the window looking for a match should be inserted:
     *
     * M_HERE   = the matched container will be replaced by the window
     *            (layout saving)
     * M_ASSIGN_WS = the matched container will be inserted in the target_ws.
     * M_BELOW  = the window will be inserted as a child of the matched container
     *            (dockareas)
     *
     */
    enum { M_HERE = 0,
           M_ASSIGN_WS,
           M_BELOW } insert_where;

    TAILQ_ENTRY(Match) matches;

    /* Whether this match was generated when restarting i3 inplace.
     * Leads to not setting focus when managing a new window, because the old
     * focus stack should be restored. */
    bool restart_mode;
};

/**
 * An Assignment makes specific windows go to a specific workspace/output or
 * run a command for that window. With this mechanism, the user can -- for
 * example -- assign their browser to workspace "www". Checking if a window is
 * assigned works by comparing the Match data structure with the window (see
 * match_matches_window()).
 *
 */
struct Assignment {
    /** type of this assignment:
     *
     * A_COMMAND = run the specified command for the matching window
     * A_TO_WORKSPACE = assign the matching window to the specified workspace
     * A_NO_FOCUS = don't focus matched window when it is managed
     *
     * While the type is a bitmask, only one value can be set at a time. It is
     * a bitmask to allow filtering for multiple types, for example in the
     * assignment_for() function.
     *
     */
    enum {
        A_ANY = 0,
        A_COMMAND = (1 << 0),
        A_TO_WORKSPACE = (1 << 1),
        A_NO_FOCUS = (1 << 2),
        A_TO_WORKSPACE_NUMBER = (1 << 3),
        A_TO_OUTPUT = (1 << 4)
    } type;

    /** the criteria to check if a window matches */
    Match match;

    /** destination workspace/command/output, depending on the type */
    union {
        char *command;
        char *workspace;
        char *output;
    } dest;

    TAILQ_ENTRY(Assignment) assignments;
};

/** Fullscreen modes. Used by Con.fullscreen_mode. */
typedef enum { CF_NONE = 0,
               CF_OUTPUT = 1,
               CF_GLOBAL = 2 } fullscreen_mode_t;

struct mark_t {
    char *name;

    TAILQ_ENTRY(mark_t) marks;
};

/**
 * A 'Con' represents everything from the X11 root window down to a single X11 window.
 *
 */
struct Con {
    bool mapped;

    /* Should this container be marked urgent? This gets set when the window
     * inside this container (if any) sets the urgency hint, for example. */
    bool urgent;

    /** This counter contains the number of UnmapNotify events for this
     * container (or, more precisely, for its ->frame) which should be ignored.
     * UnmapNotify events need to be ignored when they are caused by i3 itself,
     * for example when reparenting or when unmapping the window on a workspace
     * change. */
    uint8_t ignore_unmap;

    /* The surface used for the frame window. */
    surface_t frame;
    surface_t frame_buffer;
    bool pixmap_recreated;

    enum {
        CT_ROOT = 0,
        CT_OUTPUT = 1,
        CT_CON = 2,
        CT_FLOATING_CON = 3,
        CT_WORKSPACE = 4,
        CT_DOCKAREA = 5
    } type;

    /** the workspace number, if this Con is of type CT_WORKSPACE and the
     * workspace is not a named workspace (for named workspaces, num == -1) */
    int num;

    /** Only applicable for containers of type CT_WORKSPACE. */
    gaps_t gaps;

    struct Con *parent;

    /* The position and size for this con. These coordinates are absolute. Note
     * that the rect of a container does not include the decoration. */
    struct Rect rect;
    /* The position and size of the actual client window. These coordinates are
     * relative to the container's rect. */
    struct Rect window_rect;
    /* The position and size of the container's decoration. These coordinates
     * are relative to the container's parent's rect. */
    struct Rect deco_rect;
    /** the geometry this window requested when getting mapped */
    struct Rect geometry;

    char *name;

    /** The format with which the window's name should be displayed. */
    char *title_format;

    /** Whether the window icon should be displayed, and with what padding. -1
     * means display no window icon (default behavior), 0 means display without
     * any padding, 1 means display with 1 pixel of padding and so on. */
    int window_icon_padding;

    /* a sticky-group is an identifier which bundles several containers to a
     * group. The contents are shared between all of them, that is they are
     * displayed on whichever of the containers is currently visible */
    char *sticky_group;

    /* user-definable marks to jump to this container later */
    TAILQ_HEAD(marks_head, mark_t) marks_head;
    /* cached to decide whether a redraw is needed */
    bool mark_changed;

    double percent;

    /* the x11 border pixel attribute */
    int border_width;
    int current_border_width;

    struct Window *window;

    /* timer used for disabling urgency */
    struct ev_timer *urgency_timer;

    /** Cache for the decoration rendering */
    struct deco_render_params *deco_render_params;

    /* Only workspace-containers can have floating clients */
    TAILQ_HEAD(floating_head, Con) floating_head;

    TAILQ_HEAD(nodes_head, Con) nodes_head;
    TAILQ_HEAD(focus_head, Con) focus_head;

    TAILQ_HEAD(swallow_head, Match) swallow_head;

    fullscreen_mode_t fullscreen_mode;

    /* Whether this window should stick to the glass. This corresponds to
     * the _NET_WM_STATE_STICKY atom and will only be respected if the
     * window is floating. */
    bool sticky;

    /* layout is the layout of this container: one of split[v|h], stacked or
     * tabbed. Special containers in the tree (above workspaces) have special
     * layouts like dockarea or output.
     *
     * last_split_layout is one of splitv or splith to support the old "layout
     * default" command which by now should be "layout splitv" or "layout
     * splith" explicitly.
     *
     * workspace_layout is only for type == CT_WORKSPACE cons. When you change
     * the layout of a workspace without any children, i3 cannot just set the
     * layout (because workspaces need to be splitv/splith to allow focus
     * parent and opening new containers). Instead, it stores the requested
     * layout in workspace_layout and creates a new split container with that
     * layout whenever a new container is attached to the workspace. */
    layout_t layout, last_split_layout, workspace_layout;

    border_style_t border_style;
    /* When the border style of a con changes because of motif hints, we don't
     * want to set more decoration that the user wants. The user's preference is determined by these:
     * 1. For new tiling windows, as set by `default_border`
     * 2. For new floating windows, as set by `default_floating_border`
     * 3. For all windows that the user runs the `border` command, whatever is
     * the result of that command for that window. */
    border_style_t max_user_border_style;

    /** floating? (= not in tiling layout) This cannot be simply a bool
     * because we want to keep track of whether the status was set by the
     * application (by setting _NET_WM_WINDOW_TYPE appropriately) or by the
     * user. The user’s choice overwrites automatic mode, of course. The
     * order of the values is important because we check with >=
     * FLOATING_AUTO_ON if a client is floating. */
    enum {
        FLOATING_AUTO_OFF = 0,
        FLOATING_USER_OFF = 1,
        FLOATING_AUTO_ON = 2,
        FLOATING_USER_ON = 3
    } floating;

    TAILQ_ENTRY(Con) nodes;
    TAILQ_ENTRY(Con) focused;
    TAILQ_ENTRY(Con) all_cons;
    TAILQ_ENTRY(Con) floating_windows;

    /** callbacks */
    void (*on_remove_child)(Con *);

    enum {
        /* Not a scratchpad window. */
        SCRATCHPAD_NONE = 0,

        /* Just moved to scratchpad, not resized by the user yet.
         * Window will be auto-centered and sized appropriately. */
        SCRATCHPAD_FRESH = 1,

        /* The user changed position/size of the scratchpad window. */
        SCRATCHPAD_CHANGED = 2
    } scratchpad_state;

    /* The ID of this container before restarting. Necessary to correctly
     * interpret back-references in the JSON (such as the focus stack). */
    int old_id;

    /* Depth of the container window */
    uint16_t depth;

    /* The colormap for this con if a custom one is used. */
    xcb_colormap_t colormap;
};

```


### 2.3 Tipos de Layout en i3

**Enumeración de layouts**:

```
i3/include/commands.h:void cmd_layout_toggle(I3_CMD, const char *toggle_mode);
i3/include/con.h:void con_set_layout(Con *con, layout_t layout);
i3/include/configuration.h:    layout_t default_layout;
i3/include/data.h:} layout_t;
i3/include/data.h:    layout_t parent_layout;
i3/include/data.h:    layout_t layout, last_split_layout, workspace_layout;
i3/include/util.h: * Set 'out' to the layout_t value for the given layout. The function
i3/include/util.h:bool layout_from_name(const char *layout_str, layout_t *out);
```


### 2.4 Algoritmo de Renderizado (Render)

**render.c** - Cómo i3 calcula posiciones y tamaños

Función clave: `render_con()`

```c
void render_con(Con *con) {
    render_params params = {
        .rect = con->rect,
        .x = con->rect.x,
        .y = con->rect.y,
        .children = con_num_children(con)};

    DLOG("Rendering node %p / %s / layout %d / children %d\n", con, con->name,
         con->layout, params.children);

    if (con->type == CT_WORKSPACE) {
        gaps_t gaps = calculate_effective_gaps(con);
        Rect inset = (Rect){
            gaps.left,
            gaps.top,
            -(gaps.left + gaps.right),
            -(gaps.top + gaps.bottom),
        };
        con->rect = rect_add(con->rect, inset);
        params.rect = rect_add(params.rect, inset);
        params.x += gaps.left;
        params.y += gaps.top;
    }

    if (gaps_should_inset_con(con, params.children)) {
        gaps_t gaps = calculate_effective_gaps(con);
        Rect inset = (Rect){
            gaps_has_adjacent_container(con, D_LEFT) ? gaps.inner / 2 : gaps.inner,
            gaps_has_adjacent_container(con, D_UP) ? gaps.inner / 2 : gaps.inner,
            gaps_has_adjacent_container(con, D_RIGHT) ? -(gaps.inner / 2) : -gaps.inner,
            gaps_has_adjacent_container(con, D_DOWN) ? -(gaps.inner / 2) : -gaps.inner,
        };
        inset.width -= inset.x;
        inset.height -= inset.y;

        if (con->fullscreen_mode == CF_NONE) {
            params.rect = rect_add(params.rect, inset);
            con->rect = rect_add(con->rect, inset);
        }
        inset.height = 0;

        params.x = con->rect.x;
        params.y = con->rect.y;
    }

    con->mapped = true;

    /* if this container contains a window, set the coordinates */
    if (con->window) {
        /* depending on the border style, the rect of the child window
         * needs to be smaller */
```


### 2.5 Gestión de Focus

**con.c** - Funciones de focus

```c

add_to_focus_head:
    /* We insert to the TAIL because con_focus() will correct this.
     * This way, we have the option to insert Cons without having
     * to focus them. */
    TAILQ_INSERT_TAIL(focus_head, con, focused);
    con_force_split_parents_redraw(con);
}

/*
 * Attaches the given container to the given parent. This happens when moving
 * a container or when inserting a new container at a specific place in the
 * tree.
 *
 * ignore_focus is to just insert the Con at the end (useful when creating a
 * new split container *around* some containers, that is, detaching and
 * attaching them in order without wanting to mess with the focus in between).
 *
 */
void con_attach(Con *con, Con *parent, bool ignore_focus) {
    _con_attach(con, parent, NULL, ignore_focus);
}

--
 *
 */
void con_focus(Con *con) {
    assert(con != NULL);
    DLOG("con_focus = %p\n", con);

    /* 1: set focused-pointer to the new con */
    /* 2: exchange the position of the container in focus stack of the parent all the way up */
    TAILQ_REMOVE(&(con->parent->focus_head), con, focused);
    TAILQ_INSERT_HEAD(&(con->parent->focus_head), con, focused);
    if (con->parent->parent != NULL) {
        con_focus(con->parent);
    }

    focused = con;
    /* We can't blindly reset non-leaf containers since they might have
     * other urgent children. Therefore we only reset leafs and propagate
     * the changes upwards via con_update_parents_urgency() which does proper
     * checks before resetting the urgency.
     */
    if (con->urgent && con_is_leaf(con)) {
        con_set_urgency(con, false);
        con_update_parents_urgency(con);
        workspace_update_urgent_flag(con_get_workspace(con));
        ipc_send_window_event("urgent", con);
    }
}

/*
 * Raise container to the top if it is floating or inside some floating
 * container.
 *
--
 */
void con_activate(Con *con) {
    con_focus(con);
    con_raise(con);
}

/*
 * Activates the container like in con_activate but removes fullscreen
 * restrictions and properly warps the pointer if needed.
 *
 */
void con_activate_unblock(Con *con) {
    Con *ws = con_get_workspace(con);
    Con *previous_focus = focused;
    Con *fullscreen_on_ws = con_get_fullscreen_covering_ws(ws);

    if (fullscreen_on_ws && fullscreen_on_ws != con && !con_has_parent(con, fullscreen_on_ws)) {
        con_disable_fullscreen(fullscreen_on_ws);
    }

    con_activate(con);

    /* If the container is not on the current workspace, workspace_show() will
--
     *
     * Therefore, before calling workspace_show(), we make sure that 'con' will
     * be focused on the workspace. However, we cannot just con_focus(con)
     * because then the pointer will not be warped at all (the code thinks we
     * are already there).
     *
     * So we focus 'con' to make it the currently focused window of the target
     * workspace, then revert focus. */
    if (ws != con_get_workspace(previous_focus)) {
        con_activate(previous_focus);
        /* Now switch to the workspace, then focus */
        workspace_show(ws);
        con_activate(con);
    }
}

/*
 * Closes the given container.
 *
```


### 2.6 Tree Operations

**tree.c** - Operaciones sobre el árbol

```c
 *
 */
bool tree_close_internal(Con *con, kill_window_t kill_window, bool dont_kill_parent) {
    Con *parent = con->parent;

    /* remove the urgency hint of the workspace (if set) */
    if (con->urgent) {
        con_set_urgency(con, false);
        con_update_parents_urgency(con);
        workspace_update_urgent_flag(con_get_workspace(con));
    }

    DLOG("closing %p, kill_window = %d\n", con, kill_window);
    bool abort_kill = false;
    /* We cannot use TAILQ_FOREACH because the children get deleted
     * in their parent’s nodes_head */
    for (Con *child = TAILQ_FIRST(&(con->nodes_head)); child;) {
        Con *next_child = TAILQ_NEXT(child, nodes);
        DLOG("killing child=%p\n", child);
        if (!tree_close_internal(child, kill_window, true)) {
            abort_kill = true;
        }
        child = next_child;
    }

    if (abort_kill) {
        DLOG("One of the children could not be killed immediately (WM_DELETE sent), aborting.\n");
        return false;
    }

    if (con->window != NULL) {
        if (kill_window != DONT_KILL_WINDOW) {
            x_window_kill(con->window->id, kill_window);
            return false;
        }
--
     *
     * Rendering has to be avoided when dont_kill_parent is set (when
     * tree_close_internal calls itself recursively) because the tree is in a
     * non-renderable state during that time. */
    if (!dont_kill_parent) {
        tree_render();
    }

    /* kill the X11 part of this container */
    x_con_kill(con);

    if (ws == con) {
        DLOG("Closing workspace container %s, updating EWMH atoms\n", ws->name);
        ewmh_update_desktop_properties();
    }

    con_free(con);

--
 *
 */
void tree_split(Con *con, orientation_t orientation) {
    if (con_is_floating(con)) {
        DLOG("Floating containers can't be split.\n");
        return;
    }

    if (con->type == CT_WORKSPACE) {
        if (con_num_children(con) < 2) {
            if (con_num_children(con) == 0) {
                DLOG("Changing workspace_layout to L_DEFAULT\n");
                con->workspace_layout = L_DEFAULT;
            }
            DLOG("Changing orientation of workspace\n");
            con->layout = (orientation == HORIZ) ? L_SPLITH : L_SPLITV;
            return;
        } else {
--

/*
 * tree_flatten() removes pairs of redundant split containers, e.g.:
 *       [workspace, horizontal]
 *   [v-split]           [child3]
 *   [h-split]
 * [child1] [child2]
 * In this example, the v-split and h-split container are redundant.
 * Such a situation can be created by moving containers in a direction which is
 * not the orientation of their parent container. i3 needs to create a new
 * split container then and if you move containers this way multiple times,
 * redundant chains of split-containers can be the result.
 *
 */
void tree_flatten(Con *con) {
    Con *current, *child, *parent = con->parent;
    DLOG("Checking if I can flatten con = %p / %s\n", con, con->name);

    /* We only consider normal containers without windows */
    if (con->type != CT_CON ||
        parent->layout == L_OUTPUT || /* con == "content" */
        con->window != NULL) {
        goto recurse;
    }

    /* Ensure it got only one child */
    child = TAILQ_FIRST(&(con->nodes_head));
    if (child == NULL || TAILQ_NEXT(child, nodes) != NULL) {
        goto recurse;
    }
--
    /* 4: close the redundant cons */
    DLOG("closing redundant cons\n");
    tree_close_internal(con, DONT_KILL_WINDOW, true);

    /* Well, we got to abort the recursion here because we destroyed the
     * container. However, if tree_flatten() is called sufficiently often,
     * there can’t be the situation of having two pairs of redundant containers
     * at once. Therefore, we can safely abort the recursion on this level
     * after flattening. */
    return;

recurse:
    /* We cannot use normal foreach here because tree_flatten might close the
     * current container. */
    current = TAILQ_FIRST(&(con->nodes_head));
    while (current != NULL) {
        Con *next = TAILQ_NEXT(current, nodes);
        tree_flatten(current);
        current = next;
    }

    current = TAILQ_FIRST(&(con->floating_head));
    while (current != NULL) {
        Con *next = TAILQ_NEXT(current, floating_windows);
        tree_flatten(current);
        current = next;
    }
}
```


## 3. Análisis de Sway (C + wlroots)


### 3.1 Estructura de Datos Principal

Sway usa una estructura similar a i3 pero adaptada para Wayland:

**Definición de sway_container**:

```
sway/include/sway/commands.h:struct sway_container;
sway/include/sway/commands.h:struct sway_container *container_find_resize_parent(struct sway_container *con,
sway/include/sway/input/seat.h:struct sway_container *seat_get_focused_container(struct sway_seat *seat);
sway/include/sway/input/seat.h:struct sway_container *seat_get_focus_inactive_tiling(struct sway_seat *seat,
sway/include/sway/input/seat.h:struct sway_container *seat_get_focus_inactive_view(struct sway_seat *seat,
sway/include/sway/input/seat.h:struct sway_container *seat_get_focus_inactive_floating(struct sway_seat *seat,
sway/include/sway/output.h:struct sway_container;
sway/include/sway/output.h:struct sway_container *output_find_container(struct sway_output *output,
sway/include/sway/tree/arrange.h:struct sway_container;
sway/include/sway/tree/container.h:struct sway_container_state {
sway/include/sway/tree/container.h:struct sway_container {
sway/include/sway/tree/container.h:struct sway_container *container_create(struct sway_view *view);
sway/include/sway/tree/container.h:struct sway_container *container_find_child(struct sway_container *container,
sway/include/sway/tree/container.h:struct sway_container *container_obstructing_fullscreen_container(struct sway_container *container);
sway/include/sway/tree/container.h:struct sway_container *container_flatten(struct sway_container *container);
sway/include/sway/tree/container.h:struct sway_container *container_toplevel_ancestor(
sway/include/sway/tree/container.h:struct sway_container *container_split(struct sway_container *child,
sway/include/sway/tree/container.h:struct sway_container *container_find_mark(char *mark);
sway/include/sway/tree/node.h:struct sway_container;
sway/include/sway/tree/root.h:struct sway_container *root_find_container(
sway/include/sway/tree/view.h:struct sway_container;
sway/include/sway/tree/workspace.h:struct sway_container *workspace_find_container(struct sway_workspace *ws,
sway/include/sway/tree/workspace.h:struct sway_container *workspace_wrap_children(struct sway_workspace *ws);
sway/include/sway/tree/workspace.h:struct sway_container *workspace_add_tiling(struct sway_workspace *workspace,
sway/include/sway/tree/workspace.h:struct sway_container *workspace_insert_tiling(struct sway_workspace *workspace,
sway/include/sway/tree/workspace.h:struct sway_container *workspace_split(struct sway_workspace *workspace,
```


### 3.2 Headers Clave de Sway

**container.h** - Contenedores en Sway:

**Archivo**: `sway/include/sway/tree/container.h`

```c
#ifndef _SWAY_CONTAINER_H
#define _SWAY_CONTAINER_H
#include <stdint.h>
#include <sys/types.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_scene.h>
#include "list.h"
#include "sway/tree/node.h"

struct sway_view;
struct sway_seat;

enum sway_container_layout {
	L_NONE,
	L_HORIZ,
	L_VERT,
	L_STACKED,
	L_TABBED,
};

enum sway_container_border {
	B_NONE,
	B_PIXEL,
	B_NORMAL,
	B_CSD,
};

enum sway_fullscreen_mode {
	FULLSCREEN_NONE,
	FULLSCREEN_WORKSPACE,
	FULLSCREEN_GLOBAL,
};

struct sway_root;
struct sway_output;
struct sway_workspace;
struct sway_view;

enum wlr_direction;

struct sway_container_state {
	// Container properties
	enum sway_container_layout layout;
	double x, y;
	double width, height;

	enum sway_fullscreen_mode fullscreen_mode;

	struct sway_workspace *workspace; // NULL when hidden in the scratchpad
	struct sway_container *parent;    // NULL if container in root of workspace
	list_t *children;                 // struct sway_container

	struct sway_container *focused_inactive_child;
	bool focused;

	enum sway_container_border border;
	int border_thickness;
	bool border_top;
	bool border_bottom;
	bool border_left;
	bool border_right;

	// These are in layout coordinates.
	double content_x, content_y;
	double content_width, content_height;
};

struct sway_container {
	struct sway_node node;
	struct sway_view *view;

	struct wlr_scene_tree *scene_tree;

	struct {
		struct wlr_scene_tree *tree;

		struct wlr_scene_tree *border;
		struct wlr_scene_tree *background;

		struct sway_text_node *title_text;
		struct sway_text_node *marks_text;
	} title_bar;

	struct {
		struct wlr_scene_tree *tree;

		struct wlr_scene_rect *top;
		struct wlr_scene_rect *bottom;
		struct wlr_scene_rect *left;
		struct wlr_scene_rect *right;
	} border;

	struct wlr_scene_tree *content_tree;
	struct wlr_scene_buffer *output_handler;

	struct wl_listener output_enter;
	struct wl_listener output_leave;
	struct wl_listener output_handler_destroy;

	struct sway_container_state current;
	struct sway_container_state pending;

	char *title;           // The view's title (unformatted)
	char *formatted_title; // The title displayed in the title bar
	int title_width;

	char *title_format;

	enum sway_container_layout prev_split_layout;

	// Whether stickiness has been enabled on this container. Use
	// `container_is_sticky_[or_child]` rather than accessing this field
	// directly; it'll also check that the container is floating.
	bool is_sticky;

	// For C_ROOT, this has no meaning
	// For other types, this is the position in layout coordinates
	// Includes borders
	double saved_x, saved_y;
	double saved_width, saved_height;

	// Used when the view changes to CSD unexpectedly. This will be a non-B_CSD
	// border which we use to restore when the view returns to SSD.
	enum sway_container_border saved_border;

	// The share of the space of parent container this container occupies
	double width_fraction;
	double height_fraction;

	// The share of space of the parent container that all children occupy
	// Used for doing the resize calculations
	double child_total_width;
	double child_total_height;

	// Indicates that the container is a scratchpad container.
	// Both hidden and visible scratchpad containers have scratchpad=true.
	// Hidden scratchpad containers have a NULL parent.
	bool scratchpad;

	// Stores last output size and position for adjusting coordinates of
	// scratchpad windows.
	// Unused for non-scratchpad windows.
	struct wlr_box transform;

	float alpha;

	list_t *marks; // char *

	struct {
		struct wl_signal destroy;
	} events;
};

struct sway_container *container_create(struct sway_view *view);

void container_destroy(struct sway_container *con);

void container_begin_destroy(struct sway_container *con);

/**
 * Search a container's descendants a container based on test criteria. Returns
 * the first container that passes the test.
 */
struct sway_container *container_find_child(struct sway_container *container,
		bool (*test)(struct sway_container *view, void *data), void *data);

void container_for_each_child(struct sway_container *container,
		void (*f)(struct sway_container *container, void *data), void *data);

/**
 * Returns the fullscreen container obstructing this container if it exists.
 */
struct sway_container *container_obstructing_fullscreen_container(struct sway_container *container);

/**
 * Returns true if the given container is an ancestor of this container.
 */
bool container_has_ancestor(struct sway_container *container,
		struct sway_container *ancestor);

void container_reap_empty(struct sway_container *con);

struct sway_container *container_flatten(struct sway_container *container);

void container_update_title_bar(struct sway_container *container);

void container_update_marks(struct sway_container *container);

size_t parse_title_format(struct sway_container *container, char *buffer);

size_t container_build_representation(enum sway_container_layout layout,
		list_t *children, char *buffer);

void container_update_representation(struct sway_container *container);

/**
 * Return the height of a regular title bar.
 */
size_t container_titlebar_height(void);

void floating_calculate_constraints(int *min_width, int *max_width,
		int *min_height, int *max_height);

void floating_fix_coordinates(struct sway_container *con,
		struct wlr_box *old, struct wlr_box *new);

void container_floating_resize_and_center(struct sway_container *con);

void container_floating_set_default_size(struct sway_container *con);

void container_set_resizing(struct sway_container *con, bool resizing);

void container_set_floating(struct sway_container *container, bool enable);

void container_set_geometry_from_content(struct sway_container *con);

/**
 * Determine if the given container is itself floating.
 * This will return false for any descendants of a floating container.
 *
 * Uses pending container state.
 */
bool container_is_floating(struct sway_container *container);

/**
 * Get a container's box in layout coordinates.
 */
void container_get_box(struct sway_container *container, struct wlr_box *box);

/**
 * Move a floating container by the specified amount.
 */
void container_floating_translate(struct sway_container *con,
		double x_amount, double y_amount);

/**
 * Choose an output for the floating container's new position.
 */
struct sway_output *container_floating_find_output(struct sway_container *con);

/**
 * Move a floating container to a new layout-local position.
 */
void container_floating_move_to(struct sway_container *con,
		double lx, double ly);

/**
 * Move a floating container to the center of the workspace.
 */
void container_floating_move_to_center(struct sway_container *con);

bool container_has_urgent_child(struct sway_container *container);

/**
 * If the container is involved in a drag or resize operation via a mouse, this
 * ends the operation.
 */
void container_end_mouse_operation(struct sway_container *container);

void container_set_fullscreen(struct sway_container *con,
		enum sway_fullscreen_mode mode);

/**
 * Convenience function.
 */
void container_fullscreen_disable(struct sway_container *con);

/**
 * Walk up the container tree branch starting at the given container, and return
 * its earliest ancestor.
 */
struct sway_container *container_toplevel_ancestor(
		struct sway_container *container);

/**
 * Return true if the container is floating, or a child of a floating split
 * container.
 */
bool container_is_floating_or_child(struct sway_container *container);

/**
 * Return true if the container is fullscreen, or a child of a fullscreen split
 * container.
 */
bool container_is_fullscreen_or_child(struct sway_container *container);

enum sway_container_layout container_parent_layout(struct sway_container *con);

list_t *container_get_siblings(struct sway_container *container);

int container_sibling_index(struct sway_container *child);

void container_handle_fullscreen_reparent(struct sway_container *con);

void container_add_child(struct sway_container *parent,
		struct sway_container *child);

void container_insert_child(struct sway_container *parent,
		struct sway_container *child, int i);

/**
 * Side should be 0 to add before, or 1 to add after.
 */
void container_add_sibling(struct sway_container *parent,
		struct sway_container *child, bool after);

void container_detach(struct sway_container *child);

void container_replace(struct sway_container *container,
		struct sway_container *replacement);

void container_swap(struct sway_container *con1, struct sway_container *con2);

struct sway_container *container_split(struct sway_container *child,
		enum sway_container_layout layout);

bool container_is_transient_for(struct sway_container *child,
		struct sway_container *ancestor);

/**
 * Find any container that has the given mark and return it.
 */
struct sway_container *container_find_mark(char *mark);

/**
 * Find any container that has the given mark and remove the mark from the
 * container. Returns true if it matched a container.
 */
bool container_find_and_unmark(char *mark);

/**
 * Remove all marks from the container.
 */
void container_clear_marks(struct sway_container *container);

bool container_has_mark(struct sway_container *container, char *mark);

void container_add_mark(struct sway_container *container, char *mark);

void container_raise_floating(struct sway_container *con);

bool container_is_scratchpad_hidden(struct sway_container *con);

bool container_is_scratchpad_hidden_or_child(struct sway_container *con);

bool container_is_sticky(struct sway_container *con);

bool container_is_sticky_or_child(struct sway_container *con);

/**
 * This will destroy pairs of redundant H/V splits
 * e.g. H[V[H[app app]] app] -> H[app app app]
 * The middle "V[H[" are eliminated by a call to container_squash
 * on the V[ con. It's grandchildren are added to its parent.
 *
 * This function is roughly equivalent to i3's tree_flatten here:
 * https://github.com/i3/i3/blob/1f0c628cde40cf87371481041b7197344e0417c6/src/tree.c#L651
 *
 * Returns the number of new containers added to the parent
 */
int container_squash(struct sway_container *con);

void container_arrange_title_bar(struct sway_container *con);

void container_update(struct sway_container *con);

void container_update_itself_and_parents(struct sway_container *con);

#endif

```

**view.h** - Vistas (ventanas):

**Archivo**: `sway/include/sway/tree/view.h`

```c
#ifndef _SWAY_VIEW_H
#define _SWAY_VIEW_H
#include <wayland-server-core.h>
#include <wlr/config.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_tearing_control_v1.h>
#include "sway/config.h"
#if WLR_HAS_XWAYLAND
#include <wlr/xwayland.h>
#endif
#include "sway/input/input-manager.h"
#include "sway/input/seat.h"

struct sway_container;
struct sway_xdg_decoration;

enum sway_view_type {
	SWAY_VIEW_XDG_SHELL,
#if WLR_HAS_XWAYLAND
	SWAY_VIEW_XWAYLAND,
#endif
};

enum sway_view_prop {
	VIEW_PROP_TITLE,
	VIEW_PROP_APP_ID,
	VIEW_PROP_TAG,
	VIEW_PROP_CLASS,
	VIEW_PROP_INSTANCE,
	VIEW_PROP_WINDOW_TYPE,
	VIEW_PROP_WINDOW_ROLE,
#if WLR_HAS_XWAYLAND
	VIEW_PROP_X11_WINDOW_ID,
	VIEW_PROP_X11_PARENT_ID,
#endif
};

enum sway_view_tearing_mode {
	TEARING_OVERRIDE_FALSE,
	TEARING_OVERRIDE_TRUE,
	TEARING_WINDOW_HINT,
};

struct sway_view_impl {
	void (*get_constraints)(struct sway_view *view, double *min_width,
			double *max_width, double *min_height, double *max_height);
	const char *(*get_string_prop)(struct sway_view *view,
			enum sway_view_prop prop);
	uint32_t (*get_int_prop)(struct sway_view *view, enum sway_view_prop prop);
	uint32_t (*configure)(struct sway_view *view, double lx, double ly,
			int width, int height);
	void (*set_activated)(struct sway_view *view, bool activated);
	void (*set_tiled)(struct sway_view *view, bool tiled);
	void (*set_fullscreen)(struct sway_view *view, bool fullscreen);
	void (*set_resizing)(struct sway_view *view, bool resizing);
	bool (*wants_floating)(struct sway_view *view);
	bool (*is_transient_for)(struct sway_view *child,
			struct sway_view *ancestor);
	void (*close)(struct sway_view *view);
	void (*close_popups)(struct sway_view *view);
	void (*destroy)(struct sway_view *view);
};

struct sway_view {
	enum sway_view_type type;
	const struct sway_view_impl *impl;

	struct wlr_scene_tree *scene_tree;
	struct wlr_scene_tree *content_tree;
	struct wlr_scene_tree *saved_surface_tree;

	struct wlr_scene *image_capture_scene;
	struct wlr_ext_image_capture_source_v1 *image_capture_source;

	struct sway_container *container; // NULL if unmapped and transactions finished
	struct wlr_surface *surface; // NULL for unmapped views
	struct sway_xdg_decoration *xdg_decoration;

	pid_t pid;
	struct launcher_ctx *ctx;

	// The size the view would want to be if it weren't tiled.
	// Used when changing a view from tiled to floating.
	int natural_width, natural_height;

	bool using_csd;

	struct timespec urgent;
	bool allow_request_urgent;
	struct wl_event_source *urgent_timer;

	// The geometry for whatever the client is committing, regardless of
	// transaction state. Updated on every commit.
	struct wlr_box geometry;

	struct wlr_ext_foreign_toplevel_handle_v1 *ext_foreign_toplevel;

	struct wlr_foreign_toplevel_handle_v1 *foreign_toplevel;
	struct wl_listener foreign_activate_request;
	struct wl_listener foreign_fullscreen_request;
	struct wl_listener foreign_close_request;
	struct wl_listener foreign_destroy;

	bool destroying;

	list_t *executed_criteria; // struct criteria *

	union {
		struct wlr_xdg_toplevel *wlr_xdg_toplevel;
#if WLR_HAS_XWAYLAND
		struct wlr_xwayland_surface *wlr_xwayland_surface;
#endif
	};

	struct {
		struct wl_signal unmap;
	} events;

	int max_render_time; // In milliseconds

	enum seat_config_shortcuts_inhibit shortcuts_inhibit;

	enum sway_view_tearing_mode tearing_mode;
	enum wp_tearing_control_v1_presentation_hint tearing_hint;
};

struct sway_xdg_shell_view {
	struct sway_view view;

	struct wlr_scene_tree *image_capture_tree;
	char *tag;

	struct wl_listener commit;
	struct wl_listener request_move;
	struct wl_listener request_resize;
	struct wl_listener request_maximize;
	struct wl_listener request_fullscreen;
	struct wl_listener set_title;
	struct wl_listener set_app_id;
	struct wl_listener new_popup;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener destroy;
};
#if WLR_HAS_XWAYLAND
struct sway_xwayland_view {
	struct sway_view view;

	struct wlr_scene_tree *surface_tree;

	struct wlr_scene_surface *image_capture_scene_surface;

	struct wl_listener commit;
	struct wl_listener request_move;
	struct wl_listener request_resize;
	struct wl_listener request_maximize;
	struct wl_listener request_minimize;
	struct wl_listener request_configure;
	struct wl_listener request_fullscreen;
	struct wl_listener request_activate;
	struct wl_listener set_title;
	struct wl_listener set_class;
	struct wl_listener set_role;
	struct wl_listener set_startup_id;
	struct wl_listener set_window_type;
	struct wl_listener set_hints;
	struct wl_listener set_decorations;
	struct wl_listener associate;
	struct wl_listener dissociate;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener destroy;
	struct wl_listener override_redirect;

	struct wl_listener surface_tree_destroy;
};

struct sway_xwayland_unmanaged {
	struct wlr_xwayland_surface *wlr_xwayland_surface;

	struct wlr_scene_surface *surface_scene;

	struct wl_listener request_activate;
	struct wl_listener request_configure;
	struct wl_listener request_fullscreen;
	struct wl_listener set_geometry;
	struct wl_listener associate;
	struct wl_listener dissociate;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener destroy;
	struct wl_listener override_redirect;
};
#endif

struct sway_popup_desc {
	struct wlr_scene_node *relative;
	struct sway_view *view;
};

struct sway_xdg_popup {
	struct sway_view *view;
	struct wlr_xdg_popup *wlr_xdg_popup;

	struct wlr_scene_tree *scene_tree;
	struct wlr_scene_tree *xdg_surface_tree;

	struct wlr_scene_tree *image_capture_tree;

	struct sway_popup_desc desc;

	struct wl_listener surface_commit;
	struct wl_listener new_popup;
	struct wl_listener reposition;
	struct wl_listener destroy;
};

const char *view_get_title(struct sway_view *view);

const char *view_get_app_id(struct sway_view *view);

const char *view_get_class(struct sway_view *view);

const char *view_get_instance(struct sway_view *view);

uint32_t view_get_x11_window_id(struct sway_view *view);

uint32_t view_get_x11_parent_id(struct sway_view *view);

const char *view_get_window_role(struct sway_view *view);

uint32_t view_get_window_type(struct sway_view *view);

const char *view_get_sandbox_engine(struct sway_view *view);

const char *view_get_sandbox_app_id(struct sway_view *view);

const char *view_get_sandbox_instance_id(struct sway_view *view);

const char *view_get_tag(struct sway_view *view);

const char *view_get_shell(struct sway_view *view);

void view_get_constraints(struct sway_view *view, double *min_width,
		double *max_width, double *min_height, double *max_height);

uint32_t view_configure(struct sway_view *view, double lx, double ly, int width,
	int height);

bool view_inhibit_idle(struct sway_view *view);

/**
 * Whether or not this view's most distant ancestor (possibly itself) is the
 * only visible node in its tree. If the view is tiling, there may be floating
 * views. If the view is floating, there may be tiling views or views in a
 * different floating container.
 */
bool view_ancestor_is_only_visible(struct sway_view *view);

/**
 * Configure the view's position and size based on the container's position and
 * size, taking borders into consideration.
 */
void view_autoconfigure(struct sway_view *view);

void view_set_activated(struct sway_view *view, bool activated);

/**
 * Called when the view requests to be focused.
 */
void view_request_activate(struct sway_view *view, struct sway_seat *seat);

/*
 * Called when the view requests urgent state
 */
void view_request_urgent(struct sway_view *view);

/**
 * If possible, instructs the client to change their decoration mode.
 */
void view_set_csd_from_server(struct sway_view *view, bool enabled);

/**
 * Updates the view's border setting when the client unexpectedly changes their
 * decoration mode.
 */
void view_update_csd_from_client(struct sway_view *view, bool enabled);

void view_set_tiled(struct sway_view *view, bool tiled);

void view_close(struct sway_view *view);

void view_close_popups(struct sway_view *view);

// view implementation

bool view_init(struct sway_view *view, enum sway_view_type type,
	const struct sway_view_impl *impl);

void view_destroy(struct sway_view *view);

void view_begin_destroy(struct sway_view *view);

/**
 * Map a view, ie. make it visible in the tree.
 *
 * `fullscreen` should be set to true (and optionally `fullscreen_output`
 * should be populated) if the view should be made fullscreen immediately.
 *
 * `decoration` should be set to true if the client prefers CSD. The client's
 * preference may be ignored.
 */
void view_map(struct sway_view *view, struct wlr_surface *wlr_surface,
	bool fullscreen, struct wlr_output *fullscreen_output, bool decoration);

void view_unmap(struct sway_view *view);

void view_update_size(struct sway_view *view);
void view_center_and_clip_surface(struct sway_view *view);

struct sway_view *view_from_wlr_xdg_surface(
	struct wlr_xdg_surface *xdg_surface);
#if WLR_HAS_XWAYLAND
struct sway_view *view_from_wlr_xwayland_surface(
	struct wlr_xwayland_surface *xsurface);
#endif
struct sway_view *view_from_wlr_surface(struct wlr_surface *surface);

void view_update_app_id(struct sway_view *view);

/**
 * Re-read the view's title property and update any relevant title bars.
 * The force argument makes it recreate the title bars even if the title hasn't
 * changed.
 */
void view_update_title(struct sway_view *view, bool force);

/**
 * Run any criteria that match the view and haven't been run on this view
 * before.
 */
void view_execute_criteria(struct sway_view *view);

/**
 * Returns true if there's a possibility the view may be rendered on screen.
 * Intended for damage tracking.
 */
bool view_is_visible(struct sway_view *view);

void view_set_urgent(struct sway_view *view, bool enable);

bool view_is_urgent(struct sway_view *view);

void view_remove_saved_buffer(struct sway_view *view);

void view_save_buffer(struct sway_view *view);

bool view_is_transient_for(struct sway_view *child, struct sway_view *ancestor);

void view_assign_ctx(struct sway_view *view, struct launcher_ctx *ctx);

void view_send_frame_done(struct sway_view *view);

bool view_can_tear(struct sway_view *view);

void xdg_toplevel_tag_manager_v1_handle_set_tag(struct wl_listener *listener, void *data);

#endif

```

**workspace.h** - Espacios de trabajo:

**Archivo**: `sway/include/sway/tree/workspace.h`

```c
#ifndef _SWAY_WORKSPACE_H
#define _SWAY_WORKSPACE_H

#include <stdbool.h>
#include <wlr/types/wlr_scene.h>
#include "sway/config.h"
#include "sway/tree/container.h"
#include "sway/tree/node.h"

struct sway_view;

struct sway_workspace_state {
	struct sway_container *fullscreen;
	double x, y;
	int width, height;
	enum sway_container_layout layout;
	struct sway_output *output;
	list_t *floating;
	list_t *tiling;

	struct sway_container *focused_inactive_child;
	bool focused;
};

struct sway_workspace {
	struct sway_node node;

	struct {
		struct wlr_scene_tree *tiling;
		struct wlr_scene_tree *fullscreen;
	} layers;

	struct sway_container *fullscreen;

	char *name;
	char *representation;

	double x, y;
	int width, height;
	enum sway_container_layout layout;
	enum sway_container_layout prev_split_layout;

	struct side_gaps current_gaps;
	int gaps_inner;
	struct side_gaps gaps_outer;

	struct sway_output *output; // NULL if no outputs are connected
	list_t *floating;           // struct sway_container
	list_t *tiling;             // struct sway_container
	list_t *output_priority;
	bool urgent;

	struct sway_workspace_state current;
};

struct workspace_config *workspace_find_config(const char *ws_name);

struct sway_output *workspace_get_initial_output(const char *name);

struct sway_workspace *workspace_create(struct sway_output *output,
		const char *name);

void workspace_destroy(struct sway_workspace *workspace);

void workspace_begin_destroy(struct sway_workspace *workspace);

void workspace_consider_destroy(struct sway_workspace *ws);

char *workspace_next_name(const char *output_name);

struct sway_workspace *workspace_auto_back_and_forth(
		struct sway_workspace *workspace);

bool workspace_switch(struct sway_workspace *workspace);

struct sway_workspace *workspace_by_number(const char* name);

struct sway_workspace *workspace_by_name(const char*);

struct sway_workspace *workspace_output_next(struct sway_workspace *current);

struct sway_workspace *workspace_next(struct sway_workspace *current);

struct sway_workspace *workspace_output_prev(struct sway_workspace *current);

struct sway_workspace *workspace_prev(struct sway_workspace *current);

bool workspace_is_visible(struct sway_workspace *ws);

bool workspace_is_empty(struct sway_workspace *ws);

void workspace_output_raise_priority(struct sway_workspace *workspace,
		struct sway_output *old_output, struct sway_output *new_output);

void workspace_output_add_priority(struct sway_workspace *workspace,
		struct sway_output *output);

struct sway_output *workspace_output_get_highest_available(
		struct sway_workspace *ws);

void workspace_detect_urgent(struct sway_workspace *workspace);

void workspace_for_each_container(struct sway_workspace *ws,
		void (*f)(struct sway_container *con, void *data), void *data);

struct sway_container *workspace_find_container(struct sway_workspace *ws,
		bool (*test)(struct sway_container *con, void *data), void *data);

/**
 * Wrap the workspace's tiling children in a new container.
 * The new container will be the only direct tiling child of the workspace.
 * The new container is returned.
 */
struct sway_container *workspace_wrap_children(struct sway_workspace *ws);

void workspace_unwrap_children(struct sway_workspace *ws,
		struct sway_container *wrap);

void workspace_detach(struct sway_workspace *workspace);

struct sway_container *workspace_add_tiling(struct sway_workspace *workspace,
		struct sway_container *con);

void workspace_add_floating(struct sway_workspace *workspace,
		struct sway_container *con);

/**
 * Adds a tiling container to the workspace without considering
 * the workspace_layout, so the con will not be split.
 */
void workspace_insert_tiling_direct(struct sway_workspace *workspace,
		struct sway_container *con, int index);

struct sway_container *workspace_insert_tiling(struct sway_workspace *workspace,
		struct sway_container *con, int index);

void workspace_remove_gaps(struct sway_workspace *ws);

void workspace_add_gaps(struct sway_workspace *ws);

struct sway_container *workspace_split(struct sway_workspace *workspace,
		enum sway_container_layout layout);

void workspace_update_representation(struct sway_workspace *ws);

void workspace_get_box(struct sway_workspace *workspace, struct wlr_box *box);

size_t workspace_num_tiling_views(struct sway_workspace *ws);

size_t workspace_num_sticky_containers(struct sway_workspace *ws);

/**
 * workspace_squash is container_flatten in the reverse
 * direction. Instead of eliminating redundant splits that are
 * parents of the target container, it eliminates pairs of
 * redundant H/V splits that are children of the workspace.
 */
void workspace_squash(struct sway_workspace *workspace);

#endif

```


### 3.3 Algoritmo de Layout en Sway

**arrange.c** - Cálculo de layouts:

**Archivo**: `sway/sway/tree/arrange.c`

```c
#include <ctype.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_output_layout.h>
#include "sway/tree/arrange.h"
#include "sway/tree/container.h"
#include "sway/output.h"
#include "sway/tree/workspace.h"
#include "sway/tree/view.h"
#include "list.h"
#include "log.h"

static void apply_horiz_layout(list_t *children, struct wlr_box *parent) {
	if (!children->length) {
		return;
	}

	// Count the number of new windows we are resizing, and how much space
	// is currently occupied
	int new_children = 0;
	double current_width_fraction = 0;
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		current_width_fraction += child->width_fraction;
		if (child->width_fraction <= 0) {
			new_children += 1;
		}
	}

	// Calculate each width fraction
	double total_width_fraction = 0;
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		if (child->width_fraction <= 0) {
			if (current_width_fraction <= 0) {
				child->width_fraction = 1.0;
			} else if (children->length > new_children) {
				child->width_fraction = current_width_fraction /
					(children->length - new_children);
			} else {
				child->width_fraction = current_width_fraction;
			}
		}
		total_width_fraction += child->width_fraction;
	}
	// Normalize width fractions so the sum is 1.0
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		child->width_fraction /= total_width_fraction;
	}

	// Calculate gap size
	double inner_gap = 0;
	struct sway_container *child = children->items[0];
	struct sway_workspace *ws = child->pending.workspace;
	if (ws) {
		inner_gap = ws->gaps_inner;
	}
	// Descendants of tabbed/stacked containers don't have gaps
	struct sway_container *temp = child;
	while (temp) {
		enum sway_container_layout layout = container_parent_layout(temp);
		if (layout == L_TABBED || layout == L_STACKED) {
			inner_gap = 0;
		}
		temp = temp->pending.parent;
	}
	double total_gap = fmin(inner_gap * (children->length - 1),
		fmax(0, parent->width - MIN_SANE_W * children->length));
	double child_total_width = parent->width - total_gap;
	inner_gap = floor(total_gap / (children->length - 1));

	// Resize windows
	sway_log(SWAY_DEBUG, "Arranging %p horizontally", parent);
	double child_x = parent->x;
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		child->child_total_width = child_total_width;
		child->pending.x = child_x;
		child->pending.y = parent->y;
		child->pending.width = round(child->width_fraction * child_total_width);
		child->pending.height = parent->height;

		// Make last child use remaining width of parent
		if (i == children->length - 1) {
			child->pending.width = parent->x + parent->width - child->pending.x;
		}

		// Arbitrary lower bound for window size
		if (child->pending.width < 10 || child->pending.height < 10) {
			child->pending.width = 0;
			child->pending.height = 0;
		}
		child_x += child->pending.width + inner_gap;
	}
}

static void apply_vert_layout(list_t *children, struct wlr_box *parent) {
	if (!children->length) {
		return;
	}

	// Count the number of new windows we are resizing, and how much space
	// is currently occupied
	int new_children = 0;
	double current_height_fraction = 0;
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		current_height_fraction += child->height_fraction;
		if (child->height_fraction <= 0) {
			new_children += 1;
		}
	}

	// Calculate each height fraction
	double total_height_fraction = 0;
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		if (child->height_fraction <= 0) {
			if (current_height_fraction <= 0) {
				child->height_fraction = 1.0;
			} else if (children->length > new_children) {
				child->height_fraction = current_height_fraction /
					(children->length - new_children);
			} else {
				child->height_fraction = current_height_fraction;
			}
		}
		total_height_fraction += child->height_fraction;
	}
	// Normalize height fractions so the sum is 1.0
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		child->height_fraction /= total_height_fraction;
	}

	// Calculate gap size
	double inner_gap = 0;
	struct sway_container *child = children->items[0];
	struct sway_workspace *ws = child->pending.workspace;
	if (ws) {
		inner_gap = ws->gaps_inner;
	}
	// Descendants of tabbed/stacked containers don't have gaps
	struct sway_container *temp = child;
	while (temp) {
		enum sway_container_layout layout = container_parent_layout(temp);
		if (layout == L_TABBED || layout == L_STACKED) {
			inner_gap = 0;
		}
		temp = temp->pending.parent;
	}
	double total_gap = fmin(inner_gap * (children->length - 1),
		fmax(0, parent->height - MIN_SANE_H * children->length));
	double child_total_height = parent->height - total_gap;
	inner_gap = floor(total_gap / (children->length - 1));

	// Resize windows
	sway_log(SWAY_DEBUG, "Arranging %p vertically", parent);
	double child_y = parent->y;
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		child->child_total_height = child_total_height;
		child->pending.x = parent->x;
		child->pending.y = child_y;
		child->pending.width = parent->width;
		child->pending.height = round(child->height_fraction * child_total_height);

		// Make last child use remaining height of parent
		if (i == children->length - 1) {
			child->pending.height = parent->y + parent->height - child->pending.y;
		}

		// Arbitrary lower bound for window size
		if (child->pending.width < 10 || child->pending.height < 10) {
			child->pending.width = 0;
			child->pending.height = 0;
		}
		child_y += child->pending.height + inner_gap;
	}
}

static void apply_tabbed_layout(list_t *children, struct wlr_box *parent) {
	if (!children->length) {
		return;
	}
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		int parent_offset = child->view ? 0 : container_titlebar_height();
		child->pending.x = parent->x;
		child->pending.y = parent->y + parent_offset;
		child->pending.width = parent->width;
		child->pending.height = parent->height - parent_offset;
	}
}

static void apply_stacked_layout(list_t *children, struct wlr_box *parent) {
	if (!children->length) {
		return;
	}
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		int parent_offset = child->view ?  0 :
			container_titlebar_height() * children->length;
		child->pending.x = parent->x;
		child->pending.y = parent->y + parent_offset;
		child->pending.width = parent->width;
		child->pending.height = parent->height - parent_offset;
	}
}

static void arrange_floating(list_t *floating) {
	for (int i = 0; i < floating->length; ++i) {
		struct sway_container *floater = floating->items[i];
		arrange_container(floater);
	}
}

static void arrange_children(list_t *children,
		enum sway_container_layout layout, struct wlr_box *parent) {
	// Calculate x, y, width and height of children
	switch (layout) {
	case L_HORIZ:
		apply_horiz_layout(children, parent);
		break;
	case L_VERT:
		apply_vert_layout(children, parent);
		break;
	case L_TABBED:
		apply_tabbed_layout(children, parent);
		break;
	case L_STACKED:
		apply_stacked_layout(children, parent);
		break;
	case L_NONE:
		apply_horiz_layout(children, parent);
		break;
	}

	// Recurse into child containers
	for (int i = 0; i < children->length; ++i) {
		struct sway_container *child = children->items[i];
		arrange_container(child);
	}
}

void arrange_container(struct sway_container *container) {
	if (config->reloading) {
		return;
	}
	if (container->view) {
		view_autoconfigure(container->view);
		node_set_dirty(&container->node);
		return;
	}
	struct wlr_box box;
	container_get_box(container, &box);
	arrange_children(container->pending.children, container->pending.layout, &box);
	node_set_dirty(&container->node);
}

void arrange_workspace(struct sway_workspace *workspace) {
	if (config->reloading) {
		return;
	}
	if (!workspace->output) {
		// Happens when there are no outputs connected
		return;
	}
	struct sway_output *output = workspace->output;
	struct wlr_box *area = &output->usable_area;
	sway_log(SWAY_DEBUG, "Usable area for ws: %dx%d@%d,%d",
			area->width, area->height, area->x, area->y);

	bool first_arrange = workspace->width == 0 && workspace->height == 0;
	struct wlr_box prev_box;
	workspace_get_box(workspace, &prev_box);

	double prev_x = workspace->x - workspace->current_gaps.left;
	double prev_y = workspace->y - workspace->current_gaps.top;
	workspace->width = area->width;
	workspace->height = area->height;
	workspace->x = output->lx + area->x;
	workspace->y = output->ly + area->y;

	// Adjust any floating containers
	double diff_x = workspace->x - prev_x;
	double diff_y = workspace->y - prev_y;
	if (!first_arrange && (diff_x != 0 || diff_y != 0)) {
		for (int i = 0; i < workspace->floating->length; ++i) {
			struct sway_container *floater = workspace->floating->items[i];
			struct wlr_box workspace_box;
			workspace_get_box(workspace, &workspace_box);
			floating_fix_coordinates(floater, &prev_box, &workspace_box);
			// Set transformation for scratchpad windows.
			if (floater->scratchpad) {
				struct wlr_box output_box;
				output_get_box(output, &output_box);
				floater->transform = output_box;
			}
		}
	}

	workspace_add_gaps(workspace);
	node_set_dirty(&workspace->node);
	sway_log(SWAY_DEBUG, "Arranging workspace '%s' at %f, %f", workspace->name,
			workspace->x, workspace->y);
	if (workspace->fullscreen) {
		struct sway_container *fs = workspace->fullscreen;
		fs->pending.x = output->lx;
		fs->pending.y = output->ly;
		fs->pending.width = output->width;
		fs->pending.height = output->height;
		arrange_container(fs);
	} else {
		struct wlr_box box;
		workspace_get_box(workspace, &box);
		arrange_children(workspace->tiling, workspace->layout, &box);
		arrange_floating(workspace->floating);
	}
}

void arrange_output(struct sway_output *output) {
	if (config->reloading) {
		return;
	}
	if (!output->wlr_output->enabled) {
		return;
	}
	for (int i = 0; i < output->workspaces->length; ++i) {
		struct sway_workspace *workspace = output->workspaces->items[i];
		arrange_workspace(workspace);
	}
}

void arrange_root(void) {
	if (config->reloading) {
		return;
	}
	struct wlr_box layout_box;
	wlr_output_layout_get_box(root->output_layout, NULL, &layout_box);
	root->x = layout_box.x;
	root->y = layout_box.y;
	root->width = layout_box.width;
	root->height = layout_box.height;

	if (root->fullscreen_global) {
		struct sway_container *fs = root->fullscreen_global;
		fs->pending.x = root->x;
		fs->pending.y = root->y;
		fs->pending.width = root->width;
		fs->pending.height = root->height;
		arrange_container(fs);
	} else {
		for (int i = 0; i < root->outputs->length; ++i) {
			struct sway_output *output = root->outputs->items[i];
			arrange_output(output);
		}
	}
}

void arrange_node(struct sway_node *node) {
	switch (node->type) {
	case N_ROOT:
		arrange_root();
		break;
	case N_OUTPUT:
		arrange_output(node->sway_output);
		break;
	case N_WORKSPACE:
		arrange_workspace(node->sway_workspace);
		break;
	case N_CONTAINER:
		arrange_container(node->sway_container);
		break;
	}
}

```


### 3.4 Navegación Direccional en Sway (IMPORTANTE)

Este es el algoritmo clave para `focus left/right/up/down`. **Fundamental para la implementación**.

**Archivo**: `sway/sway/commands/focus.c`

```c
/**
 * Navegación direccional para ventanas tiling.
 * Algoritmo:
 * 1. Buscar en siblings del mismo parent según dirección
 * 2. Si no hay, subir al parent y repetir
 * 3. Si llegamos a la raíz, buscar en otro output
 * 4. Manejar wrap-around si está habilitado
 */
static struct sway_node *node_get_in_direction_tiling(
		struct sway_container *container, struct sway_seat *seat,
		enum wlr_direction dir, bool descend) {
	struct sway_container *wrap_candidate = NULL;
	struct sway_container *current = container;
	while (current) {
		if (current->pending.fullscreen_mode == FULLSCREEN_WORKSPACE) {
			// Fullscreen container with a direction - go straight to outputs
			struct sway_output *output = current->pending.workspace->output;
			struct sway_output *new_output =
				output_get_in_direction(output, dir);
			if (!new_output) {
				return NULL;
			}
			return get_node_in_output_direction(new_output, dir);
		}
		if (current->pending.fullscreen_mode == FULLSCREEN_GLOBAL) {
			return NULL;
		}

		bool can_move = false;
		int desired;
		int idx = container_sibling_index(current);
		enum sway_container_layout parent_layout =
			container_parent_layout(current);
		list_t *siblings = container_get_siblings(current);

		// CLAVE: Solo podemos movernos si la dirección coincide con el layout
		if (dir == WLR_DIRECTION_LEFT || dir == WLR_DIRECTION_RIGHT) {
			if (parent_layout == L_HORIZ || parent_layout == L_TABBED) {
				can_move = true;
				desired = idx + (dir == WLR_DIRECTION_LEFT ? -1 : 1);
			}
		} else {
			if (parent_layout == L_VERT || parent_layout == L_STACKED) {
				can_move = true;
				desired = idx + (dir == WLR_DIRECTION_UP ? -1 : 1);
			}
		}

		if (can_move) {
			if (desired < 0 || desired >= siblings->length) {
				// Fuera de rango - considerar wrap-around
				int len = siblings->length;
				if (config->focus_wrapping != WRAP_NO && !wrap_candidate
						&& len > 1) {
					if (desired < 0) {
						wrap_candidate = siblings->items[len-1];
					} else {
						wrap_candidate = siblings->items[0];
					}
					if (config->focus_wrapping == WRAP_FORCE) {
						struct sway_container *c = seat_get_focus_inactive_view(
								seat, &wrap_candidate->node);
						return &c->node;
					}
				}
			} else {
				// ¡Encontramos un sibling válido!
				struct sway_container *desired_con = siblings->items[desired];
				if (!descend) {
					return &desired_con->node;
				} else {
					// Descender al hijo más profundo en la dirección
					struct sway_container *c = seat_get_focus_inactive_view(
							seat, &desired_con->node);
					return &c->node;
				}
			}
		}

		// Subir al parent y seguir buscando
		current = current->pending.parent;
	}

	// Check a different output
	struct sway_output *output = container->pending.workspace->output;
	struct sway_output *new_output = output_get_in_direction(output, dir);
	if ((config->focus_wrapping != WRAP_WORKSPACE ||
				container->node.type == N_WORKSPACE) && new_output) {
		return get_node_in_output_direction(new_output, dir);
	}

	// If there is a wrap candidate, return its focus inactive view
	if (wrap_candidate) {
		struct sway_container *wrap_inactive = seat_get_focus_inactive_view(
				seat, &wrap_candidate->node);
		return &wrap_inactive->node;
	}

	return NULL;
}

/**
 * Navegación direccional para ventanas floating.
 * Usa distancia geométrica al centro de cada ventana.
 */
static struct sway_node *node_get_in_direction_floating(
		struct sway_container *con, struct sway_seat *seat,
		enum wlr_direction dir) {
	double ref_lx = con->pending.x + con->pending.width / 2;
	double ref_ly = con->pending.y + con->pending.height / 2;
	double closest_distance = DBL_MAX;
	struct sway_container *closest_con = NULL;

	if (!con->pending.workspace) {
		return NULL;
	}

	for (int i = 0; i < con->pending.workspace->floating->length; i++) {
		struct sway_container *floater = con->pending.workspace->floating->items[i];
		if (floater == con) {
			continue;
		}
		float distance = dir == WLR_DIRECTION_LEFT || dir == WLR_DIRECTION_RIGHT
			? (floater->pending.x + floater->pending.width / 2) - ref_lx
			: (floater->pending.y + floater->pending.height / 2) - ref_ly;
		if (dir == WLR_DIRECTION_LEFT || dir == WLR_DIRECTION_UP) {
			distance = -distance;
		}
		if (distance < 0) {
			continue;
		}
		if (distance < closest_distance) {
			closest_distance = distance;
			closest_con = floater;
		}
	}

	return closest_con ? &closest_con->node : NULL;
}
```

**Adaptación a Rust con SlotMap:**

```rust
impl Tree {
    /// Navegación direccional basada en el algoritmo de Sway
    pub fn get_in_direction(&self, from: ContainerKey, direction: Direction) -> Option<ContainerKey> {
        let mut current = from;

        loop {
            let container = self.containers.get(current)?;

            // Obtener parent y siblings
            let parent_key = container.parent?;
            let parent = self.containers.get(parent_key)?;
            let siblings = &parent.children;

            // Encontrar índice actual
            let idx = siblings.iter().position(|&k| k == current)?;

            // ¿Podemos movernos en esta dirección con este layout?
            let can_move = match (direction, parent.layout) {
                (Direction::Left | Direction::Right, Layout::SplitH) => true,
                (Direction::Up | Direction::Down, Layout::SplitV) => true,
                _ => false,
            };

            if can_move {
                let desired: i32 = match direction {
                    Direction::Left | Direction::Up => idx as i32 - 1,
                    Direction::Right | Direction::Down => idx as i32 + 1,
                };

                if desired >= 0 && (desired as usize) < siblings.len() {
                    // ¡Encontrado!
                    let target = siblings[desired as usize];
                    // Descender al leaf más profundo
                    return Some(self.descend_focused(target));
                }
                // Si no hay sibling, podríamos hacer wrap-around aquí
            }

            // Subir al parent y seguir buscando
            current = parent_key;
        }
    }

    /// Descender al nodo hoja más profundo (el último focuseado)
    fn descend_focused(&self, key: ContainerKey) -> ContainerKey {
        let container = match self.containers.get(key) {
            Some(c) => c,
            None => return key,
        };

        if container.children.is_empty() {
            return key; // Es un leaf
        }

        // En un árbol real, aquí buscaríamos el "focus inactive"
        // Por simplicidad, tomamos el primer hijo
        self.descend_focused(container.children[0])
    }
}
```

### 3.5 Focus y Gestión de Seat

**seat.c** - Gestión de focus (seat es el concepto de Wayland para input)

```c
}

struct sway_container *seat_get_focus_inactive_view(struct sway_seat *seat,
		struct sway_node *ancestor) {
	if (node_is_view(ancestor)) {
		return ancestor->sway_container;
	}
	struct sway_seat_node *current;
	wl_list_for_each(current, &seat->focus_stack, link) {
		struct sway_node *node = current->node;
		if (node_is_view(node) && node_has_ancestor(node, ancestor)) {
			return node->sway_container;
		}
	}
	return NULL;
}

static void handle_seat_node_destroy(struct wl_listener *listener, void *data) {
	struct sway_seat_node *seat_node =
		wl_container_of(listener, seat_node, destroy);
	struct sway_seat *seat = seat_node->seat;
	struct sway_node *node = seat_node->node;
	struct sway_node *parent = node_get_parent(node);
	struct sway_node *focus = seat_get_focus(seat);

	if (node->type == N_WORKSPACE) {
		seat_node_destroy(seat_node);
		// If an unmanaged or layer surface is focused when an output gets
		// disabled and an empty workspace on the output was focused by the
		// seat, the seat needs to refocus its focus inactive to update the
		// value of seat->workspace.
		if (seat->workspace == node->sway_workspace) {
			struct sway_node *node = seat_get_focus_inactive(seat, &root->node);
			seat_set_focus(seat, NULL);
			if (node) {
				seat_set_focus(seat, node);
			} else {
				seat->workspace = NULL;
			}
		}
		return;
	}

	// Even though the container being destroyed might be nowhere near the
	// focused container, we still need to set focus_inactive on a sibling of
	// the container being destroyed.
	bool needs_new_focus = focus &&
		(focus == node || node_has_ancestor(focus, node));

	seat_node_destroy(seat_node);

	if (!parent && !needs_new_focus) {
		// Destroying a container that is no longer in the tree
		return;
	}

--
	while (next_focus == NULL && parent != NULL) {
		struct sway_container *con =
			seat_get_focus_inactive_view(seat, parent);
		next_focus = con ? &con->node : NULL;

		if (next_focus == NULL && parent->type == N_WORKSPACE) {
			next_focus = parent;
			break;
		}

		parent = node_get_parent(parent);
	}

	if (!next_focus) {
		struct sway_workspace *ws = seat_get_last_known_workspace(seat);
		if (!ws) {
			return;
		}
		struct sway_container *con =
			seat_get_focus_inactive_view(seat, &ws->node);
		next_focus = con ? &(con->node) : &(ws->node);
	}

	if (next_focus->type == N_WORKSPACE &&
			!workspace_is_visible(next_focus->sway_workspace)) {
		// Do not change focus to a non-visible workspace
		return;
	}

	if (needs_new_focus) {
		// Make sure the workspace IPC event gets sent
		if (node->type == N_CONTAINER && node->sway_container->scratchpad) {
			seat_set_focus(seat, NULL);
		}
		// The structure change might have caused it to move up to the top of
		// the focus stack without sending focus notifications to the view
		if (seat_get_focus(seat) == next_focus) {
			seat_send_focus(next_focus, seat);
		} else {
			seat_set_focus(seat, next_focus);
		}
	} else {
		// Setting focus_inactive
```

**container.c** - Navegación entre contenedores

```c
}

enum sway_container_layout container_parent_layout(struct sway_container *con) {
	if (con->pending.parent) {
		return con->pending.parent->pending.layout;
	}
	if (con->pending.workspace) {
		return con->pending.workspace->layout;
	}
	return L_NONE;
}

list_t *container_get_siblings(struct sway_container *container) {
	if (container->pending.parent) {
		return container->pending.parent->pending.children;
	}
	if (!container->pending.workspace) {
		return NULL;
	}
	if (list_find(container->pending.workspace->tiling, container) != -1) {
		return container->pending.workspace->tiling;
	}
	return container->pending.workspace->floating;
--
		list_t *siblings = container_get_siblings(child);
		if (siblings->length == 1) {
			enum sway_container_layout current = container_parent_layout(child);
			if (container_is_floating(child)) {
				current = L_NONE;
			}
			if (current == L_HORIZ || current == L_VERT) {
				if (child->pending.parent) {
					child->pending.parent->pending.layout = layout;
					container_update_representation(child->pending.parent);
				} else {
					child->pending.workspace->layout = layout;
					workspace_update_representation(child->pending.workspace);
				}
				return child;
			}
		}
	}

	struct sway_seat *seat = input_manager_get_default_seat();
	bool set_focus = (seat_get_focus(seat) == &child->node);

	if (container_is_floating(child) && child->view) {
--
static bool container_is_squashable(struct sway_container *con,
		struct sway_container *child) {
	enum sway_container_layout gp_layout = container_parent_layout(con);
	return (con->pending.layout == L_HORIZ || con->pending.layout == L_VERT) &&
		(child->pending.layout == L_HORIZ || child->pending.layout == L_VERT) &&
		!is_parallel(con->pending.layout, child->pending.layout) &&
		is_parallel(gp_layout, child->pending.layout);
}

static void container_squash_children(struct sway_container *con) {
	for (int i = 0; i < con->pending.children->length; i++) {
		struct sway_container *child = con->pending.children->items[i];
		i += container_squash(child);
	}
}

int container_squash(struct sway_container *con) {
	if (!con->pending.children) {
		return 0;
	}
	if (con->pending.children->length != 1) {
		container_squash_children(con);
		return 0;
--
		struct sway_workspace *ws1 = con1->pending.workspace;
		struct sway_workspace *ws2 = con2->pending.workspace;
		enum sway_container_layout layout1 = container_parent_layout(con1);
		enum sway_container_layout layout2 = container_parent_layout(con2);
		if (focus == con1 && (layout2 == L_TABBED || layout2 == L_STACKED)) {
			if (workspace_is_visible(ws2)) {
				seat_set_focus(seat, &con2->node);
			}
			seat_set_focus_container(seat, ws1 != ws2 ? con2 : con1);
		} else if (focus == con2 && (layout1 == L_TABBED
					|| layout1 == L_STACKED)) {
			if (workspace_is_visible(ws1)) {
				seat_set_focus(seat, &con1->node);
			}
			seat_set_focus_container(seat, ws1 != ws2 ? con1 : con2);
		} else if (ws1 != ws2) {
			seat_set_focus_container(seat, focus == con1 ? con2 : con1);
		} else {
			seat_set_focus_container(seat, focus);
		}
	} else {
		seat_set_focus_container(seat, focus);
	}

```


## 4. Adaptación a Rust con SlotMap

### 4.1 Por qué SlotMap

SlotMap es ideal para este caso de uso porque:

1. **Indices estables**: Las keys no cambian aunque se eliminen elementos
2. **Sin lifetimes complicados**: No necesitas lifetimes para referencias entre nodos
3. **Generacional**: Detecta automáticamente referencias obsoletas (dangling)
4. **Performance**: O(1) para insert/remove/get
5. **Type-safe**: Cada SlotMap tiene su propio tipo de key

### 4.2 Diseño Propuesto con SlotMap

```rust
use slotmap::{SlotMap, new_key_type};

// Define un tipo de key específico para contenedores
new_key_type! {
    pub struct ContainerKey;
}

// Tipo de contenedor
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContainerType {
    Root,
    Output,
    Workspace,
    Split(SplitDirection),
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layout {
    SplitH,
    SplitV,
    Stacked,
    Tabbed,
}

// Estructura principal del contenedor
pub struct Container {
    pub container_type: ContainerType,
    pub layout: Layout,

    // Referencias usando SlotMap keys
    pub parent: Option<ContainerKey>,
    pub children: Vec<ContainerKey>,

    // Geometría
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,

    // Focus
    pub focused: bool,
    pub urgent: bool,

    // Para windows
    pub window_id: Option<WindowId>, // tu tipo de window de niri

    // Metadata
    pub name: Option<String>,
}

// El "árbol" es simplemente un SlotMap
pub struct Tree {
    containers: SlotMap<ContainerKey, Container>,
    root: ContainerKey,
    focused: Option<ContainerKey>,
}

impl Tree {
    pub fn new() -> Self {
        let mut containers = SlotMap::with_key();
        let root = containers.insert(Container {
            container_type: ContainerType::Root,
            layout: Layout::SplitH,
            parent: None,
            children: Vec::new(),
            x: 0, y: 0, width: 0, height: 0,
            focused: false,
            urgent: false,
            window_id: None,
            name: Some("root".to_string()),
        });

        Self {
            containers,
            root,
            focused: None,
        }
    }

    // Obtener un contenedor
    pub fn get(&self, key: ContainerKey) -> Option<&Container> {
        self.containers.get(key)
    }

    pub fn get_mut(&mut self, key: ContainerKey) -> Option<&mut Container> {
        self.containers.get_mut(key)
    }

    // Insertar un hijo
    pub fn insert_child(&mut self, parent_key: ContainerKey, child: Container) -> Option<ContainerKey> {
        let child_key = self.containers.insert(child);

        if let Some(parent) = self.containers.get_mut(parent_key) {
            parent.children.push(child_key);
        }

        if let Some(child) = self.containers.get_mut(child_key) {
            child.parent = Some(parent_key);
        }

        Some(child_key)
    }

    // Eliminar un contenedor
    pub fn remove(&mut self, key: ContainerKey) -> Option<Container> {
        // Primero eliminar de la lista de hijos del padre
        if let Some(container) = self.containers.get(key) {
            if let Some(parent_key) = container.parent {
                if let Some(parent) = self.containers.get_mut(parent_key) {
                    parent.children.retain(|&k| k != key);
                }
            }
        }

        // Luego eliminar del SlotMap
        self.containers.remove(key)
    }

    // Navegación direccional (inspirado en i3/sway)
    pub fn get_in_direction(&self, from: ContainerKey, direction: Direction) -> Option<ContainerKey> {
        // Implementación similar a container_get_in_direction de sway
        // TODO: buscar en siblings primero, luego subir al parent y buscar
        todo!("Implementar navegación direccional")
    }

    // Cambiar focus
    pub fn focus(&mut self, key: ContainerKey) {
        // Quitar focus del anterior
        if let Some(old_focused) = self.focused {
            if let Some(container) = self.containers.get_mut(old_focused) {
                container.focused = false;
            }
        }

        // Poner focus en el nuevo
        if let Some(container) = self.containers.get_mut(key) {
            container.focused = true;
        }

        self.focused = Some(key);
    }

    // Split de un contenedor
    pub fn split(&mut self, key: ContainerKey, direction: SplitDirection) -> Option<ContainerKey> {
        // Similar a tree_split en i3
        let container = self.containers.get(key)?;
        let parent = container.parent?;

        // Crear nuevo contenedor split
        let split_container = Container {
            container_type: ContainerType::Split(direction),
            layout: match direction {
                SplitDirection::Horizontal => Layout::SplitH,
                SplitDirection::Vertical => Layout::SplitV,
            },
            parent: Some(parent),
            children: vec![key],
            x: container.x,
            y: container.y,
            width: container.width,
            height: container.height,
            focused: false,
            urgent: false,
            window_id: None,
            name: None,
        };

        let split_key = self.containers.insert(split_container);

        // Actualizar padre
        if let Some(parent_container) = self.containers.get_mut(parent) {
            // Reemplazar key con split_key en children
            if let Some(pos) = parent_container.children.iter().position(|&k| k == key) {
                parent_container.children[pos] = split_key;
            }
        }

        // Actualizar el contenedor original
        if let Some(container) = self.containers.get_mut(key) {
            container.parent = Some(split_key);
        }

        Some(split_key)
    }

    // Calcular layout (inspirado en render_con de i3)
    pub fn arrange(&mut self, key: ContainerKey) {
        let container = match self.containers.get(key) {
            Some(c) => c,
            None => return,
        };

        let children_keys: Vec<_> = container.children.clone();
        if children_keys.is_empty() {
            return;
        }

        let (x, y, width, height) = (container.x, container.y, container.width, container.height);
        let layout = container.layout;

        match layout {
            Layout::SplitH => {
                // Dividir horizontalmente
                let child_width = width / children_keys.len() as u32;
                for (i, &child_key) in children_keys.iter().enumerate() {
                    if let Some(child) = self.containers.get_mut(child_key) {
                        child.x = x + (i as i32 * child_width as i32);
                        child.y = y;
                        child.width = child_width;
                        child.height = height;
                    }
                    self.arrange(child_key);
                }
            }
            Layout::SplitV => {
                // Dividir verticalmente
                let child_height = height / children_keys.len() as u32;
                for (i, &child_key) in children_keys.iter().enumerate() {
                    if let Some(child) = self.containers.get_mut(child_key) {
                        child.x = x;
                        child.y = y + (i as i32 * child_height as i32);
                        child.width = width;
                        child.height = child_height;
                    }
                    self.arrange(child_key);
                }
            }
            Layout::Stacked | Layout::Tabbed => {
                // Todos los hijos ocupan todo el espacio
                // pero solo uno es visible a la vez
                for &child_key in &children_keys {
                    if let Some(child) = self.containers.get_mut(child_key) {
                        child.x = x;
                        child.y = y;
                        child.width = width;
                        child.height = height;
                    }
                    self.arrange(child_key);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}
```

### 4.3 Ventajas de este Diseño

1. **No hay Rc/Arc**: SlotMap keys son Copy, muy livianas
2. **No hay RefCell**: Mutabilidad clara con &mut Tree
3. **Detección de errores**: Si usas una key vieja después de remove, get devuelve None
4. **Serializable**: Las keys se pueden serializar para IPC
5. **Debugging**: Puedes iterar todo el SlotMap para debug

### 4.4 Integración con Niri

Para integrar esto con niri:

1. **Reemplazar el scrolling layout** de niri con el tree-based layout
2. **Mantener el compositor de niri**: niri ya maneja wlroots bien
3. **Adaptar los comandos**: i3-style commands -> operaciones sobre el Tree
4. **IPC**: Similar a Sway, usar JSON sobre socket Unix

```rust
// Ejemplo de integración
pub struct NiriI3 {
    tree: Tree,
    // ... otros campos de niri que quieras mantener
}

impl NiriI3 {
    pub fn handle_command(&mut self, cmd: &str) {
        match cmd {
            "split h" => {
                if let Some(focused) = self.tree.focused {
                    self.tree.split(focused, SplitDirection::Horizontal);
                }
            }
            "split v" => {
                if let Some(focused) = self.tree.focused {
                    self.tree.split(focused, SplitDirection::Vertical);
                }
            }
            "focus left" => {
                if let Some(focused) = self.tree.focused {
                    if let Some(left) = self.tree.get_in_direction(focused, Direction::Left) {
                        self.tree.focus(left);
                    }
                }
            }
            // ... más comandos
            _ => {}
        }
    }
}
```


## 5. Referencias y Recursos

### 5.1 Documentación Clave

- **i3 Tree docs**: https://i3wm.org/docs/tree-migrating.html
- **i3 User Guide**: https://i3wm.org/docs/userguide.html
- **Sway Wiki**: https://github.com/swaywm/sway/wiki
- **SlotMap crate**: https://docs.rs/slotmap/latest/slotmap/

### 5.2 Archivos Importantes para Estudiar

**En i3**:
- `include/data.h` - Todas las estructuras de datos
- `src/con.c` - Operaciones sobre contenedores
- `src/tree.c` - Operaciones sobre el árbol
- `src/render.c` - Algoritmo de layout
- `src/commands.c` - Comandos de usuario

**En Sway**:
- `include/sway/tree/container.h` - Definición de contenedor
- `sway/tree/container.c` - Operaciones
- `sway/tree/arrange.c` - Algoritmo de layout (¡MUY IMPORTANTE!)
- `sway/commands/` - Todos los comandos i3
- `sway/input/seat.c` - Focus management

### 5.3 Próximos Pasos

1. **Estudiar este documento** y entender conceptos clave
2. **Experimentar con SlotMap** en un proyecto pequeño
3. **Fork niri** y empezar a reemplazar el scrolling layout
4. **Implementar estructura básica**:
   - Tree con SlotMap
   - Container types básicos
   - Arrange simple (split H/V)
5. **Agregar navegación**: focus left/right/up/down
6. **Implementar splits**: split h/v
7. **Agregar layouts**: tabbed, stacked
8. **IPC**: Comandos i3-style
9. **Testing**: Crear tests unitarios para cada operación

### 5.4 Diferencias Clave vs Niri

| Aspecto | Niri | Tu WM (i3-style) |
|---------|------|------------------|
| Layout | Scrolling horizontal | Tree-based (splits) |
| Navegación | Scroll | Direccional (hjkl) |
| Estructura | Linear | Árbol jerárquico |
| Focus | Scroll position | Tree node |
| Splits | No hay | Horizontal/Vertical |


## 6. Archivos Fuente Analizados

### 6.1 Archivos de i3

```
i3/i3bar/include/configuration.h
i3/i3bar/include/workspaces.h
i3/i3bar/src/config.c
i3/i3bar/src/workspaces.c
i3/i3-config-wizard/i3-config-wizard-atoms.xmacro.h
i3/i3-config-wizard/main.c
i3/i3-config-wizard/xcb.h
i3/include/commands.h
i3/include/commands_parser.h
i3/include/config_directives.h
i3/include/config_parser.h
i3/include/configuration.h
i3/include/con.h
i3/include/render.h
i3/include/tree.h
i3/include/workspace.h
i3/libi3/fake_configure_notify.c
i3/libi3/get_config_path.c
i3/libi3/ipc_connect.c
i3/libi3/root_atom_contents.c
i3/libi3/ucs2_conversion.c
i3/src/commands.c
i3/src/commands_parser.c
i3/src/con.c
i3/src/config.c
i3/src/config_directives.c
i3/src/config_parser.c
i3/src/render.c
i3/src/tree.c
i3/src/workspace.c
```

### 6.2 Archivos de Sway

```
sway/include/sway/commands.h
sway/include/sway/input/seat.h
sway/include/sway/tree/arrange.h
sway/include/sway/tree/container.h
sway/include/sway/tree/node.h
sway/include/sway/tree/root.h
sway/include/sway/tree/view.h
sway/include/sway/tree/workspace.h
sway/sway/commands/allow_tearing.c
sway/sway/commands/assign.c
sway/sway/commands/bar/bind.c
sway/sway/commands/bar/binding_mode_indicator.c
sway/sway/commands/bar.c
sway/sway/commands/bar/colors.c
sway/sway/commands/bar/font.c
sway/sway/commands/bar/gaps.c
sway/sway/commands/bar/height.c
sway/sway/commands/bar/hidden_state.c
sway/sway/commands/bar/icon_theme.c
sway/sway/commands/bar/id.c
sway/sway/commands/bar/mode.c
sway/sway/commands/bar/modifier.c
sway/sway/commands/bar/output.c
sway/sway/commands/bar/pango_markup.c
sway/sway/commands/bar/position.c
sway/sway/commands/bar/separator_symbol.c
sway/sway/commands/bar/status_command.c
sway/sway/commands/bar/status_edge_padding.c
sway/sway/commands/bar/status_padding.c
sway/sway/commands/bar/strip_workspace_name.c
sway/sway/commands/bar/strip_workspace_numbers.c
sway/sway/commands/bar/swaybar_command.c
sway/sway/commands/bar/tray_bind.c
sway/sway/commands/bar/tray_output.c
sway/sway/commands/bar/tray_padding.c
sway/sway/commands/bar/workspace_buttons.c
sway/sway/commands/bar/workspace_min_width.c
sway/sway/commands/bar/wrap_scroll.c
sway/sway/commands/bind.c
sway/sway/commands/border.c
sway/sway/commands.c
sway/sway/commands/client.c
sway/sway/commands/create_output.c
sway/sway/commands/default_border.c
sway/sway/commands/default_floating_border.c
sway/sway/commands/default_orientation.c
sway/sway/commands/exec_always.c
sway/sway/commands/exec.c
sway/sway/commands/exit.c
sway/sway/commands/floating.c
sway/sway/commands/floating_minmax_size.c
sway/sway/commands/floating_modifier.c
sway/sway/commands/focus.c
sway/sway/commands/focus_follows_mouse.c
sway/sway/commands/focus_on_window_activation.c
sway/sway/commands/focus_wrapping.c
sway/sway/commands/font.c
sway/sway/commands/force_display_urgency_hint.c
sway/sway/commands/force_focus_wrapping.c
sway/sway/commands/for_window.c
sway/sway/commands/fullscreen.c
sway/sway/commands/gaps.c
sway/sway/commands/gesture.c
sway/sway/commands/hide_edge_borders.c
sway/sway/commands/include.c
sway/sway/commands/inhibit_idle.c
sway/sway/commands/input/accel_profile.c
sway/sway/commands/input.c
sway/sway/commands/input/calibration_matrix.c
sway/sway/commands/input/clickfinger_button_map.c
sway/sway/commands/input/click_method.c
sway/sway/commands/input/drag.c
sway/sway/commands/input/drag_lock.c
sway/sway/commands/input/dwt.c
sway/sway/commands/input/dwtp.c
sway/sway/commands/input/events.c
sway/sway/commands/input/left_handed.c
sway/sway/commands/input/map_from_region.c
sway/sway/commands/input/map_to_output.c
sway/sway/commands/input/map_to_region.c
sway/sway/commands/input/middle_emulation.c
sway/sway/commands/input/natural_scroll.c
sway/sway/commands/input/pointer_accel.c
sway/sway/commands/input/repeat_delay.c
sway/sway/commands/input/repeat_rate.c
sway/sway/commands/input/rotation_angle.c
sway/sway/commands/input/scroll_button.c
sway/sway/commands/input/scroll_button_lock.c
sway/sway/commands/input/scroll_factor.c
sway/sway/commands/input/scroll_method.c
sway/sway/commands/input/tap_button_map.c
sway/sway/commands/input/tap.c
sway/sway/commands/input/tool_mode.c
sway/sway/commands/input/xkb_capslock.c
sway/sway/commands/input/xkb_file.c
sway/sway/commands/input/xkb_layout.c
sway/sway/commands/input/xkb_model.c
sway/sway/commands/input/xkb_numlock.c
sway/sway/commands/input/xkb_options.c
sway/sway/commands/input/xkb_rules.c
sway/sway/commands/input/xkb_switch_layout.c
sway/sway/commands/input/xkb_variant.c
sway/sway/commands/kill.c
sway/sway/commands/layout.c
sway/sway/commands/mark.c
sway/sway/commands/max_render_time.c
sway/sway/commands/mode.c
sway/sway/commands/mouse_warping.c
sway/sway/commands/move.c
sway/sway/commands/new_float.c
sway/sway/commands/new_window.c
sway/sway/commands/no_focus.c
sway/sway/commands/nop.c
sway/sway/commands/opacity.c
sway/sway/commands/output/adaptive_sync.c
sway/sway/commands/output/allow_tearing.c
sway/sway/commands/output/background.c
sway/sway/commands/output.c
sway/sway/commands/output/color_profile.c
sway/sway/commands/output/disable.c
sway/sway/commands/output/dpms.c
sway/sway/commands/output/enable.c
sway/sway/commands/output/hdr.c
sway/sway/commands/output/max_render_time.c
sway/sway/commands/output/mode.c
sway/sway/commands/output/position.c
sway/sway/commands/output/power.c
sway/sway/commands/output/render_bit_depth.c
sway/sway/commands/output/scale.c
sway/sway/commands/output/scale_filter.c
sway/sway/commands/output/subpixel.c
sway/sway/commands/output/toggle.c
sway/sway/commands/output/transform.c
sway/sway/commands/output/unplug.c
sway/sway/commands/popup_during_fullscreen.c
sway/sway/commands/primary_selection.c
sway/sway/commands/reload.c
sway/sway/commands/rename.c
sway/sway/commands/resize.c
sway/sway/commands/scratchpad.c
sway/sway/commands/seat/attach.c
sway/sway/commands/seat.c
sway/sway/commands/seat/cursor.c
sway/sway/commands/seat/fallback.c
sway/sway/commands/seat/hide_cursor.c
sway/sway/commands/seat/idle.c
sway/sway/commands/seat/keyboard_grouping.c
sway/sway/commands/seat/pointer_constraint.c
sway/sway/commands/seat/shortcuts_inhibitor.c
sway/sway/commands/seat/xcursor_theme.c
sway/sway/commands/set.c
sway/sway/commands/shortcuts_inhibitor.c
sway/sway/commands/show_marks.c
sway/sway/commands/smart_borders.c
sway/sway/commands/smart_gaps.c
sway/sway/commands/split.c
sway/sway/commands/sticky.c
sway/sway/commands/swap.c
sway/sway/commands/swaybg_command.c
sway/sway/commands/swaynag_command.c
sway/sway/commands/tiling_drag.c
sway/sway/commands/tiling_drag_threshold.c
sway/sway/commands/title_align.c
sway/sway/commands/titlebar_border_thickness.c
sway/sway/commands/titlebar_padding.c
sway/sway/commands/title_format.c
sway/sway/commands/unmark.c
sway/sway/commands/urgent.c
sway/sway/commands/workspace.c
sway/sway/commands/workspace_layout.c
sway/sway/commands/ws_auto_back_and_forth.c
sway/sway/commands/xwayland.c
sway/sway/config/seat.c
sway/sway/input/seat.c
sway/sway/input/seatop_default.c
sway/sway/input/seatop_down.c
sway/sway/input/seatop_move_floating.c
sway/sway/input/seatop_move_tiling.c
sway/sway/input/seatop_resize_floating.c
sway/sway/input/seatop_resize_tiling.c
sway/sway/tree/arrange.c
sway/sway/tree/container.c
sway/sway/tree/node.c
sway/sway/tree/output.c
sway/sway/tree/root.c
sway/sway/tree/view.c
sway/sway/tree/workspace.c
```


## 7. Notas Finales

Este documento proporciona una base sólida para implementar un tiling window manager
basado en niri, inspirado en i3/Sway, usando estructuras de datos modernas de Rust.

**Recuerda**:
- Empieza simple: implementa solo splits H/V primero
- Testea cada feature antes de continuar
- Usa el compositor de niri como base sólida
- Estudia `arrange.c` de Sway en detalle - es el corazón del layout

**Para el LLM que usará este contexto**:
- Este documento cubre las estructuras de datos y algoritmos fundamentales
- Los ejemplos de código son aproximaciones, adapta según necesidad
- SlotMap es la herramienta correcta para este trabajo en Rust
- Mantén la filosofía de i3: simple, eficiente, predecible

¡Buena suerte con la implementación!

---
*Documento generado el mié 24 dic 2025 13:51:37 CET*
*Script: generate_context.sh*
