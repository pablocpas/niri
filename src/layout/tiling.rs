//! i3-style hierarchical tiling layout
//!
//! This module implements an i3-style tiling window manager with hierarchical containers.
//! Windows are organized in a tree structure where:
//! - Internal nodes are containers with a layout mode (SplitH, SplitV, Tabbed, Stacked)
//! - Leaf nodes contain individual windows wrapped in Tiles
//! - Navigation and movement follow the tree hierarchy
//!
//! The implementation uses SlotMap for efficient O(1) node access and safe reference handling.

use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

use niri_config::utils::MergeWith as _;
use niri_config::{Border, PresetSize, TabBar};
use niri_ipc::{ColumnDisplay, LayoutTreeNode, SizeChange};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size};

use super::closing_window::{ClosingWindow, ClosingWindowRenderElement};
use super::container::{
    ContainerTree, DetachedContainer, DetachedNode, Direction, Layout, LeafLayoutInfo, TabBarInfo,
};
use super::monitor::{InsertPosition, SplitIndicator};
use super::tile::{Tile, TileRenderElement};
use super::{ConfigureIntent, LayoutElement, Options, RemovedTile};
use crate::animation::{Animation, Clock};
use crate::niri_render_elements;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::RenderTarget;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::transaction::Transaction;
use crate::utils::ResizeEdge;
use crate::window::ResolvedWindowRules;
use crate::layout::tab_bar::{render_tab_bar, TabBarRenderOutput};
use log::warn;
use crate::utils::{round_logical_in_physical_max1, to_physical_precise_round};

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
    /// Cached tab bar textures keyed by container path.
    tab_bar_cache: RefCell<HashMap<Vec<usize>, TabBarCacheEntry>>,
    /// Alternate tab bar cache for swap (avoids allocation).
    tab_bar_cache_alt: RefCell<HashMap<Vec<usize>, TabBarCacheEntry>>,
    /// Whether this workspace is active (for tab bar styling).
    is_active: bool,
    /// Currently fullscreen window (if any)
    fullscreen_window: Option<W::Id>,
    /// Windows in the closing animation.
    closing_windows: Vec<ClosingWindow>,
}

