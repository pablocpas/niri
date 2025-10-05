//! i3-style hierarchical tiling layout
//!
//! This module implements an i3-style tiling window manager with hierarchical containers.
//! Windows are organized in a tree structure where:
//! - Internal nodes are containers with a layout mode (SplitH, SplitV, Tabbed, Stacked)
//! - Leaf nodes contain individual windows wrapped in Tiles
//! - Navigation and movement follow the tree hierarchy
//!
//! The implementation uses SlotMap for efficient O(1) node access and safe reference handling.

use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

use niri_config::utils::MergeWith as _;
use niri_config::{Border, PresetSize};
use niri_ipc::{ColumnDisplay, SizeChange};
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};

use super::container::{ContainerTree, Direction, Layout, LeafLayoutInfo};
use super::monitor::InsertPosition;
use super::tile::{Tile, TileRenderElement};
use super::{ConfigureIntent, LayoutElement, Options, RemovedTile};
use crate::animation::Clock;
use crate::niri_render_elements;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::RenderTarget;
use crate::utils::transaction::Transaction;
use crate::utils::ResizeEdge;
use crate::window::ResolvedWindowRules;

// ============================================================================
// MAIN STRUCTURES - i3-style container tree implementation
// ============================================================================

/// i3-style tiling space using hierarchical containers
#[derive(Debug)]
pub struct TilingSpace<W: LayoutElement> {
    /// Container tree managing window layout
    tree: ContainerTree<W>,
    /// View size (output size)
    view_size: Size<f64, Logical>,
    /// Working area (view_size minus gaps/bars)
    working_area: Rectangle<f64, Logical>,
    /// Display scale
    scale: f64,
    /// Animation clock
    clock: Clock,
    /// Layout options
    options: Rc<Options>,
    /// Currently fullscreen window (if any)
    fullscreen_window: Option<W::Id>,
}

niri_render_elements! {
    TilingSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
    }
}

/// Container wrapper representing a top-level column in the i3-style tree.
///
/// This holds a NodeKey that references a subtree in the ContainerTree.
/// The subtree is removed from the main tree and stored separately.
#[derive(Debug)]
pub struct Column<W: LayoutElement> {
    /// Temporary storage for extracted subtree
    /// This contains tiles that were removed from the main tree
    tiles: Vec<Tile<W>>,
    _phantom: std::marker::PhantomData<W>,
}

/// Column width specification for tiling layout
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    Proportion(f64),
    Fixed(i32),
}

/// Window height specification for tiling layout
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowHeight {
    Auto,
    Fixed(i32),
}

/// Direction for navigation and movement operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    Left,
    Right,
    Up,
    Down,
}

struct TileIter<'a, W: LayoutElement> {
    tiles: Vec<*const Tile<W>>,
    index: usize,
    _marker: PhantomData<&'a Tile<W>>,
}

impl<'a, W: LayoutElement> TileIter<'a, W> {
    fn new(tree: &'a ContainerTree<W>) -> Self {
        Self {
            tiles: tree.tile_ptrs(),
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, W: LayoutElement> Iterator for TileIter<'a, W> {
    type Item = &'a Tile<W>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.tiles.len() {
            return None;
        }

        let ptr = self.tiles[self.index];
        self.index += 1;

        unsafe { ptr.as_ref() }
    }
}

struct TileIterMut<'a, W: LayoutElement> {
    tiles: Vec<*mut Tile<W>>,
    index: usize,
    _marker: PhantomData<&'a mut Tile<W>>,
}

impl<'a, W: LayoutElement> TileIterMut<'a, W> {
    fn new(tree: &'a mut ContainerTree<W>) -> Self {
        let tiles = tree.tile_ptrs_mut();
        Self {
            tiles,
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, W: LayoutElement> Iterator for TileIterMut<'a, W> {
    type Item = &'a mut Tile<W>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.tiles.len() {
            return None;
        }

        let ptr = self.tiles[self.index];
        self.index += 1;

        unsafe { ptr.as_mut() }
    }
}

struct TileRenderPositions<'a, W: LayoutElement> {
    entries: Vec<(*const Tile<W>, Point<f64, Logical>, bool)>,
    index: usize,
    _marker: PhantomData<&'a Tile<W>>,
}

impl<'a, W: LayoutElement> TileRenderPositions<'a, W> {
    fn new(space: &'a TilingSpace<W>) -> Self {
        let scale = Scale::from(space.scale);
        let mut entries = Vec::new();

        for info in space.tree.leaf_layouts() {
            if let Some(tile) = space.tree.tile_at_path(&info.path) {
                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);
                entries.push((tile as *const _, pos, info.visible));
            }
        }

        Self {
            entries,
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, W: LayoutElement> Iterator for TileRenderPositions<'a, W> {
    type Item = (&'a Tile<W>, Point<f64, Logical>, bool);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.entries.len() {
            return None;
        }

        let (ptr, pos, visible) = self.entries[self.index];
        self.index += 1;

        unsafe { ptr.as_ref().map(|tile| (tile, pos, visible)) }
    }
}

struct TileRenderPositionsMut<'a, W: LayoutElement> {
    space: *mut TilingSpace<W>,
    layouts: Vec<LeafLayoutInfo>,
    index: usize,
    round: bool,
    scale: Scale<f64>,
    _marker: PhantomData<&'a mut TilingSpace<W>>,
}

