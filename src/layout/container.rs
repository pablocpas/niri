//! i3-style container tree implementation using SlotMap
//!
//! This module implements the hierarchical container system used by i3wm.
//! Containers form a tree where:
//! - Leaf nodes contain windows (wrapped in Tiles)
//! - Internal nodes contain child containers with a specific layout
//! - Each container can have layouts: SplitH, SplitV, Tabbed, or Stacked
//!
//! Uses slotmap for efficient memory management and O(1) access to nodes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use slotmap::{new_key_type, SecondaryMap, SlotMap};
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::{Blocker, BlockerState};

use super::tile::Tile;
use super::{LayoutElement, Options};
use super::tab_bar::tab_bar_row_height;
use crate::window::Mapped;
use crate::utils::transaction::{Transaction, TransactionBlocker};
use niri_config::BlockOutFrom;
use niri_ipc::{LayoutTreeLayout, LayoutTreeNode};

// ============================================================================
// SlotMap Key Types
// ============================================================================

new_key_type! {
    /// Key to reference a node in the container tree
    pub struct NodeKey;
}

// ============================================================================
// Container Types and Enums
// ============================================================================

/// Layout mode for a container (following i3 model)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Horizontal split - children arranged left to right
    SplitH,
    /// Vertical split - children arranged top to bottom
    SplitV,
    /// Tabbed layout - children stacked with tab bar
    Tabbed,
    /// Stacked layout - children stacked with title bars
    Stacked,
}

/// Direction for navigation and movement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabBarTab {
    pub title: String,
    pub is_focused: bool,
    pub is_urgent: bool,
    pub block_out_from: Option<BlockOutFrom>,
}

#[derive(Debug, Clone)]
pub struct TabBarInfo {
    pub path: Vec<usize>,
    pub layout: Layout,
    pub rect: Rectangle<f64, Logical>,
    pub row_height: f64,
    pub tabs: Vec<TabBarTab>,
}

const MIN_CHILD_PERCENT: f64 = 0.05;
const MOVE_ANIMATION_THRESHOLD: f64 = 0.1;

/// Node type in the container tree
#[derive(Debug)]
pub enum NodeData<W: LayoutElement> {
    /// Container node with children (stored as keys)
    Container(ContainerData),
    /// Leaf node containing a tile
    Leaf(Tile<W>),
}

/// Detached subtree used to move container structures across trees.
#[derive(Debug)]
pub enum DetachedNode<W: LayoutElement> {
    Container(DetachedContainer<W>),
    Leaf(Tile<W>),
}

#[derive(Debug)]
pub struct DetachedContainer<W: LayoutElement> {
    layout: Layout,
    children: Vec<DetachedNode<W>>,
    child_percents: Vec<f64>,
    focus_stack: Vec<usize>,
    preserve_on_single: bool,
}

/// Container data stored in slotmap
#[derive(Debug)]
pub struct ContainerData {
    /// Layout mode for this container
    layout: Layout,
    /// Child node keys (indices into the tree's SlotMap)
    children: Vec<NodeKey>,
    /// Focus history (most recently used first)
    focus_stack: Vec<NodeKey>,
    /// Preserve container even if it has a single child (explicit split).
    preserve_on_single: bool,
    /// Relative sizes of children (sum normalized to 1.0 for split layouts)
    child_percents: Vec<f64>,
    /// Cached geometry for rendering
    geometry: Rectangle<f64, Logical>,
}

/// Cached layout information for a leaf tile.
#[derive(Debug, Clone)]
pub struct LeafLayoutInfo {
    pub key: NodeKey,
    pub path: Vec<usize>,
    pub rect: Rectangle<f64, Logical>,
    pub visible: bool,
}

#[derive(Debug, Clone)]
pub(super) struct InsertParentInfo {
    pub parent_path: Vec<usize>,
    pub insert_idx: usize,
    pub layout: Layout,
    pub child_percents: Vec<f64>,
}

#[derive(Debug)]
struct LayoutData {
    leaf_layouts: Vec<LeafLayoutInfo>,
    container_geometries: HashMap<NodeKey, Rectangle<f64, Logical>>,
    tab_bar_offsets: HashMap<NodeKey, f64>,
    titlebar_flags: HashMap<NodeKey, bool>,
}

#[derive(Debug)]
struct PendingLayout {
    data: LayoutData,
    blocker: TransactionBlocker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutRequestMode {
    Normal,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LayoutRequest {
    mode: LayoutRequestMode,
    size: Size<i32, Logical>,
}

/// Root container tree for a workspace
#[derive(Debug)]
pub struct ContainerTree<W: LayoutElement> {
    /// SlotMap storing all nodes in the tree
    nodes: SlotMap<NodeKey, NodeData<W>>,
    /// Parent pointer for each node (None for root)
    parents: SecondaryMap<NodeKey, Option<NodeKey>>,
    /// Root node key
    root: Option<NodeKey>,
    /// Layout to apply when the tree is empty (i3 workspace_layout equivalent).
    pending_layout: Option<Layout>,
    /// Focused leaf node key (source of truth for focus).
    focused_key: Option<NodeKey>,
    /// Currently selected node key (container selection via focus-parent).
    selected_key: Option<NodeKey>,
    /// Cached layout info for leaves
    leaf_layouts: Vec<LeafLayoutInfo>,
    /// Pending layouts waiting for transactions to complete.
    pending_layouts: Option<PendingLayout>,
    /// Optional transaction to use for the next atomic layout.
    pending_transaction: Option<Transaction>,
    /// Whether a new layout is requested while a transaction is pending.
    pending_relayout: bool,
    /// View size (output size)
    view_size: Size<f64, Logical>,
    /// Working area (view_size minus gaps/bars)
    working_area: Rectangle<f64, Logical>,
    /// Display scale
    scale: f64,
    /// Layout options
    options: Rc<Options>,
    /// Generation counter for cache invalidation.
    generation: u64,
    /// Cached focus path to avoid recomputation (generation, focused_key, path).
    focus_path_cache: RefCell<(u64, Option<NodeKey>, Vec<usize>)>,
    /// Reusable HashMap for tracking previous positions during animation.
    prev_positions_cache: HashMap<NodeKey, Point<f64, Logical>>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PreviewLeafGeometry {
    pub rect: Rectangle<f64, Logical>,
    pub tab_bar_offset: f64,
}

// ============================================================================
// ContainerData Implementation
// ============================================================================

impl ContainerData {
    /// Create a new container with given layout
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            children: Vec::new(),
            focus_stack: Vec::new(),
            preserve_on_single: false,
            child_percents: Vec::new(),
            geometry: Rectangle::from_size(Size::from((0.0, 0.0))),
        }
    }

    /// Get container layout
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Set container layout
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
    }

    pub fn set_layout_explicit(&mut self, layout: Layout) {
        self.layout = layout;
        self.preserve_on_single = true;
    }

    pub fn preserve_on_single(&self) -> bool {
        self.preserve_on_single
    }

    pub fn mark_preserve_on_single(&mut self) {
        self.preserve_on_single = true;
    }

    /// Get children keys
    pub fn children(&self) -> &[NodeKey] {
        &self.children
    }

    /// Number of children
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get focused child key
    pub fn focused_child_key(&self) -> Option<NodeKey> {
        self.focus_stack
            .first()
            .copied()
            .or_else(|| self.children.first().copied())
    }

    pub fn focused_child_index(&self) -> Option<usize> {
        let key = self.focused_child_key()?;
        self.children.iter().position(|child| *child == key)
    }

    pub fn bubble_focus(&mut self, node_key: NodeKey) {
        self.ensure_focus_stack();
        if let Some(pos) = self.focus_stack.iter().position(|key| *key == node_key) {
            self.focus_stack.remove(pos);
        }
        self.focus_stack.insert(0, node_key);
    }

    fn ensure_focus_stack(&mut self) {
        self.focus_stack
            .retain(|key| self.children.contains(key));
        for child in &self.children {
            if !self.focus_stack.contains(child) {
                self.focus_stack.push(*child);
            }
        }
    }

    /// Add a child node by key
    pub fn add_child(&mut self, node_key: NodeKey) {
        let idx = self.children.len();
        self.insert_child(idx, node_key);
    }

    /// Remove a child at index, returns the removed node key
    pub fn remove_child(&mut self, idx: usize) -> Option<NodeKey> {
        if idx >= self.children.len() {
            return None;
        }

        let key = self.children.remove(idx);
        self.focus_stack.retain(|child| *child != key);
        let removed_percent = if self.child_percents.len() == self.children.len() + 1 {
            self.child_percents.remove(idx)
        } else {
            0.0
        };

        if self.children.is_empty() {
            self.child_percents.clear();
            self.focus_stack.clear();
            return Some(key);
        }

        if self.child_percents.len() != self.children.len() {
            self.recalculate_percentages();
            self.ensure_focus_stack();
            return Some(key);
        }

        let remaining = 1.0 - removed_percent;
        if remaining > f64::EPSILON {
            let scale = 1.0 / remaining;
            for percent in &mut self.child_percents {
                *percent *= scale;
            }
            self.normalize_child_percents();
        } else {
            self.recalculate_percentages();
        }

        self.ensure_focus_stack();
        Some(key)
    }

    /// Get child key at index
    pub fn child_key(&self, idx: usize) -> Option<NodeKey> {
        self.children.get(idx).copied()
    }

    pub fn insert_child(&mut self, idx: usize, node_key: NodeKey) {
        let idx = idx.min(self.children.len());
        let old_len = self.children.len();

        if old_len == 0 {
            self.children.insert(idx, node_key);
            self.focus_stack.push(node_key);
            self.child_percents.clear();
            self.child_percents.push(1.0);
            return;
        }

        if self.child_percents.len() != old_len {
            self.child_percents.clear();
            let value = 1.0 / old_len as f64;
            self.child_percents.resize(old_len, value);
        } else {
            self.normalize_child_percents();
        }

        let new_share = 1.0 / (old_len as f64 + 1.0);
        let scale = 1.0 - new_share;
        for percent in &mut self.child_percents {
            *percent *= scale;
        }

        self.children.insert(idx, node_key);
        self.child_percents.insert(idx, new_share);
        self.normalize_child_percents();
        if !self.focus_stack.contains(&node_key) {
            self.focus_stack.push(node_key);
        }
    }

    pub fn recalculate_percentages(&mut self) {
        if self.children.is_empty() {
            self.child_percents.clear();
            return;
        }
        let count = self.children.len() as f64;
        let value = 1.0 / count;
        if self.child_percents.len() != self.children.len() {
            self.child_percents.resize(self.children.len(), value);
        }
        for percent in &mut self.child_percents {
            *percent = value;
        }
    }

    pub fn normalize_child_percents(&mut self) {
        if self.child_percents.is_empty() {
            return;
        }
        let mut sum = 0.0;
        for percent in &self.child_percents {
            if !percent.is_finite() || *percent < 0.0 {
                sum = 0.0;
                break;
            }
            sum += *percent;
        }
        if sum <= f64::EPSILON {
            self.recalculate_percentages();
            return;
        }
        for percent in &mut self.child_percents {
            *percent /= sum;
        }
    }

    pub fn child_percent(&self, idx: usize) -> f64 {
        self.child_percents.get(idx).copied().unwrap_or(0.0)
    }

    /// Get child percentages as a slice (avoids cloning)
    pub fn child_percents_slice(&self) -> &[f64] {
        &self.child_percents
    }

    pub fn set_child_percent(&mut self, idx: usize, percent: f64) {
        if self.child_percents.len() != self.children.len() {
            self.recalculate_percentages();
        }

        if self.child_percents.is_empty() || idx >= self.child_percents.len() {
            return;
        }

        let len = self.child_percents.len();
        if len == 1 {
            self.child_percents[0] = 1.0;
            return;
        }

        let min = MIN_CHILD_PERCENT;
        let max = 1.0 - min * (len as f64 - 1.0);
        let new_percent = percent.clamp(min, max.max(min));

        self.child_percents[idx] = new_percent;

        let mut remaining = 1.0 - new_percent;
        if remaining <= f64::EPSILON {
            remaining = min * (len as f64 - 1.0);
        }

        let mut others_sum = 0.0;
        for (i, value) in self.child_percents.iter().enumerate() {
            if i != idx {
                others_sum += *value;
            }
        }

        if others_sum <= f64::EPSILON {
            let share = remaining / (len as f64 - 1.0);
            for (i, value) in self.child_percents.iter_mut().enumerate() {
                if i != idx {
                    *value = share;
                }
            }
        } else {
            let scale = remaining / others_sum;
            for (i, value) in self.child_percents.iter_mut().enumerate() {
                if i != idx {
                    *value *= scale;
                }
            }
        }

        self.normalize_child_percents();
    }

    pub fn set_child_percent_pair(&mut self, idx: usize, neighbor_idx: usize, percent: f64) -> bool {
        if self.child_percents.len() != self.children.len() {
            self.recalculate_percentages();
        }

        let len = self.child_percents.len();
        if len < 2 || idx >= len || neighbor_idx >= len || idx == neighbor_idx {
            return false;
        }

        let total = self.child_percents[idx] + self.child_percents[neighbor_idx];
        if total <= f64::EPSILON {
            return false;
        }

        let min = MIN_CHILD_PERCENT;
        if total < min * 2.0 {
            return false;
        }

        let max_target = total - min;
        let new_percent = percent.clamp(min, max_target);
        let neighbor_percent = total - new_percent;

        if (self.child_percents[idx] - new_percent).abs() <= f64::EPSILON
            && (self.child_percents[neighbor_idx] - neighbor_percent).abs() <= f64::EPSILON
        {
            return false;
        }

        self.child_percents[idx] = new_percent;
        self.child_percents[neighbor_idx] = neighbor_percent;
        true
    }

    /// Check if container is empty
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Get number of children
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Set geometry for this container
    pub fn set_geometry(&mut self, geometry: Rectangle<f64, Logical>) {
        self.geometry = geometry;
    }

    /// Get geometry
    pub fn geometry(&self) -> Rectangle<f64, Logical> {
        self.geometry
    }
}

// ============================================================================
// Detached subtree helpers
// ============================================================================

impl<W: LayoutElement> DetachedNode<W> {
    pub fn tiles(&self) -> Vec<&Tile<W>> {
        let mut tiles = Vec::new();
        self.collect_tiles(&mut tiles);
        tiles
    }

    fn collect_tiles<'a>(&'a self, tiles: &mut Vec<&'a Tile<W>>) {
        match self {
            DetachedNode::Leaf(tile) => tiles.push(tile),
            DetachedNode::Container(container) => {
                for child in &container.children {
                    child.collect_tiles(tiles);
                }
            }
        }
    }

    pub fn contains_window(&self, window_id: &W::Id) -> bool {
        match self {
            DetachedNode::Leaf(tile) => tile.window().id() == window_id,
            DetachedNode::Container(container) => container
                .children
                .iter()
                .any(|child| child.contains_window(window_id)),
        }
    }

    pub fn into_tiles(self) -> Vec<Tile<W>> {
        let mut tiles = Vec::new();
        self.collect_tiles_owned(&mut tiles);
        tiles
    }

    fn collect_tiles_owned(self, tiles: &mut Vec<Tile<W>>) {
        match self {
            DetachedNode::Leaf(tile) => tiles.push(tile),
            DetachedNode::Container(container) => {
                for child in container.children {
                    child.collect_tiles_owned(tiles);
                }
            }
        }
    }
}

impl<W: LayoutElement> DetachedContainer<W> {
    pub fn new(layout: Layout, children: Vec<DetachedNode<W>>) -> Self {
        let mut container = Self {
            layout,
            children,
            child_percents: Vec::new(),
            focus_stack: Vec::new(),
            preserve_on_single: false,
        };
        container.ensure_focus_stack();
        container.recalculate_percentages();
        container
    }

    pub(crate) fn from_parts(
        layout: Layout,
        children: Vec<DetachedNode<W>>,
        child_percents: Vec<f64>,
        focus_stack: Vec<usize>,
        preserve_on_single: bool,
    ) -> Self {
        let mut container = Self {
            layout,
            children,
            child_percents,
            focus_stack,
            preserve_on_single,
        };
        container.normalize_child_percents();
        container.ensure_focus_stack();
        container
    }