#[derive(Debug, Clone, PartialEq)]
struct TabBarTabState {
    title: String,
    is_focused: bool,
    is_urgent: bool,
    block_out: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct TabBarState {
    layout: Layout,
    size: Size<f64, Logical>,
    row_height: f64,
    scale: f64,
    config: TabBar,
    tabs: Vec<TabBarTabState>,
}

#[derive(Debug, Clone)]
struct TabBarCacheEntry {
    state: TabBarState,
    buffer: TextureBuffer<GlesTexture>,
    tab_widths_px: Vec<i32>,
}

fn tab_bar_state_from_info(
    info: &TabBarInfo,
    config: &TabBar,
    is_active: bool,
    scale: f64,
    target: RenderTarget,
) -> TabBarState {
    let tabs = info
        .tabs
        .iter()
        .map(|tab| TabBarTabState {
            title: tab.title.clone(),
            is_focused: tab.is_focused && is_active,
            is_urgent: tab.is_urgent,
            block_out: target.should_block_out(tab.block_out_from),
        })
        .collect();

    TabBarState {
        layout: info.layout,
        size: info.rect.size,
        row_height: info.row_height,
        scale,
        config: config.clone(),
        tabs,
    }
}

niri_render_elements! {
    TilingSpaceRenderElement<R> => {
        Tile = TileRenderElement<R>,
        TabBar = PrimaryGpuTextureRenderElement,
        ClosingWindow = ClosingWindowRenderElement,
    }
}

/// Container wrapper representing a top-level column in the i3-style tree.
///
/// This holds a detached subtree so structure survives moving across workspaces.
#[derive(Debug)]
pub struct Column<W: LayoutElement> {
    /// Detached subtree that preserves container structure.
    subtree: DetachedNode<W>,
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
        let layouts = space.display_layouts();
        for info in layouts {
            // Use O(1) key lookup instead of O(depth) path lookup.
            if let Some(tile) = space.tree.get_tile(info.key) {
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
        // Clone layouts here because we need mutable access to space later.
        // The layouts are small (just NodeKey + rect per tile).
        let layouts = space.display_layouts().to_vec();
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
                // Use O(1) key lookup instead of O(depth) path lookup.
                if let Some(tile) = space.tree.get_tile_mut(info.key) {
                    let mut pos = info.rect.loc + tile.render_offset();
                    if self.round {
                        pos = pos
                            .to_physical_precise_round(self.scale)
                            .to_logical(self.scale);
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
    /// Returns a reference to the current layout information, avoiding clones.
    fn display_layouts(&self) -> &[LeafLayoutInfo] {
        if self.tree.leaf_layouts().is_empty() {
            self.tree
                .pending_leaf_layouts()
                .unwrap_or_else(|| self.tree.leaf_layouts())
        } else {
            self.tree.leaf_layouts()
        }
    }

    fn effective_tab_bar_config(&self) -> TabBar {
        self.options.layout.tab_bar.clone()
    }

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
        } else {
            let focus_path = self.tree.focus_path();
            if focus_path.is_empty() {
                self.tree
                    .focused_window()
                    .is_some()
                    .then(|| focus_path)
            } else {
                Some(focus_path)
            }
        }
    }

    fn window_container_metrics(
        &self,
        path: &[usize],
        layout: Layout,
    ) -> Option<(Vec<usize>, usize, f64, usize, Rectangle<f64, Logical>)> {
        let (parent_path, child_idx) = self.tree.find_parent_with_layout(path.to_vec(), layout)?;
        let (container_layout, rect, child_count) =
            self.tree.container_info(parent_path.as_slice())?;
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
        let tree = ContainerTree::new(view_size, working_area, scale, options.clone());

        Self {
            tree,
            view_size,
            working_area,
            scale,
            clock,
            options,
            tab_bar_cache: RefCell::new(HashMap::new()),
            tab_bar_cache_alt: RefCell::new(HashMap::new()),
            is_active: false,
            fullscreen_window: None,
            closing_windows: Vec::new(),
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
            .map_or(false, |tile| {
                tile.window().pending_sizing_mode().is_fullscreen()
                    || tile.window().is_pending_windowed_fullscreen()
            })
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
    pub fn add_window(
        &mut self,
        window: W,
        _rules: ResolvedWindowRules,
        _width: ColumnWidth,
        _height: WindowHeight,
    ) {
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
        self.sync_fullscreen_window();
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

    pub fn update_window(&mut self, window: &W::Id, serial: Option<smithay::utils::Serial>) {
        let Some(path) = self.tree.find_window(window) else {
            return;
        };
        let Some(tile) = self.tree.tile_at_path_mut(&path) else {
            return;
        };

        // Do this before calling update_window() so it can get up-to-date info.
        if let Some(serial) = serial {
            tile.window_mut().on_commit(serial);
        }

        tile.update_window();
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
        // Pre-allocate: ~4 elements per tile + closing windows + tab bars
        let tile_count = self.tree.window_count();
        let estimated_capacity = tile_count * 4 + self.closing_windows.len() + tile_count / 2;
        let mut elements = Vec::with_capacity(estimated_capacity);
        let mut active_elements = Vec::with_capacity(8);
        let scale = Scale::from(self.scale);
        let focus_path = self.tree.focus_path();
        let fullscreen_id = self.fullscreen_window.as_ref();
        let view_rect = Rectangle::from_size(self.view_size);

        for closing in self.closing_windows.iter().rev() {
            let elem = closing.render(renderer.as_gles_renderer(), view_rect, scale, target);
            elements.push(TilingSpaceRenderElement::ClosingWindow(elem));
        }

        let render_layouts = self.display_layouts();
        for info in render_layouts.iter().rev() {
            // Use O(1) key lookup instead of O(depth) path lookup.
            if let Some(tile) = self.tree.get_tile(info.key) {
                let is_fullscreen_tile = fullscreen_id.is_some_and(|id| id == tile.window().id());
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

                let is_focused = info.path == focus_path;
                let draw_focus = scrolling_focus_ring && is_focused;
                let target_elements = if info.path == focus_path {
                    &mut active_elements
                } else {
                    &mut elements
                };
                tile.render(renderer, pos, draw_focus, is_focused, target, &mut |elem| {
                    target_elements.push(TilingSpaceRenderElement::from(elem));
                });
            }
        }

        elements.extend(active_elements);

        if fullscreen_id.is_none() && !self.options.layout.tab_bar.off {
            let tab_bar_infos = self.tree.tab_bar_layouts();
            let mut cache = self.tab_bar_cache.borrow_mut();
            let mut next_cache = self.tab_bar_cache_alt.borrow_mut();
            next_cache.clear();
            let gles = renderer.as_gles_renderer();
            let tab_bar_config = self.effective_tab_bar_config();
            let is_active_workspace = self.is_active;
            for info in tab_bar_infos {
                let state = tab_bar_state_from_info(
                    &info,
                    &tab_bar_config,
                    is_active_workspace,
                    self.scale,
                    target,
                );
                let (buffer, tab_widths_px) = match cache.get(&info.path) {
                    Some(entry) if entry.state == state => {
                        (entry.buffer.clone(), entry.tab_widths_px.clone())
                    }
                    _ => match render_tab_bar(
                        gles,
                        &tab_bar_config,
                        info.layout,
                        info.rect,
                        info.row_height,
                        &info.tabs,
                        is_active_workspace,
                        target,
                        self.scale,
                    ) {
                        Ok(TabBarRenderOutput {
                            buffer,
                            tab_widths_px,
                        }) => (buffer, tab_widths_px),
                        Err(err) => {
                            warn!("tab bar render failed: {err}");
                            continue;
                        }
                    },
                };

                let mut location = info.rect.loc;
                location = location.to_physical_precise_round(scale).to_logical(scale);
                let elem = TextureRenderElement::from_texture_buffer(
                    buffer.clone(),
                    location,
                    1.0,
                    None,
                    None,
                    Kind::Unspecified,
                );
                elements.push(TilingSpaceRenderElement::TabBar(
                    PrimaryGpuTextureRenderElement(elem),
                ));

                next_cache.insert(
                    info.path,
                    TabBarCacheEntry {
                        state,
                        buffer,
                        tab_widths_px,
                    },
                );
            }
            // Swap caches: next becomes current, current will be cleared on next frame
            std::mem::swap(&mut *cache, &mut *next_cache);
        } else {
            self.tab_bar_cache.borrow_mut().clear();
        }

        elements
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        scrolling_focus_ring: bool,
        push: &mut dyn FnMut(TilingSpaceRenderElement<R>),
    ) {
        for elem in self.render_elements(renderer, target, scrolling_focus_ring) {
            push(elem);
        }
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
        self.tree
            .update_config(view_size, working_area, scale, options);
        self.tree.layout();
    }

    pub fn set_view_size(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
    ) {
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

        self.closing_windows.retain_mut(|closing| {
            closing.advance_animations();
            closing.are_animations_ongoing()
        });
    }

    pub fn are_animations_ongoing(&self) -> bool {
        TileIter::new(&self.tree).any(|tile| tile.are_animations_ongoing())
            || !self.closing_windows.is_empty()
    }

    pub fn update_render_elements(&mut self, is_active: bool) {
        self.is_active = is_active;
        let applied = self.tree.apply_pending_layouts_if_ready();
        if applied && self.tree.take_pending_relayout() {
            self.tree.layout();
        }
        let has_pending = self.tree.has_pending_layouts();
        let state_layouts = if has_pending {
            self.tree
                .pending_leaf_layouts_cloned()
                .unwrap_or_else(|| self.tree.leaf_layouts_cloned())
        } else {
            self.tree.leaf_layouts_cloned()
        };
        // Clone here because we need mutable access to tree in the loop below.
        let render_layouts = self.display_layouts().to_vec();
        let workspace_view = Rectangle::from_size(self.view_size);
        let focus_path = self.tree.focus_path();
        let scale = Scale::from(self.scale);
        let fullscreen_id = self.fullscreen_window.as_ref();

        for info in state_layouts {
            // Use O(1) key lookup instead of O(depth) path lookup.
            if let Some(tile) = self.tree.get_tile_mut(info.key) {
                Self::update_window_state(
                    tile,
                    &info,
                    &focus_path,
                    is_active,
                    self.options.deactivate_unfocused_windows,
                    !has_pending,
                    self.working_area.size,
                    &self.options,
                    fullscreen_id,
                    self.view_size,
                );
            }
        }

        for info in render_layouts {
            // Use O(1) key lookup instead of O(depth) path lookup.
            if let Some(tile) = self.tree.get_tile_mut(info.key) {
                let is_fullscreen_tile = fullscreen_id.is_some_and(|id| id == tile.window().id());

                let mut pos = info.rect.loc + tile.render_offset();
                pos = pos.to_physical_precise_round(scale).to_logical(scale);

                let mut tile_view_rect = workspace_view;
                tile_view_rect.loc -= pos;

                if is_fullscreen_tile {
                    tile_view_rect = workspace_view;
                }

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
        let focused = self.tree.focus_in_direction(Direction::Left);
        if focused {
            self.tree.layout();
        }
        focused
    }

    pub fn focus_right(&mut self) -> bool {
        let focused = self.tree.focus_in_direction(Direction::Right);
        if focused {
            self.tree.layout();
        }
        focused
    }

    pub fn focus_down(&mut self) -> bool {
        let focused = self.tree.focus_in_direction(Direction::Down);
        if focused {
            self.tree.layout();
        }
        focused
    }

    pub fn focus_up(&mut self) -> bool {
        let focused = self.tree.focus_in_direction(Direction::Up);
        if focused {
            self.tree.layout();
        }
        focused
    }

    pub fn focus_parent(&mut self) -> bool {
        let focused = self.tree.focus_parent();
        if focused {
            self.tree.layout();
        }
        focused
    }

    pub fn focus_child(&mut self) -> bool {
        let focused = self.tree.focus_child();
        if focused {
            self.tree.layout();
        }
        focused
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

    /// Toggle between horizontal and vertical split for the focused container.
    pub fn toggle_split_layout(&mut self) {
        if self.tree.toggle_split_layout() {
            self.tree.layout();
        }
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

    /// View offset (not used in i3-style layout, always 0).
    #[cfg(test)]
    pub(super) fn view_offset(&self) -> f64 {
        0.0
    }

    #[cfg(test)]
    pub fn view_pos(&self) -> f64 {
        self.view_offset()
    }

    #[cfg(test)]
    pub fn active_column_idx(&self) -> usize {
        self.tree.focused_root_index().unwrap_or(0)
    }

    fn layout_area(&self) -> Rectangle<f64, Logical> {
        let mut area = self.working_area;
        let gap = self.options.layout.gaps;
        if gap > 0.0 {
            area.loc.x += gap;
            area.loc.y += gap;
            area.size.w = (area.size.w - gap * 2.0).max(0.0);
            area.size.h = (area.size.h - gap * 2.0).max(0.0);
        }
        area
    }

    const DROP_LAYOUT_BORDER: f64 = 30.0;
    const DROP_CENTER_RATIO: f64 = 0.3;

    fn closest_edge(
        rect: Rectangle<f64, Logical>,
        pos: Point<f64, Logical>,
    ) -> (Direction, f64) {
        let left = (pos.x - rect.loc.x).abs();
        let right = (rect.loc.x + rect.size.w - pos.x).abs();
        let top = (pos.y - rect.loc.y).abs();
        let bottom = (rect.loc.y + rect.size.h - pos.y).abs();

        let mut dir = Direction::Left;
        let mut min = left;

        if right < min {
            min = right;
            dir = Direction::Right;
        }
        if top < min {
            min = top;
            dir = Direction::Up;
        }
        if bottom < min {
            min = bottom;
            dir = Direction::Down;
        }

        (dir, min)
    }

    fn leaf_rect_for_path(&self, path: &[usize]) -> Option<Rectangle<f64, Logical>> {
        let scale = Scale::from(self.scale);
        let info = self.display_layouts().iter().find(|info| info.path == path)?;
        let tile = self.tree.get_tile(info.key)?;
        let mut tile_pos = info.rect.loc + tile.render_offset();
        tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);
        Some(Rectangle::new(tile_pos, tile.tile_size()))
    }

    fn closest_leaf_rect(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(Vec<usize>, Rectangle<f64, Logical>)> {
        let scale = Scale::from(self.scale);
        let fullscreen_id = self.fullscreen_window.as_ref();

        let mut nearest: Option<(Vec<usize>, Rectangle<f64, Logical>, f64)> = None;

        for info in self.display_layouts() {
            if let Some(tile) = self.tree.get_tile(info.key) {
                let is_fullscreen_tile =
                    fullscreen_id.is_some_and(|id| id == tile.window().id());
                if fullscreen_id.is_some() && !is_fullscreen_tile {
                    continue;
                }
                if !info.visible && !is_fullscreen_tile {
                    continue;
                }

                let mut tile_pos = info.rect.loc + tile.render_offset();
                tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);
                let tile_rect = Rectangle::new(tile_pos, tile.tile_size());

                if tile_rect.contains(pos) {
                    return Some((info.path.clone(), tile_rect));
                }

                let dx = if pos.x < tile_rect.loc.x {
                    tile_rect.loc.x - pos.x
                } else if pos.x > tile_rect.loc.x + tile_rect.size.w {
                    pos.x - (tile_rect.loc.x + tile_rect.size.w)
                } else {
                    0.0
                };
                let dy = if pos.y < tile_rect.loc.y {
                    tile_rect.loc.y - pos.y
                } else if pos.y > tile_rect.loc.y + tile_rect.size.h {
                    pos.y - (tile_rect.loc.y + tile_rect.size.h)
                } else {
                    0.0
                };
                let dist2 = dx * dx + dy * dy;

                let replace = nearest.as_ref().is_none_or(|(_, _, best)| dist2 < *best);
                if replace {
                    nearest = Some((info.path.clone(), tile_rect, dist2));
                }
            }
        }

        nearest.map(|(path, rect, _)| (path, rect))
    }

    fn indicator_rect(
        rect: Rectangle<f64, Logical>,
        direction: Direction,
        thickness: f64,
    ) -> Rectangle<f64, Logical> {
        let thickness = thickness.max(1.0);
        match direction {
            Direction::Left => Rectangle::new(
                rect.loc,
                Size::from((thickness.min(rect.size.w), rect.size.h)),
            ),
            Direction::Right => Rectangle::new(
                Point::from((
                    rect.loc.x + rect.size.w - thickness.min(rect.size.w),
                    rect.loc.y,
                )),
                Size::from((thickness.min(rect.size.w), rect.size.h)),
            ),
            Direction::Up => Rectangle::new(
                rect.loc,
                Size::from((rect.size.w, thickness.min(rect.size.h))),
            ),
            Direction::Down => Rectangle::new(
                Point::from((
                    rect.loc.x,
                    rect.loc.y + rect.size.h - thickness.min(rect.size.h),
                )),
                Size::from((rect.size.w, thickness.min(rect.size.h))),
            ),
        }
    }

    fn inset_rect(rect: Rectangle<f64, Logical>, inset: f64) -> Rectangle<f64, Logical> {
        let inset = inset
            .min(rect.size.w / 2.0)
            .min(rect.size.h / 2.0)
            .max(0.0);
        Rectangle::new(
            Point::from((rect.loc.x + inset, rect.loc.y + inset)),
            Size::from((rect.size.w - 2.0 * inset, rect.size.h - 2.0 * inset)),
        )
    }

    /// Determine insert position from pointer location
    pub(super) fn insert_position(&self, pos: Point<f64, Logical>) -> InsertPosition {
        if self.tree.is_empty() {
            return InsertPosition::NewColumn(0);
        }

        let layout_area = self.layout_area();
        if pos.y < layout_area.loc.y + Self::DROP_LAYOUT_BORDER {
            return InsertPosition::SplitRoot {
                direction: Direction::Up,
                indicator: SplitIndicator::LayoutBorder,
            };
        }
        if pos.y > layout_area.loc.y + layout_area.size.h - Self::DROP_LAYOUT_BORDER {
            return InsertPosition::SplitRoot {
                direction: Direction::Down,
                indicator: SplitIndicator::LayoutBorder,
            };
        }

        let Some((path, rect)) = self.closest_leaf_rect(pos) else {
            return InsertPosition::NewColumn(0);
        };

        let parent_layout = self
            .tree
            .parent_layout_for_path(&path)
            .unwrap_or(Layout::SplitH);

        if matches!(parent_layout, Layout::SplitH | Layout::Tabbed) {
            if pos.y < rect.loc.y + Self::DROP_LAYOUT_BORDER {
                return InsertPosition::Split {
                    path,
                    direction: Direction::Up,
                    indicator: SplitIndicator::LayoutBorder,
                };
            }
            if pos.y > rect.loc.y + rect.size.h - Self::DROP_LAYOUT_BORDER {
                return InsertPosition::Split {
                    path,
                    direction: Direction::Down,
                    indicator: SplitIndicator::LayoutBorder,
                };
            }
        } else if matches!(parent_layout, Layout::SplitV | Layout::Stacked) {
            if pos.x < rect.loc.x + Self::DROP_LAYOUT_BORDER {
                return InsertPosition::Split {
                    path,
                    direction: Direction::Left,
                    indicator: SplitIndicator::LayoutBorder,
                };
            }
            if pos.x > rect.loc.x + rect.size.w - Self::DROP_LAYOUT_BORDER {
                return InsertPosition::Split {
                    path,
                    direction: Direction::Right,
                    indicator: SplitIndicator::LayoutBorder,
                };
            }
        }

        let (direction, dist) = Self::closest_edge(rect, pos);
        let thickness = f64::min(rect.size.w, rect.size.h) * Self::DROP_CENTER_RATIO;
        if dist > thickness {
            InsertPosition::Swap { path, direction }
        } else {
            InsertPosition::Split {
                path,
                direction,
                indicator: SplitIndicator::Center,
            }
        }
    }

    /// Get hint area for insertion position
    pub(super) fn insert_hint_area(
        &self,
        position: &InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        match position {
            InsertPosition::NewColumn(_) => Some(self.layout_area()),
            InsertPosition::Swap { path, .. } => {
                let rect = self.leaf_rect_for_path(path)?;
                let thickness = f64::min(rect.size.w, rect.size.h) * Self::DROP_CENTER_RATIO;
                Some(Self::inset_rect(rect, thickness))
            }
            InsertPosition::Split {
                path,
                direction,
                indicator,
            } => {
                let rect = self.leaf_rect_for_path(path)?;
                let thickness = match indicator {
                    SplitIndicator::LayoutBorder => Self::DROP_LAYOUT_BORDER,
                    SplitIndicator::Center => {
                        f64::min(rect.size.w, rect.size.h) * Self::DROP_CENTER_RATIO
                    }
                };
                Some(Self::indicator_rect(rect, *direction, thickness))
            }
            InsertPosition::SplitRoot { direction, indicator } => {
                let rect = self.layout_area();
                let thickness = match indicator {
                    SplitIndicator::LayoutBorder => Self::DROP_LAYOUT_BORDER,
                    SplitIndicator::Center => {
                        f64::min(rect.size.w, rect.size.h) * Self::DROP_CENTER_RATIO
                    }
                };
                Some(Self::indicator_rect(rect, *direction, thickness))
            }
            InsertPosition::Floating => None,
        }
    }

    // Window queries
    fn tab_bar_hit(&self, pos: Point<f64, Logical>) -> Option<(&W, super::HitType)> {
        if self.fullscreen_window.is_some() || self.options.layout.tab_bar.off {
            return None;
        }

        let scale = Scale::from(self.scale);
        let tab_bar_infos = self.tree.tab_bar_layouts();
        if tab_bar_infos.is_empty() {
            return None;
        }

        let cache = self.tab_bar_cache.borrow();
        for info in tab_bar_infos {
            let tab_count = info.tabs.len();
            if tab_count == 0 {
                continue;
            }

            let bar_loc_px: Point<i32, Physical> = info.rect.loc.to_physical_precise_round(scale);
            let pos_px: Point<i32, Physical> = pos.to_physical_precise_round(scale) - bar_loc_px;
            let width_px = to_physical_precise_round::<i32>(self.scale, info.rect.size.w).max(1);
            let height_px = to_physical_precise_round::<i32>(self.scale, info.rect.size.h).max(1);

            if pos_px.x < 0 || pos_px.y < 0 || pos_px.x >= width_px || pos_px.y >= height_px {
                continue;
            }

            let row_height_px =
                to_physical_precise_round::<i32>(self.scale, info.row_height).max(1);
            let focused_idx = info
                .tabs
                .iter()
                .position(|tab| tab.is_focused)
                .unwrap_or(0);

            let tab_idx = match info.layout {
                Layout::Tabbed => {
                    if pos_px.y >= row_height_px {
                        focused_idx
                    } else if let Some(widths) = cache.get(&info.path).and_then(|entry| {
                        if entry.tab_widths_px.len() == tab_count {
                            Some(entry.tab_widths_px.as_slice())
                        } else {
                            None
                        }
                    }) {
                        let mut cursor = 0;
                        let mut found = None;
                        for (idx, width) in widths.iter().enumerate() {
                            let end = cursor + *width;
                            if pos_px.x < end {
                                found = Some(idx);
                                break;
                            }
                            cursor = end;
                        }
                        found.unwrap_or_else(|| tab_count.saturating_sub(1))
                    } else {
                        let base = width_px / tab_count as i32;
                        let mut cursor = 0;
                        let mut found = None;
                        for idx in 0..tab_count {
                            let mut width = base;
                            if idx + 1 == tab_count {
                                width += width_px - base * tab_count as i32;
                            }
                            let end = cursor + width;
                            if pos_px.x < end {
                                found = Some(idx);
                                break;
                            }
                            cursor = end;
                        }
                        found.unwrap_or_else(|| tab_count.saturating_sub(1))
                    }
                }
                Layout::Stacked => {
                    let stack_height_px = row_height_px * tab_count as i32;
                    if pos_px.y >= stack_height_px {
                        focused_idx
                    } else {
                        let max_idx = tab_count.saturating_sub(1) as i32;
                        (pos_px.y / row_height_px).min(max_idx) as usize
                    }
                }
                _ => continue,
            };

            if let Some(window) = self.tree.window_for_tab(&info.path, tab_idx) {
                return Some((
                    window,
                    super::HitType::Activate {
                        is_tab_indicator: true,
                    },
                ));
            }
        }

        None
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, super::HitType)> {
        let scale = Scale::from(self.scale);
        let fullscreen_id = self.fullscreen_window.as_ref();

        if let Some(hit) = self.tab_bar_hit(pos) {
            return Some(hit);
        }

        let render_layouts = self.display_layouts();
        for info in render_layouts.iter().rev() {
            // Use O(1) key lookup instead of O(depth) path lookup.
            if let Some(tile) = self.tree.get_tile(info.key) {
                let is_fullscreen_tile = fullscreen_id.is_some_and(|id| id == tile.window().id());
                if fullscreen_id.is_some() && !is_fullscreen_tile {
                    continue;
                }
                if !info.visible && !is_fullscreen_tile {
                    continue;
                }

                // For fullscreen tiles, use (0,0) as base position since they cover the entire screen
                let base_pos = if is_fullscreen_tile {
                    Point::from((0.0, 0.0))
                } else {
                    info.rect.loc
                };
                let mut tile_pos = base_pos + tile.render_offset();
                tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

                if let Some(hit) = super::HitType::hit_tile(tile, tile_pos, pos) {
                    return Some(hit);
                }
            }
        }

        None
    }

    pub fn window_loc(&self, window: &W) -> Option<Point<f64, Logical>> {
        let path = self.tree.find_window(window.id())?;
        let layouts = self.display_layouts();
        let info = layouts.iter().find(|layout| layout.path == path)?;
        let tile = self.tree.tile_at_path(&path)?;
        let scale = Scale::from(self.scale);

        let mut tile_pos = info.rect.loc + tile.render_offset();
        tile_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

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

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> + '_ {
        TileRenderPositions::new(self)
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> + '_ {
        TileRenderPositionsMut::new(self, round)
    }

    pub fn tiles_with_ipc_layouts(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, niri_ipc::WindowLayout)> + '_ {
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
            || !self.closing_windows.is_empty()
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
        } else if self.tree.is_empty() {
            self.tree.append_leaf(tile, activate);
        } else {
            let focused_id = self
                .tree
                .focused_tile()
                .map(|tile| tile.window().id().clone());

            if let Some(id) = focused_id {
                let inserted = self.tree.insert_leaf_after(&id, tile, activate);
                assert!(inserted, "failed to insert tile after focused window");
            } else {
                self.tree.append_leaf(tile, activate);
            }
        }
        self.sync_fullscreen_window();
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
        self.sync_fullscreen_window();
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
            self.sync_fullscreen_window();
            self.tree.layout();
        }
    }

    pub(super) fn insert_parent_info_for_window(
        &self,
        window: &W::Id,
    ) -> Option<super::container::InsertParentInfo> {
        self.tree.insert_parent_info_for_window(window)
    }

    pub(super) fn replace_tile_at_path(
        &mut self,
        path: &[usize],
        tile: Tile<W>,
    ) -> Option<Tile<W>> {
        self.tree.replace_leaf_at_path(path, tile)
    }

    pub(super) fn is_leaf_at_path(&self, path: &[usize]) -> bool {
        self.tree.is_leaf_at_path(path)
    }

    pub(super) fn insert_tile_with_parent_info(
        &mut self,
        info: &super::container::InsertParentInfo,
        tile: Tile<W>,
        activate: bool,
    ) -> bool {
        if self
            .tree
            .insert_leaf_with_parent_info(info, tile, activate)
        {
            self.sync_fullscreen_window();
            self.tree.layout();
            return true;
        }

        false
    }

    pub fn insert_tile_split(
        &mut self,
        target_path: &[usize],
        direction: Direction,
        tile: Tile<W>,
        activate: bool,
    ) -> bool {
        if self
            .tree
            .insert_leaf_split(target_path, direction, tile, activate)
        {
            self.sync_fullscreen_window();
            self.tree.layout();
            return true;
        }

        false
    }

    pub fn insert_tile_split_root(
        &mut self,
        direction: Direction,
        tile: Tile<W>,
        activate: bool,
    ) -> bool {
        if self
            .tree
            .insert_leaf_split_root(direction, tile, activate)
        {
            self.sync_fullscreen_window();
            self.tree.layout();
            return true;
        }

        false
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
        let subtree = column.into_subtree();
        self.tree.insert_subtree_at_root(idx, subtree, activate);
        self.sync_fullscreen_window();
        self.tree.layout();
    }
    pub fn remove_tile(&mut self, window: &W::Id, transaction: Transaction) -> RemovedTile<W> {
        if !self.options.disable_transactions {
            self.tree.set_pending_transaction(transaction.clone());
        }
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
        let subtree = self.tree.take_root_child_subtree(idx)?;
        let column = Column::from_subtree(subtree);

        if let Some(full_id) = self.fullscreen_window.clone() {
            if self.tree.find_window(&full_id).is_none() {
                self.fullscreen_window = None;
            }
        }

        self.tree.layout();
        Some(column)
    }

    pub fn new_window_size(
        &self,
        _width: Option<PresetSize>,
        _height: Option<PresetSize>,
        rules: &ResolvedWindowRules,
    ) -> Size<i32, Logical> {
        let Some(preview) = self.tree.preview_new_leaf_geometry() else {
            return Size::from((800, 600));
        };

        let mut size = preview.rect.size;
        let mut border_config = self.options.layout.border.merged_with(&rules.border);
        border_config.width = round_logical_in_physical_max1(self.scale, border_config.width);

        if !border_config.off {
            let width = border_config.width * 2.0;
            size.w = f64::max(1.0, size.w - width);
            size.h = f64::max(1.0, size.h - width);
        }
        if preview.tab_bar_offset > 0.0 {
            size.h = f64::max(1.0, size.h - preview.tab_bar_offset);
        }

        size.to_i32_floor()
    }

    pub fn new_window_toplevel_bounds(&self, _rules: &ResolvedWindowRules) -> Size<i32, Logical> {
        Size::from((800, 600))
    }

    pub fn focus_column_first(&mut self) {
        self.tree.focus_root_child(0);
        self.tree.layout();
    }

    pub fn focus_column_last(&mut self) {
        let len = self.tree.root_children_len();
        if len > 0 {
            self.tree.focus_root_child(len - 1);
            self.tree.layout();
        }
    }

    /// Columns are 1-based to match user-facing commands.
    pub fn focus_column(&mut self, idx: usize) {
        if idx == 0 {
            return;
        }
        self.tree.focus_root_child(idx - 1);
        self.tree.layout();
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
        self.tree.layout();
    }

    pub fn focus_down_or_left(&mut self) {
        let focused = self.tree.focus_in_direction(Direction::Down)
            || self.tree.focus_in_direction(Direction::Left);
        if focused {
            self.tree.layout();
        }
    }

    pub fn focus_down_or_right(&mut self) {
        let focused = self.tree.focus_in_direction(Direction::Down)
            || self.tree.focus_in_direction(Direction::Right);
        if focused {
            self.tree.layout();
        }
    }

    pub fn focus_up_or_left(&mut self) {
        let focused = self.tree.focus_in_direction(Direction::Up)
            || self.tree.focus_in_direction(Direction::Left);
        if focused {
            self.tree.layout();
        }
    }

    pub fn focus_up_or_right(&mut self) {
        let focused = self.tree.focus_in_direction(Direction::Up)
            || self.tree.focus_in_direction(Direction::Right);
        if focused {
            self.tree.layout();
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

            if let Some(path) = self.tree.find_window(window) {
                if let Some(tile) = self.tree.tile_at_path_mut(&path) {
                    tile.pending_maximized |= tile.window().pending_sizing_mode().is_maximized();
                    tile.request_fullscreen(!self.options.animations.off, None);
                }
            }

            self.fullscreen_window = Some(window.clone());
            self.tree.layout();
            true
        } else {
            let Some(path) = self.tree.find_window(window) else {
                return false;
            };
            let Some(tile) = self.tree.tile_at_path_mut(&path) else {
                return false;
            };
            let is_window_fullscreen = tile.window().pending_sizing_mode().is_fullscreen();
            let fullscreen_matches = self
                .fullscreen_window
                .as_ref()
                .is_some_and(|id| id == window);
            if !is_window_fullscreen && !fullscreen_matches {
                return false;
            }

            if tile.pending_maximized {
                tile.request_maximized(
                    self.working_area.size,
                    !self.options.animations.off,
                    None,
                );
            } else {
                tile.request_tile_size(
                    self.working_area.size,
                    !self.options.animations.off,
                    None,
                );
            }

            self.fullscreen_window = None;
            self.tree.layout();
            true
        }
    }

    fn sync_fullscreen_window(&mut self) {
        let keep_existing = self.fullscreen_window.as_ref().and_then(|id| {
            self.tree
                .find_window(id)
                .and_then(|path| self.tree.tile_at_path(&path))
                .filter(|tile| tile.window().pending_sizing_mode().is_fullscreen())
                .map(|_| id.clone())
        });
        if keep_existing.is_some() {
            return;
        }

        let next_fullscreen = self
            .tiles()
            .find(|tile| tile.window().pending_sizing_mode().is_fullscreen())
            .map(|tile| tile.window().id().clone());
        self.fullscreen_window = next_fullscreen;
    }

    pub fn set_maximized(&mut self, window: &W::Id, maximize: bool) -> bool {
        let Some(path) = self.tree.find_window(window) else {
            return false;
        };
        let Some(tile) = self.tree.tile_at_path_mut(&path) else {
            return false;
        };

        tile.pending_maximized = maximize;
        self.tree.layout();
        true
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

    pub fn swap_window_in_direction(&mut self, direction: ScrollDirection) {
        let result = match direction {
            ScrollDirection::Left => self.tree.move_in_direction(Direction::Left),
            ScrollDirection::Right => self.tree.move_in_direction(Direction::Right),
            ScrollDirection::Up => self.tree.move_in_direction(Direction::Up),
            ScrollDirection::Down => self.tree.move_in_direction(Direction::Down),
        };
        if result {
            self.tree.layout();
        }
    }

    pub fn start_open_animation(&mut self, _id: &W::Id) -> bool {
        let Some(path) = self.tree.find_window(_id) else {
            return false;
        };
        if let Some(tile) = self.tree.tile_at_path_mut(&path) {
            tile.start_open_animation();
            return true;
        }
        false
    }
    pub fn start_close_animation_for_window<R: NiriRenderer>(
        &mut self,
        renderer: &mut R,
        window: &W::Id,
        blocker: crate::utils::transaction::TransactionBlocker,
    ) {
        if self.options.animations.window_close.anim.off || self.clock.should_complete_instantly() {
            return;
        }

        let Some(path) = self.tree.find_window(window) else {
            return;
        };

        let Some((rect, visible)) = self
            .tree
            .leaf_layouts()
            .iter()
            .find(|info| info.path == path)
            .map(|info| (info.rect, info.visible))
        else {
            return;
        };

        if !visible {
            return;
        }

        let Some(tile) = self.tree.tile_at_path_mut(&path) else {
            return;
        };

        let Some(snapshot) = tile.take_unmap_snapshot() else {
            return;
        };

        let tile_size = tile.tile_size();
        let tile_pos = rect.loc + tile.render_offset();

        let anim = Animation::new(
            self.clock.clone(),
            0.,
            1.,
            0.,
            self.options.animations.window_close.anim,
        );

        let blocker = if self.options.disable_transactions {
            crate::utils::transaction::TransactionBlocker::completed()
        } else {
            blocker
        };

        let scale = Scale::from(self.scale);
        let res = ClosingWindow::new(
            renderer.as_gles_renderer(),
            snapshot,
            scale,
            tile_size,
            tile_pos,
            blocker,
            anim,
        );
        match res {
            Ok(closing) => {
                self.closing_windows.push(closing);
            }
            Err(err) => {
                warn!("error creating a closing window animation: {err:?}");
            }
        }
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        let applied = self.tree.apply_pending_layouts_if_ready();
        if applied && self.tree.take_pending_relayout() {
            self.tree.layout();
        }
        let has_pending = self.tree.has_pending_layouts();
        let layouts = if has_pending {
            self.tree
                .pending_leaf_layouts_cloned()
                .unwrap_or_else(|| self.tree.leaf_layouts_cloned())
        } else {
            self.tree.leaf_layouts_cloned()
        };
        let focus_path = self.tree.focus_path();
        let fullscreen_id = self.fullscreen_window.as_ref();

        for info in layouts {
            // Use O(1) key lookup instead of O(depth) path lookup.
            if let Some(tile) = self.tree.get_tile_mut(info.key) {
                let deactivate_unfocused = self.options.deactivate_unfocused_windows && !is_focused;

                Self::update_window_state(
                    tile,
                    &info,
                    &focus_path,
                    is_active,
                    deactivate_unfocused,
                    !has_pending,
                    self.working_area.size,
                    &self.options,
                    fullscreen_id,
                    self.view_size,
                );
            }
        }
    }
    pub fn render_above_top_layer(&self) -> bool {
        false
    }

    pub fn scroll_amount_to_activate(&self, _window: &W::Id) -> f64 {
        0.0
    }

    pub fn popup_target_rect(&self, window: &W::Id) -> Option<Rectangle<f64, Logical>> {
        // Find the tile for this window and return its popup target rectangle
        for info in self.display_layouts() {
            if let Some(tile) = self.tree.get_tile(info.key) {
                if tile.window().id() == window {
                    // Similar to scrolling layout: constrain horizontally to window,
                    // vertically to the working area
                    let width = tile.window_size().w;
                    let height = self.working_area.size.h;

                    let mut target = Rectangle::from_size(Size::from((width, height)));
                    target.loc.y += self.working_area.loc.y;
                    target.loc.y -= info.rect.loc.y;
                    target.loc.y -= tile.window_loc().y;

                    return Some(target);
                }
            }
        }
        None
    }

    pub fn view_offset_gesture_begin(&mut self, _is_touchpad: bool) {}
    pub fn view_offset_gesture_update(
        &mut self,
        _delta: f64,
        _timestamp: Duration,
        _is_touchpad: bool,
    ) -> Option<bool> {
        None
    }
    pub fn view_offset_gesture_end(&mut self, _cancelled: Option<bool>) -> bool {
        false
    }

    pub fn dnd_scroll_gesture_begin(&mut self) {}
    pub fn dnd_scroll_gesture_scroll(&mut self, _delta: f64) -> bool {
        false
    }
    pub fn dnd_scroll_gesture_end(&mut self) {}
}

impl TilingSpace<crate::window::Mapped> {
    pub(crate) fn layout_tree(&self) -> Option<LayoutTreeNode> {
        self.tree.layout_tree()
    }
}

impl<W: LayoutElement> TilingSpace<W> {
    fn update_window_state(
        tile: &mut Tile<W>,
        info: &LeafLayoutInfo,
        focus_path: &[usize],
        workspace_active: bool,
        deactivate_unfocused: bool,
        request_size: bool,
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
        if request_size {
            tile.request_tile_size(target_size, false, None);
        }

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
            subtree: DetachedNode::Leaf(tile),
        }
    }

    pub fn from_tiles(tiles: Vec<Tile<W>>) -> Self {
        if tiles.is_empty() {
            return Self {
                subtree: DetachedNode::Container(DetachedContainer::new(Layout::SplitV, Vec::new())),
            };
        }

        if tiles.len() == 1 {
            return Self::new(tiles.into_iter().next().unwrap());
        }

        let children = tiles
            .into_iter()
            .map(DetachedNode::Leaf)
            .collect::<Vec<_>>();
        Self {
            subtree: DetachedNode::Container(DetachedContainer::new(Layout::SplitV, children)),
        }
    }

    pub fn tiles(&self) -> Vec<&Tile<W>> {
        self.subtree.tiles()
    }

    pub fn contains(&self, window: &W) -> bool {
        self.subtree.contains_window(window.id())
    }

    pub fn from_subtree(subtree: DetachedNode<W>) -> Self {
        Self { subtree }
    }

    pub fn into_subtree(self) -> DetachedNode<W> {
        self.subtree
    }

    pub fn into_tiles(self) -> Vec<Tile<W>> {
        self.subtree.into_tiles()
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
        f64::max(
            working_area_size.w - gaps * 2.0 - extra_size.w - border,
            1.0,
        ),
        f64::max(
            working_area_size.h - gaps * 2.0 - extra_size.h - border,
            1.0,
        ),
    ))
    .to_i32_floor()
}