impl<'a, W: LayoutElement> TileRenderPositionsMut<'a, W> {
    fn new(space: &'a mut TilingSpace<W>, round: bool) -> Self {
        let layouts = space.tree.leaf_layouts_cloned();
        Self {
            space: space as *mut _,
            layouts,
            index: 0,
            round,
            scale: Scale::from(space.scale),
            _marker: PhantomData,
        }
    }
}

impl<'a, W: LayoutElement> Iterator for TileRenderPositionsMut<'a, W> {
    type Item = (&'a mut Tile<W>, Point<f64, Logical>);

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < self.layouts.len() {
            let info = self.layouts[self.index].clone();
            self.index += 1;

            unsafe {
                let space = &mut *self.space;
                if let Some(tile) = space.tree.tile_at_path_mut(&info.path) {
                    let mut pos = info.rect.loc + tile.render_offset();
                    if self.round {
                        pos = pos.to_physical_precise_round(self.scale).to_logical(self.scale);
                    }
                    return Some((tile, pos));
                }
            }
        }

        None
    }
}

// ============================================================================
// TilingSpace Implementation
// ============================================================================

impl<W: LayoutElement> TilingSpace<W> {
    fn available_span(&self, total: f64, child_count: usize) -> f64 {
        if child_count == 0 {
            return 0.0;
        }
        let gap = self.options.layout.gaps;
        (total - gap * (child_count as f64 - 1.0)).max(0.0)
    }

    fn percent_from_size_change(current_percent: f64, available: f64, change: SizeChange) -> f64 {
        if available <= 0.0 {
            return current_percent;
        }

        let to_proportion = |value: f64| {
            if value.abs() > 1.0 {
                value / 100.0
            } else {
                value
            }
        };

        let percent = match change {
            SizeChange::SetFixed(px) => px as f64 / available,
            SizeChange::AdjustFixed(delta) => current_percent + (delta as f64 / available),
            SizeChange::SetProportion(prop) => to_proportion(prop),
            SizeChange::AdjustProportion(delta) => current_percent + to_proportion(delta),
        };

        percent.clamp(0.0, 1.0)
    }

    fn resolve_preset_dimension(available: f64, preset: PresetSize) -> f64 {
        match preset {
            PresetSize::Proportion(prop) => {
                let proportion = if prop.abs() > 1.0 {
                    (prop / 100.0).clamp(0.0, 1.0)
                } else {
                    prop.clamp(0.0, 1.0)
                };
                available * proportion
            }
            PresetSize::Fixed(px) => px as f64,
        }
    }

    fn cycle_presets(
        &self,
        available: f64,
        current_percent: f64,
        presets: &[PresetSize],
        forwards: bool,
    ) -> Option<f64> {
        if presets.is_empty() || available <= 0.0 {
            return None;
        }

        let resolved: Vec<f64> = presets
            .iter()
            .map(|preset| Self::resolve_preset_dimension(available, *preset))
            .collect();

        if resolved.is_empty() {
            return None;
        }

        let epsilon = 0.5;
        let current_width = current_percent * available;

        let target_width = if forwards {
            resolved
                .iter()
                .copied()
                .find(|width| *width > current_width + epsilon)
                .unwrap_or_else(|| resolved[0])
        } else {
            resolved
                .iter()
                .copied()
                .rev()
                .find(|width| *width + epsilon < current_width)
                .unwrap_or_else(|| *resolved.last().unwrap())
        };

        Some((target_width / available).clamp(0.0, 1.0))
    }

    fn window_path(&self, window: Option<&W::Id>) -> Option<Vec<usize>> {
        if let Some(id) = window {
            self.tree.find_window(id)
        } else if self.tree.focus_path().is_empty() {
            self.tree
                .focused_window()
                .is_some()
                .then(|| self.tree.focus_path().to_vec())
        } else {
            Some(self.tree.focus_path().to_vec())
        }
    }

    fn window_container_metrics(
        &self,
        path: &[usize],
        layout: Layout,
    ) -> Option<(Vec<usize>, usize, f64, usize, Rectangle<f64, Logical>)> {
        let (parent_path, child_idx) = self.tree.find_parent_with_layout(path.to_vec(), layout)?;
        let (container_layout, rect, child_count) = self.tree.container_info(parent_path.as_slice())?;
        if container_layout != layout || child_count == 0 {
            return None;
        }

        let available = match layout {
            Layout::SplitH => self.available_span(rect.size.w, child_count),
            Layout::SplitV => self.available_span(rect.size.h, child_count),
            Layout::Tabbed | Layout::Stacked => return None,
        };

        if available <= 0.0 {
            return None;
        }

        Some((parent_path, child_idx, available, child_count, rect))
    }
    pub fn new(
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        clock: Clock,
        options: Rc<Options>,
    ) -> Self {
        let tree = ContainerTree::new(
            view_size,
            working_area,
            scale,
            options.clone(),
        );

        Self {
            tree,
            view_size,
            working_area,
            scale,
            clock,
            options,
            fullscreen_window: None,
        }
    }