    fn recalculate_percentages(&mut self) {
        if self.children.is_empty() {
            self.child_percents.clear();
            return;
        }
        let count = self.children.len() as f64;
        let value = 1.0 / count;
        self.child_percents.clear();
        self.child_percents.resize(self.children.len(), value);
    }

    fn normalize_child_percents(&mut self) {
        if self.child_percents.len() != self.children.len() {
            self.recalculate_percentages();
            return;
        }
        if self.child_percents.is_empty() {
            return;
        }
        let mut sum = 0.0;
        for percent in &self.child_percents {
            if !percent.is_finite() || *percent < 0.0 {
                sum = 0.0;
                break;
            }
            sum += *percent;
        }
        if sum <= f64::EPSILON {
            self.recalculate_percentages();
            return;
        }
        for percent in &mut self.child_percents {
            *percent /= sum;
        }
    }

    fn ensure_focus_stack(&mut self) {
        self.focus_stack
            .retain(|idx| *idx < self.children.len());
        let mut seen = vec![false; self.children.len()];
        self.focus_stack.retain(|idx| {
            if seen[*idx] {
                false
            } else {
                seen[*idx] = true;
                true
            }
        });
        for idx in 0..self.children.len() {
            if !seen[idx] {
                self.focus_stack.push(idx);
            }
        }
    }
}

// ============================================================================
// ContainerTree Implementation
// ============================================================================

impl<W: LayoutElement> ContainerTree<W> {
    /// Create a new empty container tree
    pub fn new(
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
        scale: f64,
        options: Rc<Options>,
    ) -> Self {
        Self {
            nodes: SlotMap::with_key(),
            parents: SecondaryMap::new(),
            root: None,
            pending_layout: None,
            focused_key: None,
            selected_key: None,
            leaf_layouts: Vec::new(),
            pending_layouts: None,
            pending_transaction: None,
            pending_relayout: false,
            view_size,
            working_area,
            scale,
            options,
            generation: 0,
            focus_path_cache: RefCell::new((u64::MAX, None, Vec::new())),
            prev_positions_cache: HashMap::new(),
        }
    }

    pub(super) fn preview_new_leaf_geometry(&self) -> Option<PreviewLeafGeometry> {
        let root_rect = self.layout_area();
        let Some(root_key) = self.root else {
            if let Some(layout) = self.pending_layout {
                let (rect, tab_bar_offset) =
                    self.preview_child_rect(layout, root_rect, 1, &[1.0], 0, true);
                return Some(PreviewLeafGeometry {
                    rect,
                    tab_bar_offset,
                });
            }
            return Some(PreviewLeafGeometry {
                rect: root_rect,
                tab_bar_offset: 0.0,
            });
        };

        if matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
            let percents = self.preview_inserted_child_percents(&[], 1, 1);
            let (rect, tab_bar_offset) =
                self.preview_child_rect(Layout::SplitH, root_rect, 2, &percents, 1, true);
            return Some(PreviewLeafGeometry {
                rect,
                tab_bar_offset,
            });
        }

        let focus_path = self.focus_path();
        let (parent_path, insert_idx) = if focus_path.is_empty() {
            (Vec::new(), None)
        } else {
            let mut parent_path = focus_path.clone();
            let insert_idx = parent_path.pop().map(|idx| idx + 1);
            (parent_path, insert_idx)
        };

        let parent_key = if parent_path.is_empty() {
            root_key
        } else {
            self.get_node_key_at_path(&parent_path)?
        };
        let parent_rect = self.preview_rect_for_path(root_key, root_rect, &parent_path)?;
        let parent = self.get_container(parent_key)?;
        let child_count = parent.child_count();
        let insert_idx = insert_idx.unwrap_or(child_count).min(child_count);
        let percents = self.preview_inserted_child_percents(
            parent.child_percents_slice(),
            child_count,
            insert_idx,
        );
        let (rect, tab_bar_offset) = self.preview_child_rect(
            parent.layout(),
            parent_rect,
            child_count + 1,
            &percents,
            insert_idx,
            true,
        );

