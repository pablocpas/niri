//! i3-style container tree layout (replacing scrollable-tiling)
//!
//! This file now implements an i3-style hierarchical container tree instead of
//! the original scrollable tiling layout.
//!
//! Original scrollable-tiling backed up as: scrolling.rs.BACKUP

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
pub struct ScrollingSpace<W: LayoutElement> {
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
}

niri_render_elements! {
    ScrollingSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
    }
}

/// STUB: Simplified column structure
#[derive(Debug)]
pub struct Column<W: LayoutElement> {
    tiles: Vec<Tile<W>>,
    _phantom: std::marker::PhantomData<W>,
}

/// STUB: Column width enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnWidth {
    Proportion(f64),
    Fixed(i32),
}

/// STUB: Window height enum
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowHeight {
    Auto,
    Fixed(i32),
}

/// STUB: Scroll direction enum
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
    fn new(space: &'a ScrollingSpace<W>) -> Self {
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
    space: *mut ScrollingSpace<W>,
    layouts: Vec<LeafLayoutInfo>,
    index: usize,
    round: bool,
    scale: Scale<f64>,
    _marker: PhantomData<&'a mut ScrollingSpace<W>>,
}

impl<'a, W: LayoutElement> TileRenderPositionsMut<'a, W> {
    fn new(space: &'a mut ScrollingSpace<W>, round: bool) -> Self {
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
// STUB IMPLEMENTATIONS
// ============================================================================

impl<W: LayoutElement> ScrollingSpace<W> {
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
    ) -> Vec<ScrollingSpaceRenderElement<R>> {
        let mut elements = Vec::new();
        let mut active_elements = Vec::new();
        let scale = Scale::from(self.scale);
        let focus_path = self.tree.focus_path();

        for info in self.tree.leaf_layouts() {
            if !info.visible {
                continue;
            }

            if let Some(tile) = self.tree.tile_at_path(&info.path) {
                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);
                let draw_focus = scrolling_focus_ring && info.path == focus_path;

                let iter = tile.render(renderer, pos, draw_focus, target)
                    .map(ScrollingSpaceRenderElement::from);

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

        for info in layouts {
            if let Some(tile) = self.tree.tile_at_path_mut(&info.path) {
                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);

                let mut tile_view_rect = workspace_view;
                tile_view_rect.loc -= pos;

                Self::update_window_state(
                    tile,
                    &info,
                    &focus_path,
                    is_active,
                    self.options.deactivate_unfocused_windows,
                    self.working_area.size,
                    &self.options,
                );

                tile.update_render_elements(is_active && info.visible, tile_view_rect);
            }
        }
    }

    // STUB: Interactive resize
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
    pub fn activate_window(&mut self, _window: &W::Id) -> bool {
        // TODO: Implement window activation
        false
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

    // STUB: Size operations
    pub fn set_column_width(&mut self, _change: SizeChange) {}
    pub fn reset_window_height(&mut self, _window: Option<&W::Id>) {}

    // STUB: Fullscreen
    pub fn toggle_fullscreen(&mut self, _window: &W) {}
    pub fn toggle_width(&mut self, _forwards: bool) {}

    // STUB: View offset operations (removed for i3-conversion)
    pub(super) fn view_offset(&self) -> f64 {
        0.0
    }

    // STUB: Position queries
    pub(super) fn insert_position(&self, _pos: Point<f64, Logical>) -> InsertPosition {
        InsertPosition::NewColumn(0)
    }

    pub(super) fn insert_hint_area(
        &self,
        _position: InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        None
    }

    // STUB: Window queries
    pub fn window_under(&self, _pos: Point<f64, Logical>) -> Option<(&W, super::HitType)> {
        None
    }

    pub fn window_loc(&self, _window: &W) -> Option<Point<f64, Logical>> {
        None
    }

    pub fn window_size(&self, _window: &W) -> Option<Size<f64, Logical>> {
        None
    }

    pub fn is_fullscreen(&self, _window: &W) -> bool {
        false
    }

    // STUB: Column display
    pub fn set_column_display(&mut self, _display: ColumnDisplay) {}
pub fn toggle_column_tabbed_display(&mut self) {}

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
        let index = tile_idx.unwrap_or(col_idx);
        self.tree.insert_leaf_at(index, tile, activate);
        self.tree.layout();
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

    // STUB: Additional missing methods
    pub fn active_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        self.tree.focused_tile_mut()
    }

    pub fn add_column(
        &mut self,
        _col_idx: Option<usize>,
        mut column: Column<W>,
        activate: bool,
        _height: Option<WindowHeight>,
    ) {
        let len = column.tiles.len();
        for (idx, tile) in column.tiles.drain(..).enumerate() {
            let focus = activate && idx == len.saturating_sub(1);
            self.tree.append_leaf(tile, focus);
        }
        self.tree.layout();
    }
    pub fn remove_tile(&mut self, window: &W::Id, _transaction: Transaction) -> RemovedTile<W> {
        let tile = self
            .tree
            .remove_window(window)
            .expect("attempted to remove missing window");
        RemovedTile {
            tile,
            width: ColumnWidth::default(),
            is_full_width: false,
            is_floating: false,
        }
    }
    pub fn remove_active_tile(&mut self, transaction: Transaction) -> Option<RemovedTile<W>> {
        let id = self.tree.focused_tile()?.window().id().clone();
        Some(self.remove_tile(&id, transaction))
    }
    pub fn remove_active_column(&mut self) -> Option<Column<W>> { None }

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

    pub fn toggle_full_width(&mut self) {}
    pub fn toggle_window_height(&mut self, _window: Option<&W::Id>, _forwards: bool) {}
    pub fn toggle_window_width(&mut self, _window: Option<&W::Id>, _forwards: bool) {}
    pub fn set_window_width(&mut self, _window: Option<&W::Id>, _change: SizeChange) {}
    pub fn set_window_height(&mut self, _window: Option<&W::Id>, _change: SizeChange) {}

    pub fn set_fullscreen(&mut self, _window: &W::Id, _is_fullscreen: bool) -> bool { false }

    pub fn center_column(&mut self) {}
    pub fn center_window(&mut self, _window: Option<&W::Id>) {}
    pub fn center_visible_columns(&mut self) {}

    pub fn expand_column_to_available_width(&mut self) {}

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

impl<W: LayoutElement> ScrollingSpace<W> {
    fn update_window_state(
        tile: &mut Tile<W>,
        info: &LeafLayoutInfo,
        focus_path: &[usize],
        workspace_active: bool,
        deactivate_unfocused: bool,
        working_area_size: Size<f64, Logical>,
        options: &Options,
    ) {
        let window = tile.window_mut();

        let is_focused_tile = info.path == focus_path;
        let mut active = workspace_active && is_focused_tile;
        if deactivate_unfocused {
            active &= info.visible;
        }

        window.set_active_in_column(is_focused_tile);
        window.set_floating(false);
        window.set_activated(active);
        window.set_interactive_resize(None);

        let border_config = options.layout.border.merged_with(&window.rules().border);
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
        window.set_bounds(logical_bounds);

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

    pub fn tiles(&self) -> &[Tile<W>] {
        &self.tiles
    }

    pub fn tiles_mut(&mut self) -> &mut [Tile<W>] {
        &mut self.tiles
    }

    pub fn contains(&self, _window: &W) -> bool {
        false // TODO i3-conversion: Implement
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