    // Basic getters using ContainerTree
    pub fn windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.tree.all_windows().into_iter()
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        TileIter::new(&self.tree)
    }

    pub fn active_tile(&self) -> Option<&Tile<W>> {
        self.tree.focused_tile()
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        self.tree.focused_window_mut()
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        self.tree
            .focused_tile()
            .map_or(false, |tile| tile.window().is_pending_fullscreen())
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn parent_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    pub fn options(&self) -> &Rc<Options> {
        &self.options
    }

    pub fn verify_invariants(&self) {
        debug_assert!(
            self.tree.leaf_layouts().len() <= self.tree.window_count(),
            "cached leaf layouts exceed window count"
        );
    }

    // Window management using ContainerTree
    pub fn add_window(&mut self, window: W, _rules: ResolvedWindowRules, _width: ColumnWidth, _height: WindowHeight) {
        // Create a tile for the window
        let tile = Tile::new(
            window,
            self.view_size,
            self.scale,
            self.clock.clone(),
            self.options.clone(),
        );
        // Insert into container tree
        self.tree.insert_window(tile);
        // Recalculate layout
        self.tree.layout();
    }

    pub fn remove_window(&mut self, window: &W) -> Option<RemovedTile<W>> {
        let window_id = window.id();
        let tile = self.tree.remove_window(&window_id)?;

        if self
            .fullscreen_window
            .as_ref()
            .is_some_and(|id| id == window_id)
        {
            self.fullscreen_window = None;
        }

        // Create RemovedTile
        Some(RemovedTile {
            tile,
            width: ColumnWidth::default(),
            is_full_width: false,
            is_floating: false,
        })
    }

    pub fn update_window(&mut self, _window: &W::Id, _serial: Option<smithay::utils::Serial>) {
        // TODO: Implement window updates
    }

    pub fn find_window(&self, window: &W) -> Option<(usize, usize)> {
        // Return dummy indices for compatibility
        // In i3 model, we don't use column/tile indices
        let window_id = window.id();
        self.tree.find_window(&window_id).map(|_| (0, 0))
    }

    pub fn render_elements<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        scrolling_focus_ring: bool,
    ) -> Vec<TilingSpaceRenderElement<R>> {
        let mut elements = Vec::new();
        let mut active_elements = Vec::new();
        let scale = Scale::from(self.scale);
        let focus_path = self.tree.focus_path();
        let fullscreen_id = self.fullscreen_window.as_ref();

        for info in self.tree.leaf_layouts().iter().rev() {
            if let Some(tile) = self.tree.tile_at_path(&info.path) {
                let is_fullscreen_tile = fullscreen_id
                    .is_some_and(|id| id == tile.window().id());
                let show_tile = fullscreen_id.map_or(info.visible, |_| is_fullscreen_tile);

                if !show_tile {
                    continue;
                }

                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);
                if is_fullscreen_tile {
                    pos = Point::from((0.0, 0.0));
                }
                if is_fullscreen_tile {
                    pos = Point::from((0.0, 0.0));
                }

                let draw_focus = scrolling_focus_ring && info.path == focus_path;

                let iter = tile
                    .render(renderer, pos, draw_focus, target)
                    .map(TilingSpaceRenderElement::from);

                if info.path == focus_path {
                    active_elements.extend(iter);
                } else {
                    elements.extend(iter);
                }
            }
        }

        elements.extend(active_elements);
        elements
    }

    // Layout operations using ContainerTree
    pub fn update_config(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) {
        self.view_size = view_size;
        self.working_area = working_area;
        self.scale = scale;
        self.options = options.clone();
        self.tree.update_config(view_size, working_area, scale, options);
        self.tree.layout();
    }

    pub fn set_view_size(&mut self, view_size: Size<f64, Logical>, working_area: Rectangle<f64, Logical>) {
        self.view_size = view_size;
        self.working_area = working_area;
        self.tree.set_view_size(view_size, working_area);
        // Recalculate layout on resize
        self.tree.layout();
    }

    pub fn advance_animations(&mut self) {
        for tile in TileIterMut::new(&mut self.tree) {
            tile.advance_animations();
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        TileIter::new(&self.tree).any(|tile| tile.are_animations_ongoing())
    }

    pub fn update_render_elements(&mut self, is_active: bool) {
        let layouts = self.tree.leaf_layouts_cloned();
        let workspace_view = Rectangle::from_size(self.view_size);
        let focus_path = self.tree.focus_path().to_vec();
        let scale = Scale::from(self.scale);
        let fullscreen_id = self.fullscreen_window.as_ref();

        for info in layouts {
            if let Some(tile) = self.tree.tile_at_path_mut(&info.path) {
                let is_fullscreen_tile = fullscreen_id
                    .is_some_and(|id| id == tile.window().id());

                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);

                let mut tile_view_rect = workspace_view;
                tile_view_rect.loc -= pos;

                if is_fullscreen_tile {
                    tile_view_rect = workspace_view;
                }

                Self::update_window_state(
                    tile,
                    &info,
                    &focus_path,
                    is_active,
                    self.options.deactivate_unfocused_windows,
                    self.working_area.size,
                    &self.options,
                    fullscreen_id,
                    self.view_size,
                );

                let show_tile = fullscreen_id.map_or(info.visible, |_| is_fullscreen_tile);
                if show_tile {
                    let render_active = is_active && (info.visible || is_fullscreen_tile);
                    tile.update_render_elements(render_active, tile_view_rect);
                }
            }
        }
    }

    // Interactive resize - not implemented for i3-style tiling
    // In i3, window sizing is done via keyboard commands, not interactive mouse resize
    pub fn interactive_resize_begin(&mut self, _window: W::Id, _edges: ResizeEdge) -> bool {
        false
    }

    pub fn interactive_resize_update(
        &mut self,
        _window: &W::Id,
        _delta: Point<f64, Logical>,
    ) -> bool {
        false
    }

    pub fn interactive_resize_end(&mut self, _window: Option<&W::Id>) {}

    pub fn cancel_resize_for_window(&mut self, _window: &W) {}

    pub fn resize_edges_under(&self, _pos: Point<f64, Logical>) -> Option<ResizeEdge> {
        None
    }

    // Focus operations using ContainerTree
    pub fn activate_window(&mut self, window: &W::Id) -> bool {
        if self.tree.focus_window_by_id(window) {
            self.tree.layout();
            true
        } else {
            false
        }
    }

    pub fn focus_left(&mut self) -> bool {
        self.tree.focus_in_direction(Direction::Left)
    }

    pub fn focus_right(&mut self) -> bool {
        self.tree.focus_in_direction(Direction::Right)
    }

    pub fn focus_down(&mut self) -> bool {
        self.tree.focus_in_direction(Direction::Down)
    }

    pub fn focus_up(&mut self) -> bool {
        self.tree.focus_in_direction(Direction::Up)
    }

    pub fn focus_parent(&mut self) -> bool {
        self.tree.focus_parent()
    }

    pub fn focus_child(&mut self) -> bool {
        self.tree.focus_child()
    }

    // Move operations using ContainerTree
    pub fn move_left(&mut self) -> bool {
        let result = self.tree.move_in_direction(Direction::Left);
        if result {
            self.tree.layout();
        }
        result
    }

    pub fn move_right(&mut self) -> bool {
        let result = self.tree.move_in_direction(Direction::Right);
        if result {
            self.tree.layout();
        }
        result
    }

    pub fn move_down(&mut self) -> bool {
        let result = self.tree.move_in_direction(Direction::Down);
        if result {
            self.tree.layout();
        }
        result
    }

    pub fn move_up(&mut self) -> bool {
        let result = self.tree.move_in_direction(Direction::Up);
        if result {
            self.tree.layout();
        }
        result
    }

    // Container operations (replacing column operations)
    pub fn consume_into_column(&mut self) {
        // In i3 model: create vertical split
        self.tree.split_focused(Layout::SplitV);
        self.tree.layout();
    }

    pub fn expel_from_column(&mut self) {
        // In i3 model: create horizontal split
        self.tree.split_focused(Layout::SplitH);
        self.tree.layout();
    }

    /// Split focused window horizontally (i3-style)
    pub fn split_horizontal(&mut self) {
        self.tree.split_focused(Layout::SplitH);
        self.tree.layout();
    }

    /// Split focused window vertically (i3-style)
    pub fn split_vertical(&mut self) {
        self.tree.split_focused(Layout::SplitV);
        self.tree.layout();
    }

    /// Set layout mode for focused container
    pub fn set_layout_mode(&mut self, layout: Layout) {
        self.tree.set_focused_layout(layout);
        self.tree.layout();
    }

    /// Set the width of the currently focused root-level column
    pub fn set_column_width(&mut self, change: SizeChange) {
        let Some(idx) = self.tree.focused_root_index() else {
            return;
        };

        let Some((layout, rect, child_count)) = self.tree.container_info(&[]) else {
            return;
        };
        if layout != Layout::SplitH || child_count == 0 {
            return;
        }

        let gaps = self.options.layout.gaps;
        let available_width = (rect.size.w - gaps * (child_count as f64 - 1.0)).max(1.0);
        if available_width <= 0.0 {
            return;
        }

        let current_percent = self.tree.child_percent_at(&[], idx).unwrap_or(1.0);
        let new_percent = Self::percent_from_size_change(current_percent, available_width, change);

        if self
            .tree
            .set_child_percent_at(&[], idx, Layout::SplitH, new_percent)
        {
            self.tree.layout();
        }
    }
    pub fn reset_window_height(&mut self, window: Option<&W::Id>) {
        let Some(path) = self.window_path(window) else {
            return;
        };

        let Some((parent_path, _, _, _child_count, _rect)) =
            self.window_container_metrics(&path, Layout::SplitV)
        else {
            return;
        };

        if let Some(container) = self.tree.container_at_path_mut(parent_path.as_slice()) {
            if container.layout() == Layout::SplitV {
                container.recalculate_percentages();
                self.tree.layout();
            }
        }
    }

    /// Toggle fullscreen state for a window
    pub fn toggle_fullscreen(&mut self, window: &W) {
        let currently = self.is_fullscreen(window);
        let _ = self.set_fullscreen(window.id(), !currently);
    }
    pub fn toggle_width(&mut self, forwards: bool) {
        let Some(idx) = self.tree.focused_root_index() else {
            return;
        };

        let Some((layout, rect, child_count)) = self.tree.container_info(&[]) else {
            return;
        };
        if layout != Layout::SplitH || child_count == 0 {
            return;
        }

        let available = self.available_span(rect.size.w, child_count);
        if available <= 0.0 {
            return;
        }

        let current_percent = self.tree.child_percent_at(&[], idx).unwrap_or(1.0);
        let presets = &self.options.layout.preset_column_widths;

        if let Some(percent) = self.cycle_presets(available, current_percent, presets, forwards) {
            if self
                .tree
                .set_child_percent_at(&[], idx, Layout::SplitH, percent)
            {
                self.tree.layout();
            }
        }
    }

    /// View offset (not used in i3-style layout, always 0)
    pub(super) fn view_offset(&self) -> f64 {
        0.0
    }

    /// Determine insert position from pointer location
    pub(super) fn insert_position(&self, _pos: Point<f64, Logical>) -> InsertPosition {
        InsertPosition::NewColumn(0)
    }

    /// Get hint area for insertion position
    pub(super) fn insert_hint_area(
        &self,
        _position: InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        None
    }

    // Window queries
    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, super::HitType)> {
        let scale = Scale::from(self.scale);
        let fullscreen_id = self.fullscreen_window.as_ref();

        for info in self.tree.leaf_layouts().iter().rev() {
            if let Some(tile) = self.tree.tile_at_path(&info.path) {
                let is_fullscreen_tile = fullscreen_id
                    .is_some_and(|id| id == tile.window().id());
                if fullscreen_id.is_some() && !is_fullscreen_tile {
                    continue;
                }
                if !info.visible && !is_fullscreen_tile {
                    continue;
                }

                let mut tile_pos = info.rect.loc + tile.render_offset();
                tile_pos = tile_pos
                    .to_physical_precise_round(scale)
                    .to_logical(scale);

                if let Some(hit) = super::HitType::hit_tile(tile, tile_pos, pos) {
                    return Some(hit);
                }
            }
        }

        None
    }

    pub fn window_loc(&self, window: &W) -> Option<Point<f64, Logical>> {
        let path = self.tree.find_window(window.id())?;
        let info = self
            .tree
            .leaf_layouts()
            .iter()
            .find(|layout| layout.path == path)?;
        let tile = self.tree.tile_at_path(&path)?;
        let scale = Scale::from(self.scale);

        let mut tile_pos = info.rect.loc + tile.render_offset();
        tile_pos = tile_pos
            .to_physical_precise_round(scale)
            .to_logical(scale);

        Some(tile_pos + tile.window_loc())
    }

    pub fn window_size(&self, window: &W) -> Option<Size<f64, Logical>> {
        let path = self.tree.find_window(window.id())?;
        let tile = self.tree.tile_at_path(&path)?;
        Some(tile.window_size())
    }

    pub fn is_fullscreen(&self, window: &W) -> bool {
        self.fullscreen_window
            .as_ref()
            .is_some_and(|id| id == window.id())
    }

    /// Set the display mode for the focused container
    pub fn set_column_display(&mut self, display: ColumnDisplay) {
        let layout = match display {
            ColumnDisplay::Normal => Layout::SplitV,
            ColumnDisplay::Tabbed => Layout::Tabbed,
        };

        if self.tree.set_focused_layout(layout) {
            self.tree.layout();
        }
    }

    /// Toggle between tabbed and normal (split) layout for focused container
    pub fn toggle_column_tabbed_display(&mut self) {
        let current = self.tree.focused_layout();
        let target = match current {
            Some(Layout::Tabbed) => Layout::SplitV,
            _ => Layout::Tabbed,
        };

        if self.tree.set_focused_layout(target) {
            self.tree.layout();
        }
    }

    // Additional methods needed by workspace.rs
    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        TileIterMut::new(&mut self.tree)
    }

    pub fn tiles_with_render_positions(&self) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> + '_ {
        TileRenderPositions::new(self)
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> + '_ {
        TileRenderPositionsMut::new(self, round)
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, niri_ipc::WindowLayout)> + '_ {
        let scale = Scale::from(self.scale);

        self.tree
            .leaf_layouts()
            .iter()
            .enumerate()
            .filter_map(move |(idx, info)| {
                let tile = self.tree.tile_at_path(&info.path)?;
                let mut layout = tile.ipc_layout_template();
                let tile_size = tile.tile_size();
                layout.tile_size = (tile_size.w, tile_size.h);
                let window_size = tile.window_size().to_i32_round();
                layout.window_size = (window_size.w, window_size.h);
                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);
                layout.tile_pos_in_workspace_view = Some((pos.x, pos.y));
                let window_offset = tile.window_loc();
                layout.window_offset_in_tile = (window_offset.x, window_offset.y);
                layout.pos_in_scrolling_layout = Some((idx + 1, 1));
                Some((tile, layout))
            })
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        TileIter::new(&self.tree).any(|tile| tile.are_transitions_ongoing())
    }

    pub fn update_shaders(&mut self) {
        for tile in TileIterMut::new(&mut self.tree) {
            tile.update_shaders();
        }
    }

    pub fn active_window(&self) -> Option<&W> {
        self.tree.focused_window()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn add_tile(
        &mut self,
        col_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
        _width: ColumnWidth,
        _is_full_width: bool,
        _height: Option<WindowHeight>,
    ) {
        if let Some(index) = col_idx {
            self.tree.insert_leaf_at(index, tile, activate);
        } else {
            self.tree.append_leaf(tile, activate);
        }
        self.tree.layout();
    }

    pub fn add_tile_right_of(
        &mut self,
        next_to: &W::Id,
        tile: Tile<W>,
        activate: bool,
        _width: ColumnWidth,
        _is_full_width: bool,
    ) {
        self.tree.insert_leaf_after(next_to, tile, activate);
        self.tree.layout();
    }

    pub fn add_tile_to_column(
        &mut self,
        col_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        activate: bool,
    ) {
        if self
            .tree
            .insert_leaf_in_column(col_idx, tile_idx, tile, activate)
        {
            self.tree.layout();
        }
    }

    pub fn active_tile_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        let focus_path = self.tree.focus_path();
        self.tree
            .leaf_layouts()
            .iter()
            .find(|info| info.path == focus_path)
            .and_then(|info| {
                let mut rect = info.rect;
                let tile = self.tree.tile_at_path(&info.path)?;
                rect.loc += tile.render_offset();
                Some(rect)
            })
    }

    /// Get mutable reference to the currently focused tile
    pub fn active_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        self.tree.focused_tile_mut()
    }

    pub fn add_column(
        &mut self,
        _col_idx: Option<usize>,
        column: Column<W>,
        activate: bool,
        _height: Option<WindowHeight>,
    ) {
        let idx = _col_idx.unwrap_or_else(|| self.tree.root_children_len());
        let tiles = column.into_tiles();
        self.tree.insert_tiles_at_root(idx, tiles, activate);
        self.tree.layout();
    }
    pub fn remove_tile(&mut self, window: &W::Id, _transaction: Transaction) -> RemovedTile<W> {
        let tile = self
            .tree
            .remove_window(window)
            .expect("attempted to remove missing window");

        if self
            .fullscreen_window
            .as_ref()
            .is_some_and(|id| id == window)
        {
            self.fullscreen_window = None;
        }

        RemovedTile {
            tile,
            width: ColumnWidth::default(),
            is_full_width: false,
            is_floating: false,
        }
    }
    pub fn remove_active_tile(&mut self, transaction: Transaction) -> Option<RemovedTile<W>> {
        let id = self.tree.focused_tile()?.window().id().clone();
        let removed = self.remove_tile(&id, transaction);
        if self
            .fullscreen_window
            .as_ref()
            .is_some_and(|win_id| win_id == &id)
        {
            self.fullscreen_window = None;
        }
        Some(removed)
    }
    pub fn remove_active_column(&mut self) -> Option<Column<W>> {
        let idx = self.tree.focused_root_index()?;
        let tiles = self.tree.take_root_child_tiles(idx)?;
        let column = Column::from_tiles(tiles);

        if let Some(full_id) = self.fullscreen_window.clone() {
            if self.tree.find_window(&full_id).is_none() {
                self.fullscreen_window = None;
            }
        }

        self.tree.layout();
        Some(column)
    }

    pub fn new_window_size(&self, _width: Option<PresetSize>, _height: Option<PresetSize>, _rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        Size::from((800, 600))
    }

    pub fn new_window_toplevel_bounds(&self, _rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        Size::from((800, 600))
    }

    pub fn focus_column_first(&mut self) {
        self.tree.focus_root_child(0);
    }

    pub fn focus_column_last(&mut self) {
        let len = self.tree.root_children_len();
        if len > 0 {
            self.tree.focus_root_child(len - 1);
        }
    }

    /// Columns are 1-based to match user-facing commands.
    pub fn focus_column(&mut self, idx: usize) {
        if idx == 0 {
            return;
        }
        self.tree.focus_root_child(idx - 1);
    }

    /// Windows inside the current column are 1-based.
    pub fn focus_window_in_column(&mut self, index: u8) {
        if index == 0 {
            return;
        }
        let column_idx = match self.tree.focused_root_index() {
            Some(idx) => idx,
            None => return,
        };
        self.tree
            .focus_leaf_in_root_child(column_idx, index as usize);
    }

    pub fn focus_down_or_left(&mut self) {
        if !self.tree.focus_in_direction(Direction::Down) {
            self.tree.focus_in_direction(Direction::Left);
        }
    }

    pub fn focus_down_or_right(&mut self) {
        if !self.tree.focus_in_direction(Direction::Down) {
            self.tree.focus_in_direction(Direction::Right);
        }
    }

    pub fn focus_up_or_left(&mut self) {
        if !self.tree.focus_in_direction(Direction::Up) {
            self.tree.focus_in_direction(Direction::Left);
        }
    }

    pub fn focus_up_or_right(&mut self) {
        if !self.tree.focus_in_direction(Direction::Up) {
            self.tree.focus_in_direction(Direction::Right);
        }
    }

    pub fn focus_top(&mut self) {
        self.tree.focus_top_in_current_column();
    }

    pub fn focus_bottom(&mut self) {
        self.tree.focus_bottom_in_current_column();
    }

    pub fn move_column_to_first(&mut self) {
        if let Some(idx) = self.tree.focused_root_index() {
            if self.tree.move_root_child(idx, 0) {
                self.tree.layout();
            }
        }
    }

    pub fn move_column_to_last(&mut self) {
        let len = self.tree.root_children_len();
        if len == 0 {
            return;
        }
        if let Some(idx) = self.tree.focused_root_index() {
            if self.tree.move_root_child(idx, len - 1) {
                self.tree.layout();
            }
        }
    }

    pub fn move_column_left(&mut self) -> bool {
        let Some(idx) = self.tree.focused_root_index() else {
            return false;
        };
        if idx == 0 {
            return false;
        }

        let moved = self.tree.move_root_child(idx, idx - 1);
        if moved {
            self.tree.layout();
        }
        moved
    }

    pub fn move_column_right(&mut self) -> bool {
        let Some(idx) = self.tree.focused_root_index() else {
            return false;
        };
        let len = self.tree.root_children_len();
        if idx + 1 >= len {
            return false;
        }

        let moved = self.tree.move_root_child(idx, idx + 1);
        if moved {
            self.tree.layout();
        }
        moved
    }

    pub fn move_column_to_index(&mut self, idx: usize) {
        if idx == 0 {
            return;
        }
        let target = idx - 1;
        if let Some(current) = self.tree.focused_root_index() {
            if current == target {
                return;
            }
            let len = self.tree.root_children_len();
            if target >= len {
                return;
            }
            if self.tree.move_root_child(current, target) {
                self.tree.layout();
            }
        }
    }

    pub fn consume_or_expel_window_left(&mut self, window: Option<&W::Id>) {
        if let Some(id) = window {
            self.tree.focus_window_by_id(id);
        }

        if self.tree.move_in_direction(Direction::Left) {
            self.tree.layout();
        } else {
            self.tree.split_focused(Layout::SplitV);
            self.tree.layout();
        }
    }

    pub fn consume_or_expel_window_right(&mut self, window: Option<&W::Id>) {
        if let Some(id) = window {
            self.tree.focus_window_by_id(id);
        }

        if self.tree.move_in_direction(Direction::Right) {
            self.tree.layout();
        } else {
            self.tree.split_focused(Layout::SplitV);
            self.tree.layout();
        }
    }

    pub fn toggle_full_width(&mut self) {
        let Some(tile) = self.tree.focused_tile() else {
            return;
        };
        let id = tile.window().id().clone();
        let currently_fullscreen = self
            .fullscreen_window
            .as_ref()
            .is_some_and(|win_id| win_id == tile.window().id());
        let _ = self.set_fullscreen(&id, !currently_fullscreen);
    }
    pub fn toggle_window_height(&mut self, window: Option<&W::Id>, forwards: bool) {
        let Some(path) = self.window_path(window) else {
            return;
        };
        let Some((parent_path, child_idx, available, _, _)) =
            self.window_container_metrics(&path, Layout::SplitV)
        else {
            return;
        };
        let current_percent = self
            .tree
            .child_percent_at(parent_path.as_slice(), child_idx)
            .unwrap_or(1.0);

        if let Some(percent) = self.cycle_presets(
            available,
            current_percent,
            &self.options.layout.preset_window_heights,
            forwards,
        ) {
            if self.tree.set_child_percent_at(
                parent_path.as_slice(),
                child_idx,
                Layout::SplitV,
                percent,
            ) {
                self.tree.layout();
            }
        }
    }

    pub fn toggle_window_width(&mut self, window: Option<&W::Id>, forwards: bool) {
        let Some(path) = self.window_path(window) else {
            return;
        };
        let Some((parent_path, child_idx, available, _, _)) =
            self.window_container_metrics(&path, Layout::SplitH)
        else {
            return;
        };
        let current_percent = self
            .tree
            .child_percent_at(parent_path.as_slice(), child_idx)
            .unwrap_or(1.0);

        if let Some(percent) = self.cycle_presets(
            available,
            current_percent,
            &self.options.layout.preset_column_widths,
            forwards,
        ) {
            if self.tree.set_child_percent_at(
                parent_path.as_slice(),
                child_idx,
                Layout::SplitH,
                percent,
            ) {
                self.tree.layout();
            }
        }
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        let Some(path) = self.window_path(window) else {
            return;
        };
        let Some((parent_path, child_idx, available, _, _)) =
            self.window_container_metrics(&path, Layout::SplitH)
        else {
            return;
        };

        let current_percent = self
            .tree
            .child_percent_at(parent_path.as_slice(), child_idx)
            .unwrap_or(1.0);
        let percent = Self::percent_from_size_change(current_percent, available, change);

        if self.tree.set_child_percent_at(
            parent_path.as_slice(),
            child_idx,
            Layout::SplitH,
            percent,
        ) {
            self.tree.layout();
        }
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        let Some(path) = self.window_path(window) else {
            return;
        };
        let Some((parent_path, child_idx, available, _, _)) =
            self.window_container_metrics(&path, Layout::SplitV)
        else {
            return;
        };

        let current_percent = self
            .tree
            .child_percent_at(parent_path.as_slice(), child_idx)
            .unwrap_or(1.0);
        let percent = Self::percent_from_size_change(current_percent, available, change);

        if self.tree.set_child_percent_at(
            parent_path.as_slice(),
            child_idx,
            Layout::SplitV,
            percent,
        ) {
            self.tree.layout();
        }
    }

    pub fn set_fullscreen(&mut self, window: &W::Id, is_fullscreen: bool) -> bool {
        if is_fullscreen {
            if self
                .fullscreen_window
                .as_ref()
                .is_some_and(|id| id == window)
            {
                return false;
            }

            if !self.tree.focus_window_by_id(window) {
                return false;
            }

            self.fullscreen_window = Some(window.clone());
            self.tree.layout();
            true
        } else {
            if self
                .fullscreen_window
                .as_ref()
                .is_some_and(|id| id == window)
            {
                self.fullscreen_window = None;
                self.tree.layout();
                true
            } else {
                false
            }
        }
    }

    pub fn center_column(&mut self) {}
    pub fn center_window(&mut self, _window: Option<&W::Id>) {}
    pub fn center_visible_columns(&mut self) {}

    pub fn expand_column_to_available_width(&mut self) {
        let Some(idx) = self.tree.focused_root_index() else {
            return;
        };
        if self
            .tree
            .set_child_percent_at(&[], idx, Layout::SplitH, 1.0)
        {
            self.tree.layout();
        }
    }

    pub fn swap_window_in_direction(&mut self, _direction: ScrollDirection) {}

    pub fn start_open_animation(&mut self, _id: &W::Id) -> bool { false }
    pub fn start_close_animation_for_window<R: NiriRenderer>(
        &mut self,
        _renderer: &mut R,
        _window: &W::Id,
        _blocker: crate::utils::transaction::TransactionBlocker,
    ) {}

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        let layouts = self.tree.leaf_layouts_cloned();
        let focus_path = self.tree.focus_path().to_vec();
        let fullscreen_id = self.fullscreen_window.as_ref();

        for info in layouts {
            if let Some(tile) = self.tree.tile_at_path_mut(&info.path) {
                let deactivate_unfocused = self.options.deactivate_unfocused_windows && !is_focused;

                Self::update_window_state(
                    tile,
                    &info,
                    &focus_path,
                    is_active,
                    deactivate_unfocused,
                    self.working_area.size,
                    &self.options,
                    fullscreen_id,
                    self.view_size,
                );
            }
        }
    }
    pub fn render_above_top_layer(&self) -> bool { false }

    pub fn scroll_amount_to_activate(&self, _window: &W::Id) -> f64 { 0.0 }

    pub fn popup_target_rect(&self, _window: &W::Id) -> Option<Rectangle<f64, Logical>> { None }

    pub fn view_offset_gesture_begin(&mut self, _is_touchpad: bool) {}
    pub fn view_offset_gesture_update(&mut self, _delta: f64, _timestamp: Duration, _is_touchpad: bool) -> Option<bool> {
        None
    }
    pub fn view_offset_gesture_end(&mut self, _cancelled: Option<bool>) -> bool {
        false
    }

    pub fn dnd_scroll_gesture_begin(&mut self) {}
    pub fn dnd_scroll_gesture_scroll(&mut self, _delta: f64) -> bool { false }
    pub fn dnd_scroll_gesture_end(&mut self) {}
}