        Some(PreviewLeafGeometry {
            rect,
            tab_bar_offset,
        })
    }


    fn preview_rect_for_path(
        &self,
        root_key: NodeKey,
        root_rect: Rectangle<f64, Logical>,
        path: &[usize],
    ) -> Option<Rectangle<f64, Logical>> {
        let mut rect = root_rect;
        let mut node_key = root_key;
        for &idx in path {
            let container = self.get_container(node_key)?;
            let child_key = container.child_key(idx)?;
            let child_is_leaf = matches!(self.get_node(child_key), Some(NodeData::Leaf(_)));
            let percents_sum: f64 = container.child_percents_slice().iter().copied().sum();
            let percents = self.get_normalized_child_percents(
                node_key,
                container.child_count(),
                percents_sum,
            );
            let (child_rect, _) = self.preview_child_rect(
                container.layout(),
                rect,
                container.child_count(),
                &percents,
                idx,
                child_is_leaf,
            );
            if child_is_leaf {
                return None;
            }
            rect = child_rect;
            node_key = child_key;
        }
        Some(rect)
    }

    fn preview_inserted_child_percents(
        &self,
        current: &[f64],
        old_len: usize,
        insert_idx: usize,
    ) -> Vec<f64> {
        if old_len == 0 {
            return vec![1.0];
        }

        let mut percents = if current.len() == old_len {
            current.to_vec()
        } else {
            vec![1.0 / old_len as f64; old_len]
        };

        Self::normalize_child_percents_for_preview(&mut percents);

        let new_share = 1.0 / (old_len as f64 + 1.0);
        for percent in &mut percents {
            *percent *= 1.0 - new_share;
        }

        let insert_idx = insert_idx.min(percents.len());
        percents.insert(insert_idx, new_share);
        Self::normalize_child_percents_for_preview(&mut percents);
        percents
    }

    fn normalize_child_percents_for_preview(percents: &mut Vec<f64>) {
        if percents.is_empty() {
            return;
        }
        let mut sum = 0.0;
        for percent in percents.iter() {
            if !percent.is_finite() || *percent < 0.0 {
                sum = 0.0;
                break;
            }
            sum += *percent;
        }
        if sum <= f64::EPSILON {
            let value = 1.0 / percents.len() as f64;
            for percent in percents.iter_mut() {
                *percent = value;
            }
            return;
        }
        for percent in percents.iter_mut() {
            *percent /= sum;
        }
    }

    fn preview_child_rect(
        &self,
        layout: Layout,
        rect: Rectangle<f64, Logical>,
        child_count: usize,
        percents: &[f64],
        child_idx: usize,
        child_is_leaf: bool,
    ) -> (Rectangle<f64, Logical>, f64) {
        let gap = self.options.layout.gaps;
        match layout {
            Layout::SplitH => {
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_width = (rect.size.w - total_gap).max(0.0);
                let mut cursor_x = rect.loc.x;
                let mut used_width = 0.0;
                let split_bar_height = self.split_title_bar_height();
                for idx in 0..child_count {
                    let percent = percents
                        .get(idx)
                        .copied()
                        .unwrap_or(1.0 / child_count as f64);
                    let width = if idx == child_count - 1 {
                        (available_width - used_width).max(0.0)
                    } else {
                        (available_width * percent).max(0.0)
                    };
                    if idx == child_idx {
                        let child_rect = Rectangle::new(
                            Point::from((cursor_x, rect.loc.y)),
                            Size::from((width, rect.size.h)),
                        );
                        let tab_bar_offset = if child_is_leaf && split_bar_height > 0.0 {
                            split_bar_height
                        } else {
                            0.0
                        };
                        return (child_rect, tab_bar_offset);
                    }
                    used_width += width;
                    if idx + 1 < child_count {
                        cursor_x += width + gap;
                    }
                }
            }
            Layout::SplitV => {
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_height = (rect.size.h - total_gap).max(0.0);
                let mut cursor_y = rect.loc.y;
                let mut used_height = 0.0;
                let split_bar_height = self.split_title_bar_height();
                for idx in 0..child_count {
                    let percent = percents
                        .get(idx)
                        .copied()
                        .unwrap_or(1.0 / child_count as f64);
                    let height = if idx == child_count - 1 {
                        (available_height - used_height).max(0.0)
                    } else {
                        (available_height * percent).max(0.0)
                    };
                    if idx == child_idx {
                        let child_rect = Rectangle::new(
                            Point::from((rect.loc.x, cursor_y)),
                            Size::from((rect.size.w, height)),
                        );
                        let tab_bar_offset = if child_is_leaf && split_bar_height > 0.0 {
                            split_bar_height
                        } else {
                            0.0
                        };
                        return (child_rect, tab_bar_offset);
                    }
                    used_height += height;
                    if idx + 1 < child_count {
                        cursor_y += height + gap;
                    }
                }
            }
            Layout::Tabbed | Layout::Stacked => {
                let mut inner_rect = rect;
                if gap > 0.0 {
                    inner_rect.loc.x += gap;
                    inner_rect.loc.y += gap;
                    inner_rect.size.w = (inner_rect.size.w - gap * 2.0).max(0.0);
                    inner_rect.size.h = (inner_rect.size.h - gap * 2.0).max(0.0);
                }

                let bar_row_height = self.tab_bar_row_height();
                let mut tab_offset = 0.0;
                if bar_row_height > 0.0 && child_count > 0 {
                    let bar_height = match layout {
                        Layout::Tabbed => bar_row_height,
                        Layout::Stacked => bar_row_height * child_count as f64,
                        _ => 0.0,
                    };
                    let total_bar_height = (bar_height + self.tab_bar_spacing())
                        .min(inner_rect.size.h)
                        .max(0.0);
                    tab_offset = total_bar_height;
                }

                if child_is_leaf {
                    return (inner_rect, tab_offset);
                }

                let mut content_rect = inner_rect;
                if tab_offset > 0.0 {
                    content_rect.loc.y += tab_offset;
                    content_rect.size.h = (content_rect.size.h - tab_offset).max(0.0);
                }
                return (content_rect, 0.0);
            }
        }

        (rect, 0.0)
    }

    // ========================================================================
    // Internal SlotMap helpers
    // ========================================================================

    /// Get node data by key
    fn get_node(&self, key: NodeKey) -> Option<&NodeData<W>> {
        self.nodes.get(key)
    }

    /// Get mutable node data by key
    fn get_node_mut(&mut self, key: NodeKey) -> Option<&mut NodeData<W>> {
        self.nodes.get_mut(key)
    }

    /// Get container data by key
    fn get_container(&self, key: NodeKey) -> Option<&ContainerData> {
        match self.nodes.get(key)? {
            NodeData::Container(container) => Some(container),
            _ => None,
        }
    }

    /// Get mutable container data by key
    fn get_container_mut(&mut self, key: NodeKey) -> Option<&mut ContainerData> {
        match self.nodes.get_mut(key)? {
            NodeData::Container(container) => Some(container),
            _ => None,
        }
    }

    fn set_parent(&mut self, child: NodeKey, parent: Option<NodeKey>) {
        if let Some(entry) = self.parents.get_mut(child) {
            *entry = parent;
        } else {
            self.parents.insert(child, parent);
        }
    }

    fn parent_of(&self, key: NodeKey) -> Option<NodeKey> {
        self.parents.get(key).and_then(|parent| *parent)
    }

    fn child_index(&self, parent_key: NodeKey, child_key: NodeKey) -> Option<usize> {
        self.get_container(parent_key)?
            .children
            .iter()
            .position(|&key| key == child_key)
    }

    /// Get tile by key (O(1) access).
    pub fn get_tile(&self, key: NodeKey) -> Option<&Tile<W>> {
        match self.nodes.get(key)? {
            NodeData::Leaf(tile) => Some(tile),
            _ => None,
        }
    }

    /// Get mutable tile by key (O(1) access).
    pub fn get_tile_mut(&mut self, key: NodeKey) -> Option<&mut Tile<W>> {
        match self.nodes.get_mut(key)? {
            NodeData::Leaf(tile) => Some(tile),
            _ => None,
        }
    }

    /// Insert a new node into the slotmap
    fn insert_node(&mut self, node: NodeData<W>) -> NodeKey {
        let key = self.nodes.insert(node);
        self.parents.insert(key, None);
        key
    }

    /// Remove a node from the slotmap (and recursively all its children)
    fn remove_node_recursive(&mut self, key: NodeKey) -> Option<NodeData<W>> {
        let node = self.nodes.remove(key)?;
        self.parents.remove(key);

        // If it's a container, recursively remove all children
        if let NodeData::Container(ref container) = node {
            for &child_key in &container.children {
                self.remove_node_recursive(child_key);
            }
        }

        Some(node)
    }

    // ========================================================================
    // Public API
    // ========================================================================

    /// Check if tree is empty
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Insert a window into the tree
    pub fn insert_window(&mut self, tile: Tile<W>) {
        self.clear_focus_history();

        if self.root.is_none() {
            // First window becomes the root leaf
            let tile_key = self.insert_node(NodeData::Leaf(tile));
            if let Some(layout) = self.pending_layout.take() {
                let mut container = ContainerData::new(layout);
                container.mark_preserve_on_single();
                container.add_child(tile_key);
                let container_key = self.insert_node(NodeData::Container(container));
                self.set_parent(tile_key, Some(container_key));
                self.set_parent(container_key, None);
                self.root = Some(container_key);
            } else {
                self.set_parent(tile_key, None);
                self.root = Some(tile_key);
            }
            self.focus_node_key(tile_key);
            return;
        }

        // Ensure the root is a container so we can insert siblings easily
        let root_key = self.root.unwrap();
        if matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
            // Convert the root leaf into a container
            let old_root_key = self.root.take().unwrap();
            let mut container = ContainerData::new(Layout::SplitH);
            container.add_child(old_root_key);

            let container_key = self.insert_node(NodeData::Container(container));
            self.set_parent(old_root_key, Some(container_key));
            self.set_parent(container_key, None);
            self.root = Some(container_key);
            self.focus_node_key(old_root_key);
        }
        let focus_path = self.focus_path();

        // Insert as sibling in the parent container
        if focus_path.is_empty() {
            // Append to root container
            if let Some(root_key) = self.root {
                let tile_key = self.insert_node(NodeData::Leaf(tile));
                let mut inserted = false;
                if let Some(NodeData::Container(container)) = self.get_node_mut(root_key) {
                    let insert_idx = container.children.len();
                    container.insert_child(insert_idx, tile_key);
                    inserted = true;
                }
                if inserted {
                    self.set_parent(tile_key, Some(root_key));
                    self.focus_node_key(tile_key);
                }
            }
            return;
        }

        let parent_path = &focus_path[..focus_path.len() - 1];
        let current_idx = *focus_path.last().unwrap();

        // Get parent container and insert
        let tile_key = self.insert_node(NodeData::Leaf(tile));
        if let Some(parent_key) = self.get_node_key_at_path(parent_path) {
            let mut inserted = false;
            if let Some(NodeData::Container(parent_container)) = self.get_node_mut(parent_key) {
                let insert_idx = current_idx + 1;
                parent_container.insert_child(insert_idx, tile_key);

                inserted = true;
            }
            if inserted {
                self.set_parent(tile_key, Some(parent_key));
                self.focus_node_key(tile_key);
                return;
            }
        }

        // Fallback: append to root container
        if let Some(root_key) = self.root {
            let mut inserted = false;
            if let Some(NodeData::Container(container)) = self.get_node_mut(root_key) {
                let insert_idx = container.children.len();
                container.insert_child(insert_idx, tile_key);
                inserted = true;
            }
            if inserted {
                self.set_parent(tile_key, Some(root_key));
                self.focus_node_key(tile_key);
            }
        }
    }

    /// Helper: get node key at path
    fn get_node_key_at_path(&self, path: &[usize]) -> Option<NodeKey> {
        if path.is_empty() {
            return self.root;
        }

        let mut current_key = self.root?;

        for &idx in path {
            match self.get_node(current_key)? {
                NodeData::Container(container) => {
                    current_key = container.child_key(idx)?;
                }
                NodeData::Leaf(_) => return None,
            }
        }

        Some(current_key)
    }

    fn sync_container_focus_from_key(&mut self, key: NodeKey) {
        let mut current = key;
        while let Some(parent_key) = self.parent_of(current) {
            if let Some(container) = self.get_container_mut(parent_key) {
                container.bubble_focus(current);
            }
            current = parent_key;
        }
    }

    fn leaf_under_key(&self, mut key: NodeKey) -> Option<NodeKey> {
        loop {
            match self.get_node(key)? {
                NodeData::Leaf(_) => return Some(key),
                NodeData::Container(container) => {
                    if container.children.is_empty() {
                        return None;
                    }
                    key = container.focused_child_key()?;
                }
            }
        }
    }

    fn first_leaf_key(&self) -> Option<NodeKey> {
        let root_key = self.root?;
        self.leaf_under_key(root_key)
    }

    fn focus_node_key(&mut self, key: NodeKey) {
        let Some(leaf_key) = self.leaf_under_key(key) else {
            self.focused_key = None;
            self.selected_key = None;
            return;
        };
        self.focused_key = Some(leaf_key);
        self.selected_key = None;
        self.sync_container_focus_from_key(leaf_key);
    }

    /// Find a node by key and return path to it.
    fn find_node_path(&self, target_key: NodeKey) -> Option<Vec<usize>> {
        let root_key = self.root?;
        if target_key == root_key {
            return Some(Vec::new());
        }

        let mut path_rev = Vec::new();
        let mut current = target_key;
        while current != root_key {
            let parent = self.parent_of(current)?;
            let idx = self.child_index(parent, current)?;
            path_rev.push(idx);
            current = parent;
        }

        path_rev.reverse();
        Some(path_rev)
    }

    fn clear_focus_history(&mut self) {
        // Focus history is tracked per-container via focus_stack.
    }

    /// Find a window by ID and return path to it
    pub fn find_window(&self, window_id: &W::Id) -> Option<Vec<usize>> {
        let root_key = self.root?;
        let mut path = Vec::new();
        self.find_window_in_node(root_key, window_id, &mut path)
    }

    /// Helper: recursively find window in node
    fn find_window_in_node(
        &self,
        node_key: NodeKey,
        window_id: &W::Id,
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        match self.get_node(node_key)? {
            NodeData::Leaf(tile) => {
                if tile.window().id() == window_id {
                    Some(path.clone())
                } else {
                    None
                }
            }
            NodeData::Container(container) => {
                for (idx, &child_key) in container.children.iter().enumerate() {
                    path.push(idx);
                    if let Some(result) = self.find_window_in_node(child_key, window_id, path) {
                        return Some(result);
                    }
                    path.pop();
                }
                None
            }
        }
    }

    /// Get the currently focused window
    pub fn focused_window(&self) -> Option<&W> {
        let key = self.focused_key?;
        self.get_tile(key).map(|tile| tile.window())
    }

    /// Get the currently focused window (mutable)
    pub fn focused_window_mut(&mut self) -> Option<&mut W> {
        let key = self.focused_key?;
        self.get_tile_mut(key).map(|tile| tile.window_mut())
    }

    /// Update view size and working area
    pub fn set_view_size(
        &mut self,
        view_size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
    ) {
        self.view_size = view_size;
        self.working_area = working_area;
    }

    /// Update configuration
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
        self.options = options;
    }

    /// Count total number of windows in tree
    pub fn window_count(&self) -> usize {
        self.root
            .map_or(0, |root_key| self.count_windows_in_node(root_key))
    }

    /// Helper: count windows in a node
    fn count_windows_in_node(&self, node_key: NodeKey) -> usize {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(_)) => 1,
            Some(NodeData::Container(container)) => container
                .children
                .iter()
                .map(|&child_key| self.count_windows_in_node(child_key))
                .sum(),
            None => 0,
        }
    }

    /// Access the cached leaf layout information from the last layout pass.
    pub fn leaf_layouts(&self) -> &[LeafLayoutInfo] {
        &self.leaf_layouts
    }

    /// Clone of the cached leaf layout information
    pub fn leaf_layouts_cloned(&self) -> Vec<LeafLayoutInfo> {
        self.leaf_layouts.clone()
    }

    pub fn pending_leaf_layouts(&self) -> Option<&[LeafLayoutInfo]> {
        self.pending_layouts
            .as_ref()
            .map(|pending| pending.data.leaf_layouts.as_slice())
    }

    pub fn pending_leaf_layouts_cloned(&self) -> Option<Vec<LeafLayoutInfo>> {
        self.pending_layouts
            .as_ref()
            .map(|pending| pending.data.leaf_layouts.clone())
    }

    pub fn set_pending_transaction(&mut self, transaction: Transaction) {
        self.pending_transaction = Some(transaction);
    }

    fn prune_leaf_layouts(&mut self) {
        self.leaf_layouts
            .retain(|info| self.nodes.contains_key(info.key));
        if let Some(pending) = &mut self.pending_layouts {
            pending
                .data
                .leaf_layouts
                .retain(|info| self.nodes.contains_key(info.key));
        }
    }

    fn debug_layout_state(&self, context: &'static str) {
        let window_count = self.window_count();
        let leaf_count = self.leaf_layouts.len();
        let pending_leaf_count = self
            .pending_layouts
            .as_ref()
            .map(|pending| pending.data.leaf_layouts.len())
            .unwrap_or(0);
        let has_pending = self.pending_layouts.is_some();

        if window_count <= 3 {
            debug!(
                context = context,
                window_count,
                leaf_count,
                pending_leaf_count,
                has_pending,
                working_area = ?self.working_area,
                view_size = ?self.view_size,
                scale = self.scale,
                root = ?self.root,
                focused = ?self.focused_key,
                "layout summary"
            );
            for info in &self.leaf_layouts {
                debug!(
                    context = context,
                    key = ?info.key,
                    rect = ?info.rect,
                    visible = info.visible,
                    path = ?info.path,
                    "leaf layout"
                );
            }
            if let Some(pending) = &self.pending_layouts {
                for info in &pending.data.leaf_layouts {
                    debug!(
                        context = context,
                        key = ?info.key,
                        rect = ?info.rect,
                        visible = info.visible,
                        path = ?info.path,
                        "pending leaf layout"
                    );
                }
            }
        }

        if leaf_count == 0 && pending_leaf_count > 0 {
            debug!(
                context = context,
                window_count,
                pending_leaf_count,
                "layout has no leaf layouts but pending exists"
            );
        }
        if window_count != leaf_count {
            debug!(
                context = context,
                window_count,
                leaf_count,
                pending_leaf_count,
                has_pending,
                "layout window/leaf mismatch"
            );
        }

        let zero_size = self
            .leaf_layouts
            .iter()
            .filter(|info| info.rect.size.w <= 0.0 || info.rect.size.h <= 0.0)
            .count();
        if zero_size > 0 {
            debug!(context = context, zero_size, "layout has zero-size leafs");
        }
    }

    /// Current focus path within the tree.
    /// Uses cached path when generation and focused_key haven't changed.
    pub fn focus_path(&self) -> Vec<usize> {
        {
            let cache = self.focus_path_cache.borrow();
            if cache.0 == self.generation && cache.1 == self.focused_key {
                if let Some(key) = self.focused_key {
                    if self.get_node_key_at_path(&cache.2) == Some(key) {
                        return cache.2.clone();
                    }
                }
            }
        }

        // Recompute path with fallback when focused key is invalid.
        let path = if let Some(key) = self.focused_key {
            self.find_node_path(key).or_else(|| {
                self.first_leaf_key()
                    .and_then(|first_key| self.find_node_path(first_key))
            })
        } else {
            self.first_leaf_key()
                .and_then(|first_key| self.find_node_path(first_key))
        }
        .unwrap_or_default();

        // Update cache
        let mut cache = self.focus_path_cache.borrow_mut();
        cache.0 = self.generation;
        cache.1 = self.focused_key;
        cache.2 = path.clone();
        path
    }

    pub fn selected_path(&self) -> Vec<usize> {
        if let Some(key) = self.selected_key {
            if let Some(path) = self.find_node_path(key) {
                return path;
            }
        }
        self.focus_path()
    }

    pub fn selected_node_key(&self) -> Option<NodeKey> {
        if let Some(key) = self.selected_key {
            if self.get_node(key).is_some() {
                return Some(key);
            }
        }
        self.focused_key.or_else(|| self.first_leaf_key())
    }

    pub fn selected_is_container(&self) -> bool {
        self.selected_key.is_some_and(|key| matches!(self.get_node(key), Some(NodeData::Container(_))))
    }

    pub fn clear_selection(&mut self) {
        self.selected_key = None;
    }

    pub fn select_parent(&mut self) -> bool {
        let base_key = self
            .selected_key
            .or(self.focused_key)
            .or_else(|| self.first_leaf_key());
        let Some(base_key) = base_key else {
            return false;
        };
        let Some(parent_key) = self.parent_of(base_key) else {
            return false;
        };
        self.selected_key = Some(parent_key);
        true
    }

    pub fn select_child(&mut self) -> bool {
        let Some(selected_key) = self.selected_key else {
            return false;
        };
        let Some(container) = self.get_container(selected_key) else {
            return false;
        };
        let Some(child_key) = container.focused_child_key() else {
            return false;
        };
        self.selected_key = Some(child_key);
        true
    }

    /// Focused tile (if any).
    pub fn focused_tile(&self) -> Option<&Tile<W>> {
        let key = self.focused_key.or_else(|| self.first_leaf_key())?;
        self.get_tile(key)
    }

    /// Focused tile (mutable) if any.
    pub fn focused_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        let key = self.focused_key.or_else(|| self.first_leaf_key())?;
        self.get_tile_mut(key)
    }

    /// Calculate and apply layout to the tree
    pub fn layout(&mut self) {
        self.layout_with_resize_animation(true);
    }

    /// Calculate and apply layout to the tree, with control over resize animation.
    pub fn layout_with_resize_animation(&mut self, animate_resize: bool) {
        let animate = !self.options.animations.off;
        self.layout_with_animations(animate, animate_resize);
    }

    /// Calculate and apply layout to the tree with explicit animation flags.
    pub fn layout_with_animation_flags(&mut self, animate: bool, animate_resize: bool) {
        self.layout_with_animations(animate, animate_resize);
    }

    fn layout_with_animations(&mut self, animate: bool, animate_resize: bool) {
        // Increment generation for focus path caching.
        self.generation = self.generation.wrapping_add(1);

        if self.should_use_atomic_layout() {
            self.layout_atomic(animate_resize);
            return;
        }

        self.pending_layouts = None;
        self.pending_transaction = None;
        self.pending_relayout = false;

        self.prev_positions_cache.clear();
        if animate {
            for info in &self.leaf_layouts {
                self.prev_positions_cache.insert(info.key, info.rect.loc);
            }
        }

        self.leaf_layouts.clear();

        if let Some(root_key) = self.root {
            let mut path = Vec::new();
            let area = self.layout_area();
            self.layout_node(
                root_key,
                area,
                &mut path,
                true,
                animate,
                animate_resize,
                0.0,
                false,
            );
        }

        if animate {
            // Iterate by index to avoid cloning leaf_layouts
            for i in 0..self.leaf_layouts.len() {
                let key = self.leaf_layouts[i].key;
                let rect_loc = self.leaf_layouts[i].rect.loc;
                let prev_loc = self.prev_positions_cache.get(&key).copied();
                if let Some(tile) = self.get_tile_mut(key) {
                    if let Some(prev_loc) = prev_loc {
                        let delta = prev_loc - rect_loc;
                        if delta.x.abs() > MOVE_ANIMATION_THRESHOLD
                            || delta.y.abs() > MOVE_ANIMATION_THRESHOLD
                        {
                            tile.animate_move_from(delta);
                        }
                    } else {
                        tile.start_open_animation();
                    }
                }
            }
        }

        self.debug_layout_state("layout");
    }

    fn should_use_atomic_layout(&self) -> bool {
        !self.options.disable_transactions
    }

    pub fn layout_area(&self) -> Rectangle<f64, Logical> {
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

    pub(super) fn parent_layout_for_path(&self, path: &[usize]) -> Option<Layout> {
        if path.is_empty() {
            return None;
        }

        let parent_path = &path[..path.len() - 1];
        let parent_key = if parent_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(parent_path)?
        };
        self.get_container(parent_key).map(|c| c.layout())
    }

    pub(super) fn single_child_split_layout_for_path(&self, path: &[usize]) -> Option<Layout> {
        if path.is_empty() {
            return None;
        }

        let parent_path = &path[..path.len() - 1];
        let parent_key = if parent_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(parent_path)?
        };

        let container = self.get_container(parent_key)?;
        if container.child_count() != 1 || !container.preserve_on_single() {
            return None;
        }

        match container.layout() {
            Layout::SplitH | Layout::SplitV => Some(container.layout()),
            _ => None,
        }
    }

    fn layout_atomic(&mut self, animate_resize: bool) {
        if self.pending_layouts.is_some() && !self.apply_pending_layouts_if_ready() {
            self.pending_relayout = true;
            self.debug_layout_state("layout_atomic_pending");
            return;
        }
        self.pending_relayout = false;

        let Some(root_key) = self.root else {
            self.leaf_layouts.clear();
            self.pending_layouts = None;
            self.pending_transaction = None;
            self.pending_relayout = false;
            self.debug_layout_state("layout_atomic_empty");
            return;
        };

        let data = self.collect_layout_data(root_key);
        let changed = self.changed_layout_keys(&data);
        if changed.is_empty() {
            self.pending_layouts = None;
            self.pending_transaction = None;
            self.apply_layout_data(data);
            self.debug_layout_state("layout_atomic_apply");
            return;
        }

        let transaction = self
            .pending_transaction
            .take()
            .unwrap_or_else(Transaction::new);
        self.request_sizes_for_layout(&data, &changed, &transaction, animate_resize);
        let should_apply_now = transaction.is_last();
        self.pending_layouts = Some(PendingLayout {
            data,
            blocker: transaction.blocker(),
        });
        drop(transaction);
        if should_apply_now && self.apply_pending_layouts_if_ready() {
            return;
        }
        self.debug_layout_state("layout_atomic_requested");
    }

    pub fn apply_pending_layouts_if_ready(&mut self) -> bool {
        let Some(pending) = &self.pending_layouts else {
            return false;
        };
        if pending.blocker.state() != BlockerState::Released {
            return false;
        }
        let pending = self.pending_layouts.take().unwrap();
        self.apply_layout_data(pending.data);
        self.debug_layout_state("layout_atomic_apply_pending");
        true
    }

    pub fn has_pending_layouts(&self) -> bool {
        self.pending_layouts.is_some()
    }

    pub fn take_pending_relayout(&mut self) -> bool {
        std::mem::take(&mut self.pending_relayout)
    }

    fn layout_request_for(
        &self,
        tile: &Tile<W>,
        tile_size: Size<f64, Logical>,
        tab_offset: f64,
    ) -> LayoutRequest {
        if tile.window().pending_sizing_mode().is_fullscreen() {
            LayoutRequest {
                mode: LayoutRequestMode::Fullscreen,
                size: self.view_size.to_i32_round(),
            }
        } else if tile.pending_maximized {
            LayoutRequest {
                mode: LayoutRequestMode::Maximized,
                size: tile_size.to_i32_round(),
            }
        } else {
            LayoutRequest {
                mode: LayoutRequestMode::Normal,
                size: tile.requested_window_size_for_tile(tile_size, tab_offset),
            }
        }
    }

    fn collect_layout_data(&self, root_key: NodeKey) -> LayoutData {
        let mut data = LayoutData {
            leaf_layouts: Vec::new(),
            container_geometries: HashMap::new(),
            tab_bar_offsets: HashMap::new(),
            titlebar_flags: HashMap::new(),
        };

        let mut path = Vec::new();
        let area = self.layout_area();
        self.collect_layout_node(root_key, area, &mut path, true, 0.0, false, &mut data);
        data
    }

    fn collect_layout_node(
        &self,
        node_key: NodeKey,
        rect: Rectangle<f64, Logical>,
        path: &mut Vec<usize>,
        visible: bool,
        tab_bar_offset: f64,
        draw_titlebar: bool,
        data: &mut LayoutData,
    ) {
        let (layout, child_count, focused_idx, child_percents_sum) = match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => {
                let (offset, show_titlebar) = if tile.window().pending_sizing_mode().is_fullscreen()
                {
                    (0.0, false)
                } else {
                    (tab_bar_offset, draw_titlebar)
                };
                data.tab_bar_offsets.insert(node_key, offset);
                data.titlebar_flags.insert(node_key, show_titlebar);
                data.leaf_layouts.push(LeafLayoutInfo {
                    key: node_key,
                    path: path.clone(),
                    rect,
                    visible,
                });
                return;
            }
            Some(NodeData::Container(container)) => {
                data.container_geometries.insert(node_key, rect);
                let percents = container.child_percents_slice();
                let sum: f64 = percents.iter().copied().sum();
                (
                    container.layout(),
                    container.child_count(),
                    container.focused_child_index(),
                    sum,
                )
            }
            None => return,
        };

        if child_count == 0 {
            return;
        }

        let gap = self.options.layout.gaps;

        match layout {
            Layout::SplitH => {
                let split_bar_height = self.split_title_bar_height();
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_width = (rect.size.w - total_gap).max(0.0);

                // Pre-compute normalized percentages
                let percents: Vec<f64> = self.get_normalized_child_percents(node_key, child_count, child_percents_sum);

                let mut cursor_x = rect.loc.x;
                let mut used_width = 0.0;

                for idx in 0..child_count {
                    let Some(child_key) = self.get_container_child_at(node_key, idx) else {
                        continue;
                    };
                    let percent = percents.get(idx).copied().unwrap_or(1.0 / child_count as f64);
                    let width = if idx == child_count - 1 {
                        (available_width - used_width).max(0.0)
                    } else {
                        (available_width * percent).max(0.0)
                    };

                    let child_rect = Rectangle::new(
                        Point::from((cursor_x, rect.loc.y)),
                        Size::from((width, rect.size.h)),
                    );

                    path.push(idx);
                    let (child_offset, child_titlebar) =
                        self.split_child_titlebar(child_key, split_bar_height);
                    self.collect_layout_node(
                        child_key,
                        child_rect,
                        path,
                        visible,
                        child_offset,
                        child_titlebar,
                        data,
                    );
                    path.pop();

                    used_width += width;
                    if idx + 1 < child_count {
                        cursor_x += width + gap;
                    }
                }
            }
            Layout::SplitV => {
                let split_bar_height = self.split_title_bar_height();
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_height = (rect.size.h - total_gap).max(0.0);

                // Pre-compute normalized percentages
                let percents: Vec<f64> = self.get_normalized_child_percents(node_key, child_count, child_percents_sum);

                let mut cursor_y = rect.loc.y;
                let mut used_height = 0.0;

                for idx in 0..child_count {
                    let Some(child_key) = self.get_container_child_at(node_key, idx) else {
                        continue;
                    };
                    let percent = percents.get(idx).copied().unwrap_or(1.0 / child_count as f64);
                    let height = if idx == child_count - 1 {
                        (available_height - used_height).max(0.0)
                    } else {
                        (available_height * percent).max(0.0)
                    };

                    let child_rect = Rectangle::new(
                        Point::from((rect.loc.x, cursor_y)),
                        Size::from((rect.size.w, height)),
                    );

                    path.push(idx);
                    let (child_offset, child_titlebar) =
                        self.split_child_titlebar(child_key, split_bar_height);
                    self.collect_layout_node(
                        child_key,
                        child_rect,
                        path,
                        visible,
                        child_offset,
                        child_titlebar,
                        data,
                    );
                    path.pop();

                    used_height += height;
                    if idx + 1 < child_count {
                        cursor_y += height + gap;
                    }
                }
            }
            Layout::Tabbed | Layout::Stacked => {
                let mut inner_rect = rect;
                if gap > 0.0 {
                    inner_rect.loc.x += gap;
                    inner_rect.loc.y += gap;
                    inner_rect.size.w = (inner_rect.size.w - gap * 2.0).max(0.0);
                    inner_rect.size.h = (inner_rect.size.h - gap * 2.0).max(0.0);
                }

                let bar_row_height = self.tab_bar_row_height();
                let mut tab_offset = 0.0;
                if bar_row_height > 0.0 && child_count > 0 {
                    let bar_height = match layout {
                        Layout::Tabbed => bar_row_height,
                        Layout::Stacked => bar_row_height * child_count as f64,
                        _ => 0.0,
                    };
                    let total_bar_height = (bar_height + self.tab_bar_spacing())
                        .min(inner_rect.size.h)
                        .max(0.0);
                    tab_offset = total_bar_height;
                }

                let focused_idx = focused_idx.unwrap_or(0).min(child_count.saturating_sub(1));

                for idx in 0..child_count {
                    let Some(child_key) = self.get_container_child_at(node_key, idx) else {
                        continue;
                    };
                    path.push(idx);
                    let child_visible = visible && idx == focused_idx;
                    let is_leaf = matches!(self.get_node(child_key), Some(NodeData::Leaf(_)));
                    let (child_rect, child_offset) = if is_leaf {
                        (inner_rect, tab_offset)
                    } else {
                        let mut content_rect = inner_rect;
                        if tab_offset > 0.0 {
                            content_rect.loc.y += tab_offset;
                            content_rect.size.h = (content_rect.size.h - tab_offset).max(0.0);
                        }
                        (content_rect, 0.0)
                    };
                    self.collect_layout_node(
                        child_key,
                        child_rect,
                        path,
                        child_visible,
                        child_offset,
                        false,
                        data,
                    );
                    path.pop();
                }
            }
        }
    }

    fn changed_layout_keys(&self, data: &LayoutData) -> HashSet<NodeKey> {
        let mut current = HashMap::new();
        for info in &self.leaf_layouts {
            let Some(tile) = self.get_tile(info.key) else {
                continue;
            };
            let request =
                self.layout_request_for(tile, info.rect.size, tile.tab_bar_offset());
            current.insert(info.key, request);
        }

        let mut changed = HashSet::new();
        for info in &data.leaf_layouts {
            let offset = data.tab_bar_offsets.get(&info.key).copied().unwrap_or(0.0);
            let Some(tile) = self.get_tile(info.key) else {
                changed.insert(info.key);
                continue;
            };
            let request = self.layout_request_for(tile, info.rect.size, offset);
            if current.get(&info.key).map_or(true, |old| *old != request) {
                changed.insert(info.key);
            }
        }

        changed
    }

    fn request_sizes_for_layout(
        &mut self,
        data: &LayoutData,
        changed: &HashSet<NodeKey>,
        transaction: &Transaction,
        animate_resize: bool,
    ) {
        for info in &data.leaf_layouts {
            let Some(tile) = self.get_tile_mut(info.key) else {
                continue;
            };
            let offset = data.tab_bar_offsets.get(&info.key).copied().unwrap_or(0.0);
            let show_titlebar = data
                .titlebar_flags
                .get(&info.key)
                .copied()
                .unwrap_or(false);
            let old_offset = tile.tab_bar_offset();
            let old_titlebar = tile.draw_titlebar();
            tile.set_tab_bar_offset(offset);
            tile.set_draw_titlebar(show_titlebar);

            let tx = changed
                .contains(&info.key)
                .then(|| transaction.clone());
            let size = Size::from((info.rect.size.w, info.rect.size.h));
            if tile.window().pending_sizing_mode().is_fullscreen() {
                tile.request_fullscreen(animate_resize, tx);
            } else if tile.pending_maximized {
                tile.request_maximized(size, animate_resize, tx);
            } else {
                tile.request_tile_size(size, animate_resize, tx);
            }

            tile.set_tab_bar_offset(old_offset);
            tile.set_draw_titlebar(old_titlebar);
        }
    }

    fn apply_layout_data(&mut self, data: LayoutData) {
        for (key, rect) in data.container_geometries {
            if let Some(NodeData::Container(container)) = self.get_node_mut(key) {
                container.set_geometry(rect);
            }
        }
        for (key, offset) in data.tab_bar_offsets {
            if let Some(tile) = self.get_tile_mut(key) {
                tile.set_tab_bar_offset(offset);
            }
        }
        for (key, show_titlebar) in data.titlebar_flags {
            if let Some(tile) = self.get_tile_mut(key) {
                tile.set_draw_titlebar(show_titlebar);
            }
        }
        self.leaf_layouts = data.leaf_layouts;
    }

    /// Helper: recursively layout a node
    fn layout_node(
        &mut self,
        node_key: NodeKey,
        rect: Rectangle<f64, Logical>,
        path: &mut Vec<usize>,
        visible: bool,
        animate: bool,
        animate_resize: bool,
        tab_bar_offset: f64,
        draw_titlebar: bool,
    ) {
        // We need to work around borrow checker by getting info first
        let (layout, child_count, focused_idx, child_percents_sum) = match self.get_node(node_key) {
            Some(NodeData::Leaf(_)) => {
                // Handle leaf
                if let Some(NodeData::Leaf(tile)) = self.get_node_mut(node_key) {
                    let (offset, show_titlebar) = if tile
                        .window()
                        .pending_sizing_mode()
                        .is_fullscreen()
                    {
                        (0.0, false)
                    } else {
                        (tab_bar_offset, draw_titlebar)
                    };
                    tile.set_tab_bar_offset(offset);
                    tile.set_draw_titlebar(show_titlebar);
                    let size = Size::from((rect.size.w, rect.size.h));
                    if tile.window().pending_sizing_mode().is_fullscreen() {
                        tile.request_fullscreen(animate_resize, None);
                    } else if tile.pending_maximized {
                        tile.request_maximized(size, animate_resize, None);
                    } else {
                        tile.request_tile_size(size, animate_resize, None);
                    }
                    self.leaf_layouts.push(LeafLayoutInfo {
                        key: node_key,
                        path: path.clone(),
                        rect,
                        visible,
                    });
                }
                return;
            }
            Some(NodeData::Container(container)) => {
                // Extract only Copy types to avoid cloning Vec
                let percents = container.child_percents_slice();
                let sum: f64 = percents.iter().copied().sum();
                (
                    container.layout(),
                    container.child_count(),
                    container.focused_child_index(),
                    sum,
                )
            }
            None => return,
        };

        // Update container geometry
        if let Some(NodeData::Container(container)) = self.get_node_mut(node_key) {
            container.set_geometry(rect);
        }

        if child_count == 0 {
            return;
        }

        let gap = self.options.layout.gaps;

        match layout {
            Layout::SplitH => {
                // Horizontal split
                let split_bar_height = self.split_title_bar_height();
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_width = (rect.size.w - total_gap).max(0.0);

                // Pre-compute normalized percentages
                let percents: Vec<f64> = self.get_normalized_child_percents(node_key, child_count, child_percents_sum);

                let mut cursor_x = rect.loc.x;
                let mut used_width = 0.0;

                for idx in 0..child_count {
                    let Some(child_key) = self.get_container_child_at(node_key, idx) else {
                        continue;
                    };
                    let percent = percents.get(idx).copied().unwrap_or(1.0 / child_count as f64);
                    let width = if idx == child_count - 1 {
                        (available_width - used_width).max(0.0)
                    } else {
                        (available_width * percent).max(0.0)
                    };

                    let child_rect = Rectangle::new(
                        Point::from((cursor_x, rect.loc.y)),
                        Size::from((width, rect.size.h)),
                    );

                    path.push(idx);
                    let (child_offset, child_titlebar) =
                        self.split_child_titlebar(child_key, split_bar_height);
                    self.layout_node(
                        child_key,
                        child_rect,
                        path,
                        visible,
                        animate,
                        animate_resize,
                        child_offset,
                        child_titlebar,
                    );
                    path.pop();

                    used_width += width;
                    if idx + 1 < child_count {
                        cursor_x += width + gap;
                    }
                }
            }
            Layout::SplitV => {
                // Vertical split
                let split_bar_height = self.split_title_bar_height();
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_height = (rect.size.h - total_gap).max(0.0);

                // Pre-compute normalized percentages
                let percents: Vec<f64> = self.get_normalized_child_percents(node_key, child_count, child_percents_sum);

                let mut cursor_y = rect.loc.y;
                let mut used_height = 0.0;

                for idx in 0..child_count {
                    let Some(child_key) = self.get_container_child_at(node_key, idx) else {
                        continue;
                    };
                    let percent = percents.get(idx).copied().unwrap_or(1.0 / child_count as f64);
                    let height = if idx == child_count - 1 {
                        (available_height - used_height).max(0.0)
                    } else {
                        (available_height * percent).max(0.0)
                    };

                    let child_rect = Rectangle::new(
                        Point::from((rect.loc.x, cursor_y)),
                        Size::from((rect.size.w, height)),
                    );

                    path.push(idx);
                    let (child_offset, child_titlebar) =
                        self.split_child_titlebar(child_key, split_bar_height);
                    self.layout_node(
                        child_key,
                        child_rect,
                        path,
                        visible,
                        animate,
                        animate_resize,
                        child_offset,
                        child_titlebar,
                    );
                    path.pop();

                    used_height += height;
                    if idx + 1 < child_count {
                        cursor_y += height + gap;
                    }
                }
            }
            Layout::Tabbed | Layout::Stacked => {
                // All children get full size, only focused is visible.
                let mut inner_rect = rect;
                if gap > 0.0 {
                    inner_rect.loc.x += gap;
                    inner_rect.loc.y += gap;
                    inner_rect.size.w = (inner_rect.size.w - gap * 2.0).max(0.0);
                    inner_rect.size.h = (inner_rect.size.h - gap * 2.0).max(0.0);
                }

                let bar_row_height = self.tab_bar_row_height();
                let mut tab_offset = 0.0;
                if bar_row_height > 0.0 && child_count > 0 {
                    let bar_height = match layout {
                        Layout::Tabbed => bar_row_height,
                        Layout::Stacked => bar_row_height * child_count as f64,
                        _ => 0.0,
                    };
                    let total_bar_height = (bar_height + self.tab_bar_spacing())
                        .min(inner_rect.size.h)
                        .max(0.0);
                    tab_offset = total_bar_height;
                }

                let focused_idx = focused_idx.unwrap_or(0).min(child_count.saturating_sub(1));

                for idx in 0..child_count {
                    let Some(child_key) = self.get_container_child_at(node_key, idx) else {
                        continue;
                    };
                    path.push(idx);
                    let child_visible = visible && idx == focused_idx;
                    let is_leaf = matches!(self.get_node(child_key), Some(NodeData::Leaf(_)));
                    let (child_rect, child_offset) = if is_leaf {
                        (inner_rect, tab_offset)
                    } else {
                        let mut content_rect = inner_rect;
                        if tab_offset > 0.0 {
                            content_rect.loc.y += tab_offset;
                            content_rect.size.h = (content_rect.size.h - tab_offset).max(0.0);
                        }
                        (content_rect, 0.0)
                    };
                    self.layout_node(
                        child_key,
                        child_rect,
                        path,
                        child_visible,
                        animate,
                        animate_resize,
                        child_offset,
                        false,
                    );
                    path.pop();
                }
            }
        }
    }

    fn tab_bar_row_height(&self) -> f64 {
        if self.options.layout.tab_bar.off {
            return 0.0;
        }
        tab_bar_row_height(&self.options.layout.tab_bar, self.scale)
    }

    fn split_title_bar_height(&self) -> f64 {
        if !self.options.layout.tab_bar.show_in_split {
            return 0.0;
        }
        self.tab_bar_row_height()
    }

    /// Get a child key at a specific index from a container node (avoids cloning children vec)
    fn get_container_child_at(&self, container_key: NodeKey, idx: usize) -> Option<NodeKey> {
        match self.get_node(container_key) {
            Some(NodeData::Container(container)) => container.child_key(idx),
            _ => None,
        }
    }

    /// Get normalized child percentages from a container (avoids cloning by computing inline)
    fn get_normalized_child_percents(&self, container_key: NodeKey, child_count: usize, percents_sum: f64) -> Vec<f64> {
        let Some(NodeData::Container(container)) = self.get_node(container_key) else {
            return vec![1.0 / child_count.max(1) as f64; child_count];
        };
        
        let percents = container.child_percents_slice();
        if percents_sum > f64::EPSILON {
            percents.iter().map(|p| p / percents_sum).collect()
        } else {
            vec![1.0 / child_count.max(1) as f64; child_count]
        }
    }

    fn split_child_titlebar(
        &self,
        child_key: NodeKey,
        split_bar_height: f64,
    ) -> (f64, bool) {
        if split_bar_height <= 0.0 {
            return (0.0, false);
        }

        let is_leaf = matches!(self.get_node(child_key), Some(NodeData::Leaf(_)));
        if is_leaf {
            (split_bar_height, true)
        } else {
            (0.0, false)
        }
    }

    fn tab_bar_spacing(&self) -> f64 {
        0.0
    }

    fn tab_bar_rect(
        &self,
        layout: Layout,
        rect: Rectangle<f64, Logical>,
        tab_count: usize,
    ) -> Option<(Rectangle<f64, Logical>, f64)> {
        if tab_count == 0 {
            return None;
        }

        let row_height = self.tab_bar_row_height();
        if row_height <= 0.0 {
            return None;
        }

        let gap = self.options.layout.gaps;
        let mut inner_rect = rect;
        if gap > 0.0 {
            inner_rect.loc.x += gap;
            inner_rect.loc.y += gap;
            inner_rect.size.w = (inner_rect.size.w - gap * 2.0).max(0.0);
            inner_rect.size.h = (inner_rect.size.h - gap * 2.0).max(0.0);
        }

        let spacing = self.tab_bar_spacing();
        let base_height = match layout {
            Layout::Tabbed => row_height,
            Layout::Stacked => row_height * tab_count as f64,
            _ => 0.0,
        };
        let bar_height = (base_height + spacing).min(inner_rect.size.h).max(0.0);
        if bar_height <= 0.0 {
            return None;
        }

        let bar_rect = Rectangle::new(
            inner_rect.loc,
            Size::from((inner_rect.size.w, bar_height)),
        );

        let actual_row_height = if layout == Layout::Stacked {
            row_height
        } else {
            row_height
        };

        Some((bar_rect, actual_row_height))
    }

    /// Get all windows in the tree (depth-first traversal)
    pub fn all_windows(&self) -> Vec<&W> {
        let mut windows = Vec::new();
        if let Some(root_key) = self.root {
            self.collect_windows_from_node(root_key, &mut windows);
        }
        windows
    }

    /// Helper: collect all windows from a node
    fn collect_windows_from_node<'a>(&'a self, node_key: NodeKey, windows: &mut Vec<&'a W>) {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => windows.push(tile.window()),
            Some(NodeData::Container(container)) => {
                for &child_key in &container.children {
                    self.collect_windows_from_node(child_key, windows);
                }
            }
            None => {}
        }
    }

    /// Get all tiles in the tree (depth-first traversal)
    pub fn all_tiles(&self) -> Vec<&Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(root_key) = self.root {
            self.collect_tiles_from_node(root_key, &mut tiles);
        }
        tiles
    }

    /// Helper: collect all tiles from a node
    fn collect_tiles_from_node<'a>(&'a self, node_key: NodeKey, tiles: &mut Vec<&'a Tile<W>>) {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => tiles.push(tile),
            Some(NodeData::Container(container)) => {
                for &child_key in &container.children {
                    self.collect_tiles_from_node(child_key, tiles);
                }
            }
            None => {}
        }
    }

    pub fn tab_bar_layouts(&self) -> Vec<TabBarInfo> {
        let mut out = Vec::new();
        let Some(root_key) = self.root else {
            return out;
        };

        let mut path = Vec::new();
        self.collect_tab_bar_layouts(root_key, &mut path, &mut out, true);
        out
    }

    fn collect_tab_bar_layouts(
        &self,
        node_key: NodeKey,
        path: &mut Vec<usize>,
        out: &mut Vec<TabBarInfo>,
        visible: bool,
    ) {
        let Some(NodeData::Container(container)) = self.get_node(node_key) else {
            return;
        };

        if visible && matches!(container.layout, Layout::Tabbed | Layout::Stacked) {
            if let Some((rect, row_height)) =
                self.tab_bar_rect(container.layout, container.geometry, container.children.len())
            {
                let focused_idx = container.focused_child_index().unwrap_or(0);
                let tabs = container
                    .children
                    .iter()
                    .enumerate()
                    .map(|(idx, &child_key)| {
                        let (title, block_out_from) =
                            self.focused_title_and_block_out(child_key);
                        TabBarTab {
                            title,
                            is_focused: idx == focused_idx,
                            is_urgent: self.subtree_has_urgent(child_key),
                            block_out_from,
                        }
                    })
                    .collect();

                out.push(TabBarInfo {
                    path: path.clone(),
                    layout: container.layout,
                    rect,
                    row_height,
                    tabs,
                });
            }
        }

        let focused_idx = container.focused_child_index().unwrap_or(0);
        for (idx, &child_key) in container.children.iter().enumerate() {
            path.push(idx);
            let child_visible = match container.layout {
                Layout::Tabbed | Layout::Stacked => idx == focused_idx,
                _ => true,
            };
            self.collect_tab_bar_layouts(child_key, path, out, visible && child_visible);
            path.pop();
        }
    }

    pub fn window_for_tab(&self, container_path: &[usize], tab_idx: usize) -> Option<&W> {
        let key = if container_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(container_path)?
        };
        if let Some(container) = self.get_container(key) {
            let child_key = container.child_key(tab_idx)?;
            return self.focused_window_in_subtree(child_key);
        }

        if tab_idx == 0 {
            if let Some(NodeData::Leaf(tile)) = self.get_node(key) {
                return Some(tile.window());
            }
        }

        None
    }

    fn focused_title_and_block_out(&self, node_key: NodeKey) -> (String, Option<BlockOutFrom>) {
        if let Some(window) = self.focused_window_in_subtree(node_key) {
            let title = window
                .title()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| String::from("untitled"));
            return (title, window.rules().block_out_from);
        }

        (String::from("untitled"), None)
    }

    fn focused_window_in_subtree(&self, node_key: NodeKey) -> Option<&W> {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => Some(tile.window()),
            Some(NodeData::Container(container)) => {
                let child_key = container.focused_child_key()?;
                self.focused_window_in_subtree(child_key)
            }
            None => None,
        }
    }

    fn subtree_has_urgent(&self, node_key: NodeKey) -> bool {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => tile.window().is_urgent(),
            Some(NodeData::Container(container)) => container
                .children
                .iter()
                .any(|&child_key| self.subtree_has_urgent(child_key)),
            None => false,
        }
    }

    /// Collect raw pointers to tiles (immutable) in depth-first order.
    pub fn tile_ptrs(&self) -> Vec<*const Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(root_key) = self.root {
            self.collect_tile_ptrs(root_key, &mut tiles);
        }
        tiles
    }

    fn collect_tile_ptrs(&self, node_key: NodeKey, out: &mut Vec<*const Tile<W>>) {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => out.push(tile as *const _),
            Some(NodeData::Container(container)) => {
                for &child_key in &container.children {
                    self.collect_tile_ptrs(child_key, out);
                }
            }
            None => {}
        }
    }

    /// Collect raw pointers to tiles (mutable) in depth-first order.
    pub fn tile_ptrs_mut(&mut self) -> Vec<*mut Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(root_key) = self.root {
            self.collect_tile_ptrs_mut(root_key, &mut tiles);
        }
        tiles
    }

    fn collect_tile_ptrs_mut(&mut self, node_key: NodeKey, out: &mut Vec<*mut Tile<W>>) {
        // Safety: We're creating raw pointers, caller must ensure proper usage
        match self.get_node_mut(node_key) {
            Some(NodeData::Leaf(tile)) => out.push(tile as *mut _),
            Some(NodeData::Container(container)) => {
                let children = container.children.clone();
                for child_key in children {
                    self.collect_tile_ptrs_mut(child_key, out);
                }
            }
            None => {}
        }
    }

    /// Helper: get tile at a given path (immutable).
    pub fn tile_at_path(&self, path: &[usize]) -> Option<&Tile<W>> {
        let key = self.get_node_key_at_path(path)?;
        self.get_tile(key)
    }

    /// Helper: get tile at a given path (mutable).
    pub fn tile_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Tile<W>> {
        let key = self.get_node_key_at_path(path)?;
        self.get_tile_mut(key)
    }

    // ========================================================================
    // Navigation methods
    // ========================================================================

    /// Move focus in a direction
    pub fn focus_in_direction(&mut self, direction: Direction) -> bool {
        self.clear_focus_history();
        if self.root.is_none() {
            return false;
        }

        if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        }

        let focus_path = self.focus_path();

        // Navigate up the focus path to find appropriate container
        for depth in (0..focus_path.len()).rev() {
            let parent_path = &focus_path[..depth];
            let current_idx = if depth < focus_path.len() {
                focus_path[depth]
            } else {
                continue;
            };

            let parent_key = if parent_path.is_empty() {
                self.root
            } else {
                self.get_node_key_at_path(parent_path)
            };

            if let Some(parent_key) = parent_key {
                if let Some(container) = self.get_container(parent_key) {
                    // Check if this container's layout matches the direction
                    let layout_matches = match (container.layout, direction) {
                        (Layout::SplitH | Layout::Tabbed, Direction::Left | Direction::Right) => {
                            true
                        }
                        (Layout::SplitV | Layout::Stacked, Direction::Up | Direction::Down) => {
                            true
                        }
                        _ => false,
                    };

                    if !layout_matches {
                        continue;
                    }

                    // Try to move in the direction
                    let child_count = container.children.len();
                    let new_idx = match direction {
                        Direction::Left | Direction::Up => {
                            if current_idx > 0 {
                                Some(current_idx - 1)
                            } else {
                                None
                            }
                        }
                        Direction::Right | Direction::Down => {
                            if current_idx + 1 < child_count {
                                Some(current_idx + 1)
                            } else {
                                None
                            }
                        }
                    };

                    if let Some(new_idx) = new_idx {
                        let Some(target_key) = container.child_key(new_idx) else {
                            continue;
                        };
                        self.focus_node_key(target_key);
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Focus window by its ID if present.
    pub fn focus_window_by_id(&mut self, window_id: &W::Id) -> bool {
        self.clear_focus_history();
        let Some(path) = self.find_window(window_id) else {
            return false;
        };
        let Some(key) = self.get_node_key_at_path(&path) else {
            return false;
        };
        self.focus_node_key(key);
        true
    }

    pub fn focus_parent(&mut self) -> bool {
        self.clear_focus_history();
        let Some(focused_key) = self.focused_key else {
            return false;
        };
        let Some(parent_key) = self.parent_of(focused_key) else {
            return false;
        };
        self.focus_node_key(parent_key);
        true
    }

    pub fn focus_child(&mut self) -> bool {
        self.clear_focus_history();
        let Some(focused_key) = self.focused_key else {
            return false;
        };
        let Some(parent_key) = self.parent_of(focused_key) else {
            return false;
        };
        let Some(parent) = self.get_container(parent_key) else {
            return false;
        };
        let Some(child_key) = parent.focused_child_key() else {
            return false;
        };
        self.focus_node_key(child_key);
        true
    }

    fn prune_selected_key(&mut self) {
        if let Some(key) = self.selected_key {
            if self.get_node(key).is_none() {
                self.selected_key = None;
            }
        }
    }

    fn reconcile_focus_after_change(&mut self, focused_removed: bool) {
        if self.root.is_none() {
            self.focused_key = None;
        } else if focused_removed {
            self.focused_key = None;
            self.focus_first_leaf();
        } else if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        } else {
            self.focus_first_leaf();
        }
    }

    // ========================================================================
    // Management methods
    // ========================================================================

    /// Remove a window by ID, returns the removed tile
    pub fn remove_window(&mut self, window_id: &W::Id) -> Option<Tile<W>> {
        let path = self.find_window(window_id)?;
        let node_key = self.get_node_key_at_path(&path)?;
        let cleanup_key = self.parent_of(node_key);
        let was_focused = self.focused_key == Some(node_key);

        // First, remove from parent's children list BEFORE removing from slotmap
        if !path.is_empty() {
            let parent_path = &path[..path.len() - 1];
            let child_idx = *path.last().unwrap();

            if let Some(parent_key) = self.get_node_key_at_path(parent_path) {
                if let Some(container) = self.get_container_mut(parent_key) {
                    container.remove_child(child_idx);
                }
            }
            self.set_parent(node_key, None);
        } else {
            // Was root
            self.root = None;
            self.set_parent(node_key, None);
        }

        // Now remove from slotmap (only the leaf, not recursive)
        let node_data = self.nodes.remove(node_key)?;
        self.parents.remove(node_key);
        let tile = match node_data {
            NodeData::Leaf(tile) => tile,
            NodeData::Container(_) => return None, // Should never happen
        };

        self.cleanup_containers(cleanup_key);
        self.prune_leaf_layouts();

        self.prune_selected_key();
        self.reconcile_focus_after_change(was_focused);

        self.layout();

        Some(tile)
    }

    pub(super) fn take_subtree_at_path(
        &mut self,
        path: &[usize],
    ) -> Option<(DetachedNode<W>, Option<InsertParentInfo>)> {
        let node_key = self.get_node_key_at_path(path)?;
        let insert_info = self.insert_parent_info_for_path(path);

        let focused_path = self.focus_path();
        let focused_in_subtree =
            focused_path.len() >= path.len() && focused_path[..path.len()] == *path;

        if let Some(selected_key) = self.selected_key {
            if let Some(selected_path) = self.find_node_path(selected_key) {
                if selected_path.len() >= path.len() && selected_path[..path.len()] == *path {
                    self.selected_key = None;
                }
            }
        }

        let cleanup_key = if path.is_empty() {
            self.root = None;
            self.set_parent(node_key, None);
            None
        } else {
            let parent_path = &path[..path.len() - 1];
            let parent_key = if parent_path.is_empty() {
                self.root?
            } else {
                self.get_node_key_at_path(parent_path)?
            };

            if let Some(container) = self.get_container_mut(parent_key) {
                let idx = *path.last().unwrap();
                container.remove_child(idx);
            }
            self.set_parent(node_key, None);
            Some(parent_key)
        };

        let subtree = self.extract_subtree(node_key);
        self.cleanup_containers(cleanup_key);
        self.prune_leaf_layouts();

        self.prune_selected_key();
        self.reconcile_focus_after_change(focused_in_subtree);

        self.layout();

        Some((subtree, insert_info))
    }

    /// Move window in a direction (swaps with sibling)
    pub fn move_in_direction(&mut self, direction: Direction) -> bool {
        self.clear_focus_history();
        if self.root.is_none() {
            return false;
        }

        if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        }

        let mut move_path = self.focus_path();
        if move_path.is_empty() {
            return false;
        }

        loop {
            if move_path.is_empty() {
                break;
            }

            let parent_path = &move_path[..move_path.len() - 1];
            if parent_path.is_empty() {
                break;
            }

            let parent_key = match self.get_node_key_at_path(parent_path) {
                Some(key) => key,
                None => break,
            };
            let parent_container = match self.get_container(parent_key) {
                Some(container) => container,
                None => break,
            };

            if parent_container.child_count() == 1 && !parent_container.preserve_on_single() {
                move_path = parent_path.to_vec();
                continue;
            }
            break;
        }

        if move_path.is_empty() {
            return false;
        }

        let node_key = match self.get_node_key_at_path(&move_path) {
            Some(key) => key,
            None => return false,
        };
        let node_parent_path = &move_path[..move_path.len() - 1];
        let node_idx = *move_path.last().unwrap();

        let parent_key = if node_parent_path.is_empty() {
            self.root
        } else {
            self.get_node_key_at_path(node_parent_path)
        };

        let Some(parent_key) = parent_key else {
            return false;
        };

        let Some(parent_layout) = self.get_container(parent_key).map(|c| c.layout()) else {
            return false;
        };

        let layout_matches = match (parent_layout, direction) {
            (Layout::SplitH | Layout::Tabbed, Direction::Left | Direction::Right) => true,
            (Layout::SplitV | Layout::Stacked, Direction::Up | Direction::Down) => true,
            _ => false,
        };

        if layout_matches {
            let child_count = match self.get_container(parent_key) {
                Some(container) => container.child_count(),
                None => 0,
            };
            if child_count == 0 {
                return false;
            }

            let target_idx = match direction {
                Direction::Left | Direction::Up => {
                    if node_idx > 0 {
                        Some(node_idx - 1)
                    } else {
                        None
                    }
                }
                Direction::Right | Direction::Down => {
                    if node_idx + 1 < child_count {
                        Some(node_idx + 1)
                    } else {
                        None
                    }
                }
            };

            let Some(target_idx) = target_idx else {
                // At edge: escape to grandparent if possible.
                if node_parent_path.is_empty() {
                    return false;
                }
                let grandparent_path = &node_parent_path[..node_parent_path.len() - 1];
                let parent_idx = *node_parent_path.last().unwrap();
                return self.move_node_to_grandparent(
                    node_key,
                    node_parent_path,
                    node_idx,
                    grandparent_path,
                    parent_idx,
                    direction,
                );
            };

            let target_key = match self.get_container(parent_key).and_then(|c| c.child_key(target_idx)) {
                Some(key) => key,
                None => return false,
            };

            if matches!(parent_layout, Layout::SplitH | Layout::SplitV) {
                if let Some(target_container) = self.get_container(target_key) {
                    let should_enter = target_container.layout() != parent_layout
                        || target_container.preserve_on_single();
                    if should_enter {
                        return self.move_node_into_container(
                            node_key,
                            node_parent_path,
                            node_idx,
                            target_key,
                            direction,
                            target_container.focused_child_index().unwrap_or(0),
                        );
                    }
                }
            }

            if let Some(container) = self.get_container_mut(parent_key) {
                container.children.swap(node_idx, target_idx);
                container.child_percents.swap(node_idx, target_idx);
            }

            self.focus_node_key(node_key);
            return true;
        }

        if node_parent_path.is_empty() {
            return false;
        }

        let grandparent_path = &node_parent_path[..node_parent_path.len() - 1];
        let parent_idx = *node_parent_path.last().unwrap();

        self.move_node_to_grandparent(
            node_key,
            node_parent_path,
            node_idx,
            grandparent_path,
            parent_idx,
            direction,
        )
    }

    fn ensure_root_container_with_layout(&mut self, layout: Layout) -> bool {
        if let Some(root_key) = self.root {
            if matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
                let old_root_key = self.root.take().unwrap();
                let mut container = ContainerData::new(layout);
                container.mark_preserve_on_single();
                container.add_child(old_root_key);
                let container_key = self.insert_node(NodeData::Container(container));
                self.set_parent(old_root_key, Some(container_key));
                self.set_parent(container_key, None);
                self.root = Some(container_key);
                self.focus_node_key(old_root_key);
                return true;
            }
        }
        false
    }

    fn node_key_for_path_or_root(&self, path: &[usize]) -> Option<NodeKey> {
        if path.is_empty() {
            self.root
        } else {
            self.get_node_key_at_path(path)
        }
    }

    /// Split the focused container in a direction
    pub fn split_focused(&mut self, layout: Layout) -> bool {
        self.clear_focus_history();
        if self.root.is_none() {
            self.pending_layout = Some(layout);
            return true;
        }

        let focus_path = self.focus_path();

        // Special case: if root is a leaf, wrap it in a container
        if focus_path.is_empty() {
            if self.ensure_root_container_with_layout(layout) {
                return true;
            }
        }

        if focus_path.is_empty() {
            return false;
        }

        let parent_path = &focus_path[..focus_path.len() - 1];
        let child_idx = *focus_path.last().unwrap();

        let Some(parent_key) = self.node_key_for_path_or_root(parent_path) else {
            return false;
        };

        let parent_layout = match self.get_container(parent_key) {
            Some(container) => container.layout(),
            None => return false,
        };

        // Get the focused child key
        let focused_child_key = if let Some(container) = self.get_container(parent_key) {
            match container.child_key(child_idx) {
                Some(key) => key,
                None => return false,
            }
        } else {
            return false;
        };

        // Only split if it's a leaf
        if matches!(self.get_node(focused_child_key), Some(NodeData::Leaf(_))) {
            if parent_layout == layout {
                return true;
            }

            if let Some(container) = self.get_container_mut(parent_key) {
                if container.child_count() == 1 && matches!(parent_layout, Layout::SplitH | Layout::SplitV)
                {
                    container.set_layout_explicit(layout);
                    return true;
                }
            }

            // Remove child from parent
            if let Some(container) = self.get_container_mut(parent_key) {
                container.remove_child(child_idx);
            }
            self.set_parent(focused_child_key, None);

            // Create new container with the leaf
            let mut new_container = ContainerData::new(layout);
            new_container.mark_preserve_on_single();
            new_container.add_child(focused_child_key);
            let new_container_key = self.insert_node(NodeData::Container(new_container));
            self.set_parent(focused_child_key, Some(new_container_key));

            // Insert new container back at same position
            if let Some(container) = self.get_container_mut(parent_key) {
                container.insert_child(child_idx, new_container_key);
            }
            self.set_parent(new_container_key, Some(parent_key));

            self.focus_node_key(focused_child_key);
            return true;
        }

        false
    }

    /// Change layout of focused container
    pub fn set_focused_layout(&mut self, layout: Layout) -> bool {
        if self.root.is_none() {
            self.pending_layout = Some(layout);
            return true;
        }

        let focus_path = self.focus_path();

        if focus_path.is_empty() {
            if self.ensure_root_container_with_layout(layout) {
                return true;
            }
        }

        // If focus is on a leaf, use parent container
        if let Some(node_key) = self.get_node_key_at_path(&focus_path) {
            if matches!(self.get_node(node_key), Some(NodeData::Leaf(_))) {
                // Get parent container
                if focus_path.is_empty() {
                    return false;
                }

                let parent_path = &focus_path[..focus_path.len() - 1];
                let Some(parent_key) = self.node_key_for_path_or_root(parent_path) else {
                    return false;
                };

                if let Some(container) = self.get_container_mut(parent_key) {
                    container.set_layout_explicit(layout);
                    return true;
                }
            } else {
                // It's already a container, change its layout
                if let Some(container) = self.get_container_mut(node_key) {
                    container.set_layout_explicit(layout);
                    return true;
                }
            }
        }

        false
    }

    /// Toggle between horizontal and vertical split for the focused container.
    pub fn toggle_split_layout(&mut self) -> bool {
        if self.root.is_none() {
            let next = match self.pending_layout.unwrap_or(Layout::SplitH) {
                Layout::SplitH => Layout::SplitV,
                _ => Layout::SplitH,
            };
            self.pending_layout = Some(next);
            return true;
        }

        let focus_path = self.focus_path();
        let target_key = if focus_path.is_empty() {
            self.node_key_for_path_or_root(&[])
        } else {
            self.node_key_for_path_or_root(&focus_path[..focus_path.len() - 1])
        };
        let Some(target_key) = target_key else {
            return false;
        };

        let current = match self.get_container(target_key) {
            Some(container) => container.layout(),
            None => return false,
        };

        let next = match current {
            Layout::SplitH => Layout::SplitV,
            Layout::SplitV => Layout::SplitH,
            Layout::Tabbed | Layout::Stacked => Layout::SplitH,
        };

        if matches!(current, Layout::Tabbed | Layout::Stacked) {
            if let Some(container) = self.get_container_mut(target_key) {
                container.set_layout_explicit(next);
                return true;
            }
            return false;
        }

        self.set_focused_layout(next)
    }

    /// Layout of the container that currently owns the focused leaf (if any).
    pub fn focused_layout(&self) -> Option<Layout> {
        let focus_path = self.focus_path();
        if focus_path.is_empty() {
            let root_key = self.root?;
            self.get_container(root_key).map(|c| c.layout())
        } else {
            let parent_path = &focus_path[..focus_path.len() - 1];
            let parent_key = if parent_path.is_empty() {
                self.root?
            } else {
                self.get_node_key_at_path(parent_path)?
            };
            self.get_container(parent_key).map(|c| c.layout())
        }
    }

    /// Whether the focused container should accept new splits.
    pub fn focused_container_allows_splits(&self) -> bool {
        let focus_path = self.focus_path();
        let container_key = if focus_path.is_empty() {
            let root_key = match self.root {
                Some(key) => key,
                None => return false,
            };
            if matches!(self.get_node(root_key), Some(NodeData::Container(_))) {
                root_key
            } else {
                return false;
            }
        } else {
            let parent_path = &focus_path[..focus_path.len() - 1];
            if parent_path.is_empty() {
                match self.root {
                    Some(key) => key,
                    None => return false,
                }
            } else {
                match self.get_node_key_at_path(parent_path) {
                    Some(key) => key,
                    None => return false,
                }
            }
        };

        let Some(container) = self.get_container(container_key) else {
            return false;
        };
        container.child_count() > 1 || container.preserve_on_single()
    }

    // ========================================================================
    // Query methods
    // ========================================================================

    pub fn container_info(
        &self,
        path: &[usize],
    ) -> Option<(Layout, Rectangle<f64, Logical>, usize)> {
        let container_key = if path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(path)?
        };

        let container = self.get_container(container_key)?;
        Some((
            container.layout(),
            container.geometry(),
            container.child_count(),
        ))
    }

    pub fn child_rect_at(
        &self,
        parent_path: &[usize],
        child_idx: usize,
    ) -> Option<Rectangle<f64, Logical>> {
        let container_key = if parent_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(parent_path)?
        };

        let container = self.get_container(container_key)?;
        if child_idx >= container.child_count() {
            return None;
        }

        let child_key = container.child_key(child_idx)?;
        let child_is_leaf = matches!(self.get_node(child_key), Some(NodeData::Leaf(_)));
        let child_count = container.child_count();
        let percents_sum: f64 = container.child_percents_slice().iter().copied().sum();
        let percents = self.get_normalized_child_percents(container_key, child_count, percents_sum);
        let (rect, _) = self.preview_child_rect(
            container.layout(),
            container.geometry(),
            child_count,
            &percents,
            child_idx,
            child_is_leaf,
        );

        Some(rect)
    }

    pub fn find_parent_with_layout(
        &self,
        mut path: Vec<usize>,
        layout: Layout,
    ) -> Option<(Vec<usize>, usize)> {
        while !path.is_empty() {
            let child_idx = *path.last().unwrap();
            let parent_path_vec = path[..path.len() - 1].to_vec();

            let container_key = if parent_path_vec.is_empty() {
                self.root?
            } else {
                self.get_node_key_at_path(&parent_path_vec)?
            };

            if let Some(container) = self.get_container(container_key) {
                if container.layout() == layout {
                    return Some((parent_path_vec, child_idx));
                }
            }

            path.pop();
        }

        None
    }

    pub fn child_percent_at(&self, parent_path: &[usize], child_idx: usize) -> Option<f64> {
        let container_key = if parent_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(parent_path)?
        };

        let container = self.get_container(container_key)?;

        if child_idx >= container.child_count() {
            return None;
        }
        Some(container.child_percent(child_idx))
    }

    pub fn set_child_percent_at(
        &mut self,
        parent_path: &[usize],
        child_idx: usize,
        layout: Layout,
        percent: f64,
    ) -> bool {
        let container_key = if parent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(parent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        if let Some(container) = self.get_container_mut(container_key) {
            if container.layout() != layout || child_idx >= container.child_count() {
                return false;
            }
            container.set_child_percent(child_idx, percent);
            true
        } else {
            false
        }
    }

    pub fn set_child_percent_pair_at(
        &mut self,
        parent_path: &[usize],
        child_idx: usize,
        neighbor_idx: usize,
        layout: Layout,
        percent: f64,
    ) -> bool {
        let container_key = if parent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(parent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        if let Some(container) = self.get_container_mut(container_key) {
            if container.layout() != layout
                || child_idx >= container.child_count()
                || neighbor_idx >= container.child_count()
            {
                return false;
            }
            container.set_child_percent_pair(child_idx, neighbor_idx, percent)
        } else {
            false
        }
    }

    pub fn container_at_path_mut(&mut self, path: &[usize]) -> Option<&mut ContainerData> {
        let key = if path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(path)?
        };
        self.get_container_mut(key)
    }

    // ========================================================================
    // Root-level methods
    // ========================================================================

    /// Number of root-level children (columns).
    pub fn root_children_len(&self) -> usize {
        let root_key = match self.root {
            Some(key) => key,
            None => return 0,
        };

        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => 1,
            Some(NodeData::Container(container)) => container.children.len(),
            None => 0,
        }
    }

    pub fn root_container(&self) -> Option<&ContainerData> {
        let root_key = self.root?;
        self.get_container(root_key)
    }

    pub fn root_container_mut(&mut self) -> Option<&mut ContainerData> {
        let root_key = self.root?;
        self.get_container_mut(root_key)
    }

    /// Current percent of a root child relative to the root container, if any.
    pub fn root_child_percent(&self, idx: usize) -> Option<f64> {
        let root_key = self.root?;
        match self.get_node(root_key) {
            Some(NodeData::Container(container)) => {
                if idx >= container.children.len() {
                    None
                } else {
                    Some(container.child_percent(idx))
                }
            }
            Some(NodeData::Leaf(_)) => {
                if idx == 0 {
                    Some(1.0)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    /// Set the percent of a root child.
    pub fn set_root_child_percent(&mut self, idx: usize, percent: f64) -> bool {
        let root_key = match self.root {
            Some(key) => key,
            None => return false,
        };

        if let Some(container) = self.get_container_mut(root_key) {
            if idx >= container.children.len() {
                return false;
            }
            container.set_child_percent(idx, percent);
            true
        } else {
            false
        }
    }

    /// Index of currently focused root child, if any.
    pub fn focused_root_index(&self) -> Option<usize> {
        let root_key = self.root?;
        if let Some(key) = self.focused_key {
            if key == root_key {
                return Some(0);
            }
            let mut child = key;
            let mut parent = self.parent_of(child)?;
            while parent != root_key {
                child = parent;
                parent = self.parent_of(child)?;
            }
            return self.child_index(root_key, child);
        }

        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => Some(0),
            Some(NodeData::Container(container)) => {
                let focus_path = self.focus_path();
                if focus_path.is_empty() {
                    container.focused_child_index()
                } else {
                    Some(focus_path[0])
                }
            }
            None => None,
        }
    }

    /// Focus root child at index, descending to the first leaf.
    pub fn focus_root_child(&mut self, idx: usize) -> bool {
        self.clear_focus_history();
        let root_key = match self.root {
            Some(key) => key,
            None => return false,
        };

        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => {
                if idx == 0 {
                    self.focus_node_key(root_key);
                    true
                } else {
                    false
                }
            }
            Some(NodeData::Container(container)) => {
                if idx >= container.children.len() {
                    return false;
                }
                let child_key = container.child_key(idx);
                if let Some(child_key) = child_key {
                    self.focus_node_key(child_key);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Move a root child from one index to another
    pub fn move_root_child(&mut self, from: usize, to: usize) -> bool {
        self.clear_focus_history();
        let root_key = match self.root {
            Some(key) => key,
            None => return false,
        };

        let container = match self.get_container_mut(root_key) {
            Some(c) => c,
            None => return false,
        };

        if from >= container.children.len() || to >= container.children.len() {
            return false;
        }

        let node_key = container.children.remove(from);
        let percent = container.child_percents.remove(from);
        container.children.insert(to, node_key);
        container.child_percents.insert(to, percent);
        container.normalize_child_percents();

        if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        } else {
            self.focus_first_leaf();
        }
        true
    }

    /// Extract a subtree rooted at the given key into a detached representation.
    fn extract_subtree(&mut self, key: NodeKey) -> DetachedNode<W> {
        let node_data = self
            .nodes
            .remove(key)
            .expect("node key must exist when extracting subtree");
        self.parents.remove(key);

        match node_data {
            NodeData::Leaf(tile) => DetachedNode::Leaf(tile),
            NodeData::Container(container) => {
                let child_keys = container.children.clone();
                let mut children = Vec::new();
                for child_key in container.children {
                    children.push(self.extract_subtree(child_key));
                }
                let mut index_by_key = HashMap::new();
                for (idx, key) in child_keys.iter().enumerate() {
                    index_by_key.insert(*key, idx);
                }
                let focus_stack = container
                    .focus_stack
                    .iter()
                    .filter_map(|key| index_by_key.get(key).copied())
                    .collect();
                DetachedNode::Container(DetachedContainer::from_parts(
                    container.layout,
                    children,
                    container.child_percents,
                    focus_stack,
                    container.preserve_on_single,
                ))
            }
        }
    }

    /// Insert a detached subtree into this tree, returning the new root key.
    fn insert_subtree(&mut self, subtree: DetachedNode<W>) -> NodeKey {
        match subtree {
            DetachedNode::Leaf(tile) => self.insert_node(NodeData::Leaf(tile)),
            DetachedNode::Container(container) => {
                let container_key =
                    self.insert_node(NodeData::Container(ContainerData::new(container.layout)));

                let mut child_keys = Vec::new();
                for child in container.children {
                    let child_key = self.insert_subtree(child);
                    self.set_parent(child_key, Some(container_key));
                    child_keys.push(child_key);
                }

                if let Some(node) = self.get_container_mut(container_key) {
                    node.children = child_keys;
                    node.child_percents = container.child_percents;
                    node.focus_stack = container
                        .focus_stack
                        .iter()
                        .filter_map(|idx| node.children.get(*idx).copied())
                        .collect();
                    node.preserve_on_single = container.preserve_on_single;
                    if node.child_percents.len() != node.children.len() {
                        node.recalculate_percentages();
                    } else {
                        node.normalize_child_percents();
                    }
                    node.ensure_focus_stack();
                }

                container_key
            }
        }
    }

    /// Remove and return the root-level child at the given index as a detached subtree.
    pub fn take_root_child_subtree(&mut self, idx: usize) -> Option<DetachedNode<W>> {
        let root_key = self.root?;

        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => {
                if idx == 0 {
                    self.focused_key = None;
                    let subtree = self.extract_subtree(root_key);
                    self.root = None;
                    self.prune_leaf_layouts();
                    Some(subtree)
                } else {
                    None
                }
            }
            Some(NodeData::Container(_)) => {
                let child_key = {
                    let container = self.get_container(root_key)?;
                    if idx >= container.children.len() {
                        return None;
                    }
                    container.child_key(idx)?
                };

                if let Some(container) = self.get_container_mut(root_key) {
                    container.remove_child(idx);
                }
                self.set_parent(child_key, None);

                let remaining = self.get_container(root_key)?.children.len();

                self.cleanup_containers(Some(root_key));
                self.prune_leaf_layouts();

                match self.get_node(root_key) {
                    Some(NodeData::Leaf(_)) | None => {
                        self.focus_first_leaf();
                    }
                    Some(NodeData::Container(root_container)) => {
                        if remaining > 0 {
                            let new_idx = idx.min(root_container.children.len().saturating_sub(1));
                            let child_key = root_container.child_key(new_idx);
                            if let Some(child_key) = child_key {
                                self.focus_node_key(child_key);
                            } else {
                                self.focus_first_leaf();
                            }
                        } else {
                            self.focus_first_leaf();
                        }
                    }
                }

                let subtree = self.extract_subtree(child_key);
                Some(subtree)
            }
            None => None,
        }
    }

    /// Remove and return the root-level child at the given index as a vector of tiles.
    pub fn take_root_child_tiles(&mut self, idx: usize) -> Option<Vec<Tile<W>>> {
        self.take_root_child_subtree(idx)
            .map(|subtree| subtree.into_tiles())
    }

    /// Insert a detached subtree at root level.
    pub fn insert_subtree_at_root(&mut self, index: usize, subtree: DetachedNode<W>, focus: bool) {
        let node_key = self.insert_subtree(subtree);
        self.insert_key_at_root(index, node_key, focus);
    }

    /// Focus nth (1-based) leaf within the given root child.
    pub fn focus_leaf_in_root_child(&mut self, child_idx: usize, leaf_idx: usize) -> bool {
        self.clear_focus_history();
        if leaf_idx == 0 {
            return false;
        }
        let mut paths = self.leaf_paths_under(&[child_idx]);
        if paths.is_empty() {
            return false;
        }
        if leaf_idx > paths.len() {
            return false;
        }
        let path = paths.remove(leaf_idx - 1);
        if let Some(key) = self.get_node_key_at_path(&path) {
            self.focus_node_key(key);
            true
        } else {
            false
        }
    }

    /// Focus the first leaf in the currently focused root child.
    pub fn focus_top_in_current_column(&mut self) -> bool {
        let idx = match self.focused_root_index() {
            Some(idx) => idx,
            None => return false,
        };
        self.focus_leaf_in_root_child(idx, 1)
    }

    /// Focus the last leaf in the currently focused root child.
    pub fn focus_bottom_in_current_column(&mut self) -> bool {
        let idx = match self.focused_root_index() {
            Some(idx) => idx,
            None => return false,
        };
        let paths = self.leaf_paths_under(&[idx]);
        if let Some(path) = paths.last() {
            if let Some(key) = self.get_node_key_at_path(path) {
                self.focus_node_key(key);
                return true;
            }
            false
        } else {
            false
        }
    }

    /// Collect leaf paths under a given prefix path.
    pub fn leaf_paths_under(&self, prefix: &[usize]) -> Vec<Vec<usize>> {
        let mut results = Vec::new();
        let mut path = prefix.to_vec();
        if let Some(node_key) = self.get_node_key_at_path(prefix) {
            self.collect_leaf_paths_from_node(node_key, &mut path, &mut results);
        }
        results
    }

    fn collect_leaf_paths_from_node(
        &self,
        node_key: NodeKey,
        path: &mut Vec<usize>,
        results: &mut Vec<Vec<usize>>,
    ) {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(_)) => results.push(path.clone()),
            Some(NodeData::Container(container)) => {
                for (idx, &child_key) in container.children.iter().enumerate() {
                    path.push(idx);
                    self.collect_leaf_paths_from_node(child_key, path, results);
                    path.pop();
                }
            }
            None => {}
        }
    }

    // ========================================================================
    // Insertion methods
    // ========================================================================

    /// Insert multiple tiles as a column (vertical container) at root level
    pub fn insert_tiles_at_root(&mut self, index: usize, tiles: Vec<Tile<W>>, focus: bool) {
        if tiles.is_empty() {
            return;
        }

        // If only one tile, insert it directly
        if tiles.len() == 1 {
            let tile = tiles.into_iter().next().unwrap();
            self.insert_leaf_at(index, tile, focus);
            return;
        }

        // Create a vertical container with all tiles
        let mut container = ContainerData::new(Layout::SplitV);
        let mut tile_keys = Vec::new();
        for tile in tiles {
            let tile_key = self.insert_node(NodeData::Leaf(tile));
            container.add_child(tile_key);
            tile_keys.push(tile_key);
        }

        let container_key = self.insert_node(NodeData::Container(container));
        for tile_key in tile_keys {
            self.set_parent(tile_key, Some(container_key));
        }
        self.insert_key_at_root(index, container_key, focus);
    }

    pub fn append_leaf(&mut self, tile: Tile<W>, focus: bool) {
        self.insert_leaf_at(self.root_children_len(), tile, focus);
    }

    pub fn insert_leaf_at(&mut self, index: usize, tile: Tile<W>, focus: bool) {
        let tile_key = self.insert_node(NodeData::Leaf(tile));
        self.insert_key_at_root(index, tile_key, focus);
    }

    fn insert_key_at_root(&mut self, index: usize, node_key: NodeKey, focus: bool) {
        let insert_idx = {
            let container_key = self.ensure_root_container();
            let container = self.get_container(container_key).unwrap();
            let idx = index.min(container.children.len());

            if let Some(container) = self.get_container_mut(container_key) {
                container.insert_child(idx, node_key);

                idx
            } else {
                idx
            }
        };
        let container_key = self.ensure_root_container();
        if let Some(container) = self.get_container(container_key) {
            if container.child_key(insert_idx) == Some(node_key) {
                self.set_parent(node_key, Some(container_key));
            }
        }

        if focus {
            self.focus_node_key(node_key);
        } else {
            if let Some(key) = self.focused_key {
                self.sync_container_focus_from_key(key);
            } else {
                self.focus_first_leaf();
            }
        }
    }

    pub fn insert_leaf_after(&mut self, window_id: &W::Id, tile: Tile<W>, focus: bool) -> bool {
        let path = match self.find_window(window_id) {
            Some(path) => path,
            None => {
                self.append_leaf(tile, focus);
                return true;
            }
        };

        if path.is_empty() {
            self.append_leaf(tile, focus);
            return true;
        }

        let parent_path = &path[..path.len() - 1];
        let current_idx = *path.last().unwrap();

        let parent_key = if parent_path.is_empty() {
            self.root
        } else {
            self.get_node_key_at_path(parent_path)
        };

        if let Some(parent_key) = parent_key {
            let insert_idx = current_idx + 1;
            let tile_key = self.insert_node(NodeData::Leaf(tile));

            let mut inserted = false;
            if let Some(parent) = self.get_container_mut(parent_key) {
                parent.insert_child(insert_idx, tile_key);
                inserted = true;
            }
            if inserted {
                self.set_parent(tile_key, Some(parent_key));
                if focus {
                    self.focus_node_key(tile_key);
                } else if let Some(key) = self.focused_key {
                    self.sync_container_focus_from_key(key);
                } else {
                    self.focus_first_leaf();
                }
                return true;
            }
        }

        false
    }

    pub fn insert_leaf_in_column(
        &mut self,
        column_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        focus: bool,
    ) -> bool {
        let root_key = self.ensure_root_container();

        let root_container = match self.get_container(root_key) {
            Some(c) => c,
            None => return false,
        };

        if column_idx >= root_container.children.len() {
            return false;
        }

        let column_key = root_container.child_key(column_idx).unwrap();

        // Check if column is a leaf, if so convert to container
        if matches!(self.get_node(column_key), Some(NodeData::Leaf(_))) {
            // Get the existing data first
            let (existing_key, existing_percent, focus_pos) =
                if let Some(container) = self.get_container_mut(root_key) {
                    let existing_key = container.children.remove(column_idx);
                    let existing_percent = container.child_percents.remove(column_idx);
                    let focus_pos = container
                        .focus_stack
                        .iter()
                        .position(|key| *key == existing_key);
                    container.focus_stack.retain(|key| *key != existing_key);
                    (existing_key, existing_percent, focus_pos)
                } else {
                    return false;
                };

            // Create new column container
            let mut column_container = ContainerData::new(Layout::SplitV);
            column_container.add_child(existing_key);
            let column_container_key = self.insert_node(NodeData::Container(column_container));
            self.set_parent(existing_key, Some(column_container_key));

            // Insert back
            if let Some(container) = self.get_container_mut(root_key) {
                container.children.insert(column_idx, column_container_key);
                container
                    .child_percents
                    .insert(column_idx, existing_percent);
                if let Some(pos) = focus_pos {
                    container.focus_stack.insert(pos, column_container_key);
                } else if !container.focus_stack.contains(&column_container_key) {
                    container.focus_stack.push(column_container_key);
                }
                container.ensure_focus_stack();
                container.normalize_child_percents();
            }
            self.set_parent(column_container_key, Some(root_key));
        }

        // Now insert the new tile
        let column_key = match self.get_container(root_key) {
            Some(c) => match c.child_key(column_idx) {
                Some(key) => key,
                None => return false,
            },
            None => return false,
        };
        let column_container = match self.get_container(column_key) {
            Some(c) => c,
            None => return false,
        };

        let insert_at = tile_idx.unwrap_or(column_container.children.len());
        let insert_at = insert_at.min(column_container.children.len());

        let tile_key = self.insert_node(NodeData::Leaf(tile));

        if let Some(column_container) = self.get_container_mut(column_key) {
            column_container.insert_child(insert_at, tile_key);

            if focus {
                self.focus_node_key(tile_key);
            } else if let Some(key) = self.focused_key {
                self.sync_container_focus_from_key(key);
            } else {
                self.focus_first_leaf();
            }
        }
        self.set_parent(tile_key, Some(column_key));

        true
    }

    pub(super) fn insert_parent_info_for_window(
        &self,
        window_id: &W::Id,
    ) -> Option<InsertParentInfo> {
        let path = self.find_window(window_id)?;
        self.insert_parent_info_for_path(&path)
    }

    fn insert_parent_info_for_path(&self, path: &[usize]) -> Option<InsertParentInfo> {
        if path.is_empty() {
            return None;
        }

        let mut parent_path = path.to_vec();
        let insert_idx = parent_path.pop().unwrap();
        let parent_key = if parent_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(&parent_path)?
        };
        let parent = self.get_container(parent_key)?;
        Some(InsertParentInfo {
            parent_path,
            insert_idx,
            layout: parent.layout(),
            child_percents: parent.child_percents_slice().to_vec(),
        })
    }

    pub(super) fn replace_leaf_at_path(
        &mut self,
        path: &[usize],
        tile: Tile<W>,
    ) -> Option<Tile<W>> {
        let key = self.get_node_key_at_path(path)?;
        match self.get_node_mut(key)? {
            NodeData::Leaf(existing) => Some(std::mem::replace(existing, tile)),
            _ => None,
        }
    }

    pub(super) fn is_leaf_at_path(&self, path: &[usize]) -> bool {
        let Some(key) = self.get_node_key_at_path(path) else {
            return false;
        };
        matches!(self.get_node(key), Some(NodeData::Leaf(_)))
    }

    pub(super) fn insert_leaf_with_parent_info(
        &mut self,
        info: &InsertParentInfo,
        tile: Tile<W>,
        focus: bool,
    ) -> bool {
        let container_key = match self.ensure_container_at_path(&info.parent_path, info.layout) {
            Some(key) => key,
            None => {
                self.append_leaf(tile, focus);
                return true;
            }
        };

        let tile_key = self.insert_node(NodeData::Leaf(tile));
        if let Some(container) = self.get_container_mut(container_key) {
            container.insert_child(info.insert_idx, tile_key);
            if info.child_percents.len() == container.child_percents.len() {
                container.child_percents = info.child_percents.clone();
                container.normalize_child_percents();
            }
        }
        self.set_parent(tile_key, Some(container_key));

        if focus {
            self.focus_node_key(tile_key);
        } else if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        } else {
            self.focus_first_leaf();
        }

        true
    }

    pub(super) fn insert_subtree_with_parent_info(
        &mut self,
        info: &InsertParentInfo,
        subtree: DetachedNode<W>,
        focus: bool,
    ) -> bool {
        let container_key = match self.ensure_container_at_path(&info.parent_path, info.layout) {
            Some(key) => key,
            None => {
                self.insert_subtree_at_root(self.root_children_len(), subtree, focus);
                return true;
            }
        };

        let node_key = self.insert_subtree(subtree);
        if let Some(container) = self.get_container_mut(container_key) {
            container.insert_child(info.insert_idx, node_key);
            if info.child_percents.len() == container.child_percents.len() {
                container.child_percents = info.child_percents.clone();
                container.normalize_child_percents();
            }
        }
        self.set_parent(node_key, Some(container_key));

        if focus {
            self.focus_node_key(node_key);
        } else if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        } else {
            self.focus_first_leaf();
        }

        true
    }

    pub fn insert_leaf_split(
        &mut self,
        target_path: &[usize],
        direction: Direction,
        tile: Tile<W>,
        focus: bool,
    ) -> bool {
        if self.root.is_none() {
            self.append_leaf(tile, focus);
            return true;
        }

        let desired_layout = if direction.is_horizontal() {
            Layout::SplitH
        } else {
            Layout::SplitV
        };

        if target_path.is_empty() {
            let Some(root_key) = self.root else {
                self.append_leaf(tile, focus);
                return true;
            };
            if !matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
                self.append_leaf(tile, focus);
                return true;
            }

            let tile_key = self.insert_node(NodeData::Leaf(tile));
            let mut container = ContainerData::new(desired_layout);
            container.mark_preserve_on_single();
            match direction {
                Direction::Left | Direction::Up => {
                    container.add_child(tile_key);
                    container.add_child(root_key);
                }
                Direction::Right | Direction::Down => {
                    container.add_child(root_key);
                    container.add_child(tile_key);
                }
            }

            let container_key = self.insert_node(NodeData::Container(container));
            self.set_parent(tile_key, Some(container_key));
            self.set_parent(root_key, Some(container_key));
            self.set_parent(container_key, None);
            self.root = Some(container_key);

            if focus {
                self.focus_node_key(tile_key);
            } else if let Some(key) = self.focused_key {
                self.sync_container_focus_from_key(key);
            } else {
                self.focus_first_leaf();
            }
            return true;
        }

        let parent_path = &target_path[..target_path.len() - 1];
        let target_idx = *target_path.last().unwrap();
        let parent_key = if parent_path.is_empty() {
            self.root
        } else {
            self.get_node_key_at_path(parent_path)
        };
        let Some(parent_key) = parent_key else {
            self.append_leaf(tile, focus);
            return true;
        };

        let parent = match self.get_container(parent_key) {
            Some(container) => container,
            None => {
                self.append_leaf(tile, focus);
                return true;
            }
        };
        let target_key = match parent.child_key(target_idx) {
            Some(key) => key,
            None => {
                self.append_leaf(tile, focus);
                return true;
            }
        };

        let parent_layout = parent.layout();
        if matches!(parent_layout, Layout::SplitH | Layout::SplitV) && parent_layout == desired_layout
        {
            let insert_idx = match direction {
                Direction::Left | Direction::Up => target_idx,
                Direction::Right | Direction::Down => target_idx + 1,
            };
            let tile_key = self.insert_node(NodeData::Leaf(tile));

            let container = self
                .get_container_mut(parent_key)
                .expect("insert split parent missing");
            container.insert_child(insert_idx, tile_key);

            self.set_parent(tile_key, Some(parent_key));
            if focus {
                self.focus_node_key(tile_key);
            } else if let Some(key) = self.focused_key {
                self.sync_container_focus_from_key(key);
            } else {
                self.focus_first_leaf();
            }
            return true;
        }

        let tile_key = self.insert_node(NodeData::Leaf(tile));
        let mut new_container = ContainerData::new(desired_layout);
        new_container.mark_preserve_on_single();
        match direction {
            Direction::Left | Direction::Up => {
                new_container.add_child(tile_key);
                new_container.add_child(target_key);
            }
            Direction::Right | Direction::Down => {
                new_container.add_child(target_key);
                new_container.add_child(tile_key);
            }
        }
        let new_container_key = self.insert_node(NodeData::Container(new_container));

        self.set_parent(tile_key, Some(new_container_key));
        self.set_parent(target_key, Some(new_container_key));

        let container = self
            .get_container_mut(parent_key)
            .expect("insert split parent missing");
        let idx = container
            .children
            .iter()
            .position(|child| *child == target_key)
            .expect("insert split target missing");
        container.children[idx] = new_container_key;

        if let Some(pos) = container
            .focus_stack
            .iter()
            .position(|key| *key == target_key)
        {
            container.focus_stack[pos] = new_container_key;
        } else if !container.focus_stack.contains(&new_container_key) {
            container.focus_stack.push(new_container_key);
        }
        container.ensure_focus_stack();

        self.set_parent(new_container_key, Some(parent_key));

        if focus {
            self.focus_node_key(tile_key);
        } else if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        } else {
            self.focus_first_leaf();
        }

        true
    }

    pub fn insert_leaf_split_root(
        &mut self,
        direction: Direction,
        tile: Tile<W>,
        focus: bool,
    ) -> bool {
        let desired_layout = if direction.is_horizontal() {
            Layout::SplitH
        } else {
            Layout::SplitV
        };

        if self.root.is_none() {
            self.append_leaf(tile, focus);
            return true;
        }

        let Some(root_key) = self.root else {
            self.append_leaf(tile, focus);
            return true;
        };

        let tile_key = self.insert_node(NodeData::Leaf(tile));

        if let Some(root_container) = self.get_container(root_key) {
            if root_container.layout() == desired_layout {
                let insert_idx = match direction {
                    Direction::Left | Direction::Up => 0,
                    Direction::Right | Direction::Down => root_container.child_count(),
                };
                if let Some(container) = self.get_container_mut(root_key) {
                    container.insert_child(insert_idx, tile_key);
                }

                self.set_parent(tile_key, Some(root_key));
                if focus {
                    self.focus_node_key(tile_key);
                } else if let Some(key) = self.focused_key {
                    self.sync_container_focus_from_key(key);
                } else {
                    self.focus_first_leaf();
                }
                return true;
            }
        }

        let mut new_container = ContainerData::new(desired_layout);
        new_container.mark_preserve_on_single();
        match direction {
            Direction::Left | Direction::Up => {
                new_container.add_child(tile_key);
                new_container.add_child(root_key);
            }
            Direction::Right | Direction::Down => {
                new_container.add_child(root_key);
                new_container.add_child(tile_key);
            }
        }
        let new_container_key = self.insert_node(NodeData::Container(new_container));

        self.set_parent(tile_key, Some(new_container_key));
        self.set_parent(root_key, Some(new_container_key));
        self.set_parent(new_container_key, None);
        self.root = Some(new_container_key);

        if focus {
            self.focus_node_key(tile_key);
        } else if let Some(key) = self.focused_key {
            self.sync_container_focus_from_key(key);
        } else {
            self.focus_first_leaf();
        }

        true
    }

    // ========================================================================
    // Helper methods
    // ========================================================================

    fn cleanup_containers(&mut self, mut key: Option<NodeKey>) {
        loop {
            let Some(container_key) = key else {
                if let Some(root_key) = self.root {
                    if let Some(container) = self.get_container(root_key) {
                        if container.children.is_empty() {
                            self.pending_layout = Some(container.layout());
                            self.remove_node_recursive(root_key);
                            self.root = None;
                        }
                    }
                }
                break;
            };

            let parent_key = self.parent_of(container_key);
            let Some(container) = self.get_container(container_key) else {
                key = parent_key;
                continue;
            };

            let mut remove_container = false;
            let mut replace_with_child = None;
            let container_layout = container.layout();
            let child_count = container.children.len();
            let can_replace_with_child = !container.preserve_on_single()
                && parent_key
                    .and_then(|parent_key| {
                        self.get_container(parent_key)
                            .map(|parent| parent.layout() == container_layout)
                    })
                    .unwrap_or(false);

            if child_count == 0 {
                remove_container = true;
            } else if child_count == 1 && can_replace_with_child {
                replace_with_child = container.child_key(0);
            }

            if let Some(parent_key) = parent_key {
                let parent_idx = match self.child_index(parent_key, container_key) {
                    Some(idx) => idx,
                    None => {
                        key = Some(parent_key);
                        continue;
                    }
                };

                if remove_container {
                    if let Some(parent) = self.get_container_mut(parent_key) {
                        parent.remove_child(parent_idx);
                    }
                    self.set_parent(container_key, None);
                    self.remove_node_recursive(container_key);
                } else if let Some(child_key) = replace_with_child {
                    if let Some(parent) = self.get_container_mut(parent_key) {
                        parent.children[parent_idx] = child_key;
                        if let Some(pos) = parent
                            .focus_stack
                            .iter()
                            .position(|key| *key == container_key)
                        {
                            parent.focus_stack[pos] = child_key;
                        } else if !parent.focus_stack.contains(&child_key) {
                            parent.focus_stack.push(child_key);
                        }
                        parent.ensure_focus_stack();
                    }
                    self.set_parent(child_key, Some(parent_key));
                    self.nodes.remove(container_key);
                    self.parents.remove(container_key);
                }
            } else if remove_container {
                self.pending_layout = Some(container.layout());
                self.remove_node_recursive(container_key);
                self.root = None;
            } else if let Some(child_key) = replace_with_child {
                self.set_parent(child_key, None);
                self.nodes.remove(container_key);
                self.parents.remove(container_key);
                self.root = Some(child_key);
            }

            key = parent_key;
        }
    }


    fn focus_first_leaf(&mut self) {
        if let Some(key) = self.first_leaf_key() {
            self.focus_node_key(key);
        } else {
            self.focused_key = None;
        }
    }

    fn ensure_root_container(&mut self) -> NodeKey {
        if self.root.is_none() {
            let explicit_layout = self.pending_layout.is_some();
            let layout = self.pending_layout.take().unwrap_or(Layout::SplitH);
            let mut container = ContainerData::new(layout);
            if explicit_layout {
                container.mark_preserve_on_single();
            }
            let container_key = self.insert_node(NodeData::Container(container));
            self.set_parent(container_key, None);
            self.root = Some(container_key);
            self.focused_key = None;
            return container_key;
        }

        let root_key = self.root.unwrap();
        let needs_conversion = matches!(self.get_node(root_key), Some(NodeData::Leaf(_)));

        if needs_conversion {
            let old_root_key = self.root.take().unwrap();
            let mut container = ContainerData::new(Layout::SplitH);
            container.add_child(old_root_key);
            let container_key = self.insert_node(NodeData::Container(container));
            self.set_parent(old_root_key, Some(container_key));
            self.set_parent(container_key, None);
            self.root = Some(container_key);
            self.focus_node_key(old_root_key);
            container_key
        } else {
            root_key
        }
    }

    fn ensure_container_at_path(&mut self, path: &[usize], layout: Layout) -> Option<NodeKey> {
        let root_key = self.root?;
        if path.is_empty() {
            if matches!(self.get_node(root_key), Some(NodeData::Container(_))) {
                return Some(root_key);
            }

            let mut container = ContainerData::new(layout);
            container.mark_preserve_on_single();
            container.add_child(root_key);
            let container_key = self.insert_node(NodeData::Container(container));
            self.set_parent(root_key, Some(container_key));
            self.set_parent(container_key, None);
            self.root = Some(container_key);
            return Some(container_key);
        }

        let key = self.get_node_key_at_path(path)?;
        if matches!(self.get_node(key), Some(NodeData::Container(_))) {
            return Some(key);
        }

        let parent_path = &path[..path.len() - 1];
        let parent_key = if parent_path.is_empty() {
            self.root?
        } else {
            self.get_node_key_at_path(parent_path)?
        };
        let child_idx = *path.last().unwrap();

        let mut container = ContainerData::new(layout);
        container.mark_preserve_on_single();
        container.add_child(key);
        let container_key = self.insert_node(NodeData::Container(container));
        self.set_parent(key, Some(container_key));

        if let Some(parent) = self.get_container_mut(parent_key) {
            parent.children[child_idx] = container_key;
            if let Some(pos) = parent.focus_stack.iter().position(|k| *k == key) {
                parent.focus_stack[pos] = container_key;
            } else if !parent.focus_stack.contains(&container_key) {
                parent.focus_stack.push(container_key);
            }
            parent.ensure_focus_stack();
        }

        self.set_parent(container_key, Some(parent_key));
        Some(container_key)
    }

    fn move_node_to_grandparent(
        &mut self,
        node_key: NodeKey,
        node_parent_path: &[usize],
        node_idx: usize,
        grandparent_path: &[usize],
        parent_idx: usize,
        direction: Direction,
    ) -> bool {
        let node_parent_key = if node_parent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(node_parent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        let parent_child_count = self
            .get_container(node_parent_key)
            .map(|container| container.child_count())
            .unwrap_or(0);
        let parent_will_be_removed = parent_child_count == 1;

        if let Some(container) = self.get_container_mut(node_parent_key) {
            let _ = container.remove_child(node_idx);
        } else {
            return false;
        }
        self.set_parent(node_key, None);

        let grandparent_key = if grandparent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(grandparent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        let insert_at = match direction {
            Direction::Left | Direction::Up => {
                if parent_will_be_removed {
                    parent_idx.saturating_sub(1)
                } else {
                    parent_idx
                }
            }
            Direction::Right | Direction::Down => {
                if parent_will_be_removed {
                    parent_idx + 2
                } else {
                    parent_idx + 1
                }
            }
        };

        if let Some(container) = self.get_container_mut(grandparent_key) {
            container.insert_child(insert_at, node_key);
        } else {
            return false;
        }
        self.set_parent(node_key, Some(grandparent_key));

        self.cleanup_containers(Some(node_parent_key));

        self.focus_node_key(node_key);

        true
    }

    fn move_node_into_container(
        &mut self,
        node_key: NodeKey,
        node_parent_path: &[usize],
        node_idx: usize,
        target_key: NodeKey,
        direction: Direction,
        target_focus_idx: usize,
    ) -> bool {
        let (insert_idx, child_count) = if let Some(container) = self.get_container(target_key) {
            let child_count = container.child_count();
            let insert_idx = match container.layout() {
                Layout::SplitH | Layout::SplitV => {
                    let axis_matches =
                        (container.layout() == Layout::SplitH && direction.is_horizontal())
                            || (container.layout() == Layout::SplitV && direction.is_vertical());
                    if axis_matches {
                        match direction {
                            Direction::Left | Direction::Up => child_count,
                            Direction::Right | Direction::Down => 0,
                        }
                    } else {
                        match direction {
                            Direction::Left | Direction::Up => target_focus_idx + 1,
                            Direction::Right | Direction::Down => target_focus_idx,
                        }
                    }
                }
                Layout::Tabbed | Layout::Stacked => match direction {
                    Direction::Left | Direction::Up => target_focus_idx,
                    Direction::Right | Direction::Down => target_focus_idx + 1,
                },
            };
            (insert_idx, child_count)
        } else {
            return false;
        };

        let node_parent_key = if node_parent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(node_parent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        if let Some(container) = self.get_container_mut(node_parent_key) {
            let _ = container.remove_child(node_idx);
        } else {
            return false;
        }

        if let Some(container) = self.get_container_mut(target_key) {
            let idx = insert_idx.min(child_count);
            container.insert_child(idx, node_key);
        } else {
            return false;
        }
        self.set_parent(node_key, Some(target_key));

        self.cleanup_containers(Some(node_parent_key));

        self.focus_node_key(node_key);

        true
    }

    #[cfg(test)]
    pub(crate) fn debug_tree(&self) -> String
    where
        W::Id: std::fmt::Display,
    {
        let mut out = String::new();
        let Some(root_key) = self.root else {
            out.push_str("(empty)\n");
            return out;
        };

        let mut path = Vec::new();
        let focused_path = self.focus_path();
        self.debug_tree_node(root_key, &mut path, &mut out, &focused_path);
        out
    }

    #[cfg(test)]
    fn debug_tree_node(
        &self,
        node_key: NodeKey,
        path: &mut Vec<usize>,
        out: &mut String,
        focused_path: &[usize],
    ) where
        W::Id: std::fmt::Display,
    {
        use std::fmt::Write as _;

        let indent = "  ".repeat(path.len());
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => {
                let focused = if *path == focused_path { " *" } else { "" };
                let _ = writeln!(
                    out,
                    "{indent}Window {}{focused}",
                    tile.window().id()
                );
            }
            Some(NodeData::Container(container)) => {
                let label = layout_label(container.layout());
                let _ = writeln!(out, "{indent}{label}");
                for (idx, child_key) in container.children.iter().enumerate() {
                    path.push(idx);
                    self.debug_tree_node(*child_key, path, out, focused_path);
                    path.pop();
                }
            }
            None => {
                let _ = writeln!(out, "{indent}(missing)");
            }
        }
    }
}

impl ContainerTree<Mapped> {
    pub fn layout_tree(&self) -> Option<LayoutTreeNode> {
        let root_key = self.root?;
        let focused_key = self.focused_key.or_else(|| self.first_leaf_key());
        Some(self.build_layout_tree_node(root_key, focused_key))
    }

    fn build_layout_tree_node(
        &self,
        node_key: NodeKey,
        focused_key: Option<NodeKey>,
    ) -> LayoutTreeNode {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => LayoutTreeNode {
                layout: None,
                window_id: Some(tile.window().id().get()),
                focused: focused_key == Some(node_key),
                children: Vec::new(),
            },
            Some(NodeData::Container(container)) => LayoutTreeNode {
                layout: Some(layout_to_ipc(container.layout())),
                window_id: None,
                focused: focused_key == Some(node_key),
                children: container
                    .children
                    .iter()
                    .map(|child_key| self.build_layout_tree_node(*child_key, focused_key))
                    .collect(),
            },
            None => LayoutTreeNode {
                layout: None,
                window_id: None,
                focused: false,
                children: Vec::new(),
            },
        }
    }
}

fn layout_to_ipc(layout: Layout) -> LayoutTreeLayout {
    match layout {
        Layout::SplitH => LayoutTreeLayout::SplitH,
        Layout::SplitV => LayoutTreeLayout::SplitV,
        Layout::Tabbed => LayoutTreeLayout::Tabbed,
        Layout::Stacked => LayoutTreeLayout::Stacked,
    }
}

// ============================================================================
// Additional helper implementations
// ============================================================================

impl Default for Layout {
    fn default() -> Self {
        Layout::SplitH
    }
}

impl Direction {
    /// Get the opposite direction
    pub fn opposite(self) -> Self {
        match self {
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
        }
    }

    /// Check if direction is horizontal
    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }

    /// Check if direction is vertical
    pub fn is_vertical(self) -> bool {
        matches!(self, Direction::Up | Direction::Down)
    }
}

#[cfg(test)]
fn layout_label(layout: Layout) -> &'static str {
    match layout {
        Layout::SplitH => "SplitH",
        Layout::SplitV => "SplitV",
        Layout::Tabbed => "Tabbed",
        Layout::Stacked => "Stacked",
    }
}
