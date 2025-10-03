//! STUB for i3-conversion: This file originally implemented scrollable-tiling layout.
//! It has been replaced with a minimal stub to allow compilation while we implement
//! the i3-style container tree layout.
//!
//! Original file backed up as: scrolling.rs.BACKUP

use std::rc::Rc;
use std::time::Duration;

use niri_config::PresetSize;
use niri_ipc::{ColumnDisplay, SizeChange};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Point, Rectangle, Scale, Serial, Size};

use super::monitor::InsertPosition;
use super::tile::{Tile, TileRenderElement};
use super::workspace::InteractiveResize;
use super::{LayoutElement, Options, RemovedTile};
use crate::animation::Clock;
use crate::niri_render_elements;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::RenderTarget;
use crate::utils::transaction::Transaction;
use crate::utils::ResizeEdge;
use crate::window::ResolvedWindowRules;

// ============================================================================
// STUB STRUCTURES - Minimal implementations for compilation
// ============================================================================

/// STUB: Simplified tiling space (will be replaced with i3 container tree)
#[derive(Debug)]
pub struct ScrollingSpace<W: LayoutElement> {
    tiles: Vec<Tile<W>>,
    view_size: Size<f64, Logical>,
    working_area: Rectangle<f64, Logical>,
    scale: f64,
    clock: Clock,
    options: Rc<Options>,
    _phantom: std::marker::PhantomData<W>,
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
        Self {
            tiles: Vec::new(),
            view_size,
            working_area,
            scale,
            clock,
            options,
            _phantom: std::marker::PhantomData,
        }
    }

    // STUB: Basic getters
    pub fn windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.tiles.iter().map(|t| t.window())
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        self.tiles.iter()
    }

    pub fn active_tile(&self) -> Option<&Tile<W>> {
        self.tiles.first()
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        self.tiles.first_mut().map(|t| t.window_mut())
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        false
    }

    // STUB: Window management - minimal implementations
    pub fn add_window(&mut self, _window: W, _rules: ResolvedWindowRules, _width: ColumnWidth, _height: WindowHeight) {
        // TODO i3-conversion: Implement in container tree
    }

    pub fn remove_window(&mut self, _window: &W) -> Option<RemovedTile<W>> {
        // TODO i3-conversion: Implement in container tree
        None
    }

    pub fn update_window(&mut self, _window: &W::Id, _serial: Option<smithay::utils::Serial>) {
        // TODO i3-conversion: Implement in container tree
    }

    pub fn find_window(&self, _window: &W) -> Option<(usize, usize)> {
        // TODO i3-conversion: Implement in container tree
        None
    }

    // STUB: Rendering - return empty
    pub fn render_elements<R: NiriRenderer>(
        &self,
        _renderer: &mut R,
        _target: RenderTarget,
        _scrolling_focus_ring: bool,
    ) -> Vec<ScrollingSpaceRenderElement<R>> {
        Vec::new()
    }

    // STUB: Layout operations - no-ops
    pub fn update_config(
        &mut self,
        _view_size: Size<f64, Logical>,
        _working_area: Rectangle<f64, Logical>,
        _scale: f64,
        _options: Rc<Options>,
    ) {}

    pub fn set_view_size(&mut self, view_size: Size<f64, Logical>, _working_area: Rectangle<f64, Logical>) {
        self.view_size = view_size;
    }

    pub fn advance_animations(&mut self) {}

    pub fn are_animations_ongoing(&self) -> bool {
        false
    }

    pub fn update_render_elements(&mut self, _is_active: bool) {}

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

    // STUB: Focus operations - no-ops
    pub fn activate_window(&mut self, _window: &W::Id) -> bool { false }

    pub fn focus_left(&mut self) -> bool { false }
    pub fn focus_right(&mut self) -> bool { false }
    pub fn focus_down(&mut self) -> bool { false }
    pub fn focus_up(&mut self) -> bool { false }

    // STUB: Move operations - no-ops
    pub fn move_left(&mut self) -> bool { false }
    pub fn move_right(&mut self) -> bool { false }
    pub fn move_down(&mut self) -> bool { false }
    pub fn move_up(&mut self) -> bool { false }

    // STUB: Column operations - no-ops
    pub fn consume_into_column(&mut self) {}
    pub fn expel_from_column(&mut self) {}

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

    // STUB: Additional methods needed by workspace.rs
    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        self.tiles.iter_mut()
    }

    pub fn tiles_with_render_positions(&self) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> + '_ {
        self.tiles.iter().map(|t| (t, Point::from((0.0, 0.0)), true))
    }

    pub fn tiles_with_render_positions_mut(&mut self, _round: bool) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> + '_ {
        self.tiles.iter_mut().map(|t| (t, Point::from((0.0, 0.0))))
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, niri_ipc::WindowLayout)> + '_ {
        use niri_ipc::WindowLayout;
        self.tiles.iter().map(|t| {
            (t, WindowLayout {
                pos_in_scrolling_layout: None,
                tile_size: (0.0, 0.0),
                window_size: (0, 0),
                tile_pos_in_workspace_view: None,
                window_offset_in_tile: (0.0, 0.0),
            })
        })
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        false
    }

    pub fn update_shaders(&mut self) {}

    pub fn active_window(&self) -> Option<&W> {
        self.tiles.first().map(|t| t.window())
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn add_tile(
        &mut self,
        _col_idx: Option<usize>,
        _tile: Tile<W>,
        _activate: bool,
        _width: ColumnWidth,
        _is_full_width: bool,
        _height: Option<WindowHeight>,
    ) {
        // TODO i3-conversion: Implement in container tree
    }

    pub fn add_tile_right_of(
        &mut self,
        _next_to: &W::Id,
        _tile: Tile<W>,
        _activate: bool,
        _width: ColumnWidth,
        _is_full_width: bool,
    ) {
        // TODO i3-conversion: Implement in container tree
    }

    pub fn add_tile_to_column(
        &mut self,
        _col_idx: usize,
        _tile_idx: Option<usize>,
        _tile: Tile<W>,
        _activate: bool,
    ) {
        // TODO i3-conversion: Implement in container tree
    }

    pub fn active_tile_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        None
    }

    // STUB: Additional missing methods
    pub fn active_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        self.tiles.first_mut()
    }

    pub fn add_column(
        &mut self,
        _col_idx: Option<usize>,
        _column: Column<W>,
        _activate: bool,
        _height: Option<WindowHeight>,
    ) {}
    pub fn remove_tile(&mut self, _window: &W::Id, _transaction: Transaction) -> RemovedTile<W> {
        // TODO i3-conversion: Return proper RemovedTile
        panic!("ScrollingSpace::remove_tile called on stub - should not happen during compilation")
    }
    pub fn remove_active_tile(&mut self, _transaction: Transaction) -> Option<RemovedTile<W>> {
        // TODO i3-conversion: Return proper RemovedTile
        None
    }
    pub fn remove_active_column(&mut self) -> Option<Column<W>> { None }

    pub fn new_window_size(&self, _width: Option<PresetSize>, _height: Option<PresetSize>, _rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        Size::from((800, 600))
    }

    pub fn new_window_toplevel_bounds(&self, _rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        Size::from((800, 600))
    }

    pub fn focus_column_first(&mut self) {}
    pub fn focus_column_last(&mut self) {}
    pub fn focus_column(&mut self, _idx: usize) {}
    pub fn focus_window_in_column(&mut self, _index: u8) {}
    pub fn focus_down_or_left(&mut self) {}
    pub fn focus_down_or_right(&mut self) {}
    pub fn focus_up_or_left(&mut self) {}
    pub fn focus_up_or_right(&mut self) {}
    pub fn focus_top(&mut self) {}
    pub fn focus_bottom(&mut self) {}

    pub fn move_column_to_first(&mut self) {}
    pub fn move_column_to_last(&mut self) {}
    pub fn move_column_to_index(&mut self, _idx: usize) {}

    pub fn consume_or_expel_window_left(&mut self, _window: Option<&W::Id>) {}
    pub fn consume_or_expel_window_right(&mut self, _window: Option<&W::Id>) {}

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

    pub fn refresh(&mut self, _is_active: bool, _is_focused: bool) {}
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