impl<W: LayoutElement> TilingSpace<W> {
    fn update_window_state(
        tile: &mut Tile<W>,
        info: &LeafLayoutInfo,
        focus_path: &[usize],
        workspace_active: bool,
        deactivate_unfocused: bool,
        working_area_size: Size<f64, Logical>,
        options: &Options,
        fullscreen_id: Option<&W::Id>,
        view_size: Size<f64, Logical>,
    ) {
        let window_id = tile.window().id().clone();
        let is_focused_tile = info.path == focus_path;
        let is_fullscreen_tile = fullscreen_id.is_some_and(|id| id == &window_id);

        let target_size: Size<f64, Logical> = if is_fullscreen_tile {
            view_size
        } else {
            Size::from((info.rect.size.w, info.rect.size.h))
        };
        tile.request_tile_size(target_size, false, None);

        let window = tile.window_mut();

        let mut active = workspace_active && is_focused_tile;

        if fullscreen_id.is_some() && !is_fullscreen_tile {
            active = false;
        } else if deactivate_unfocused {
            active &= info.visible;
        }

        let active_in_column = is_focused_tile && (fullscreen_id.is_none() || is_fullscreen_tile);

        window.set_active_in_column(active_in_column);
        window.set_floating(false);
        window.set_activated(active);
        window.set_interactive_resize(None);

        let border_config = options.layout.border.merged_with(&window.rules().border);

        let bounds = if is_fullscreen_tile {
            view_size.to_i32_floor()
        } else {
            let max_bounds = compute_toplevel_bounds(
                border_config,
                working_area_size,
                Size::from((0.0, 0.0)),
                options.layout.gaps,
            );
            let mut logical_bounds: Size<i32, Logical> =
                Size::from((info.rect.size.w, info.rect.size.h)).to_i32_floor();
            logical_bounds.w = logical_bounds.w.min(max_bounds.w);
            logical_bounds.h = logical_bounds.h.min(max_bounds.h);
            logical_bounds
        };

        window.set_bounds(bounds);

        match window.configure_intent() {
            ConfigureIntent::CanSend | ConfigureIntent::ShouldSend => {
                window.send_pending_configure();
            }
            _ => {}
        }

        window.refresh();
    }
}

impl<W: LayoutElement> Column<W> {
    pub fn new(tile: Tile<W>) -> Self {
        Self {
            tiles: vec![tile],
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn from_tiles(tiles: Vec<Tile<W>>) -> Self {
        Self {
            tiles,
            _phantom: std::marker::PhantomData,
        }
    }

    pub fn tiles(&self) -> Vec<&Tile<W>> {
        self.tiles.iter().collect()
    }

    pub fn contains(&self, window: &W) -> bool {
        let target_id = window.id();
        self.tiles.iter().any(|tile| tile.window().id() == target_id)
    }

    pub fn into_tiles(self) -> Vec<Tile<W>> {
        self.tiles
    }
}

impl Default for ColumnWidth {
    fn default() -> Self {
        Self::Proportion(1.0)
    }
}

impl Default for WindowHeight {
    fn default() -> Self {
        Self::Auto
    }
}

fn compute_toplevel_bounds(
    border_config: Border,
    working_area_size: Size<f64, Logical>,
    extra_size: Size<f64, Logical>,
    gaps: f64,
) -> Size<i32, Logical> {
    let mut border = 0.0;
    if !border_config.off {
        border = border_config.width * 2.0;
    }

    Size::from((
        f64::max(working_area_size.w - gaps * 2.0 - extra_size.w - border, 1.0),
        f64::max(working_area_size.h - gaps * 2.0 - extra_size.h - border, 1.0),
    ))
    .to_i32_floor()
}
