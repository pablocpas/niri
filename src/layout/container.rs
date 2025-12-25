//! i3-style container tree implementation using SlotMap
//!
//! This module implements the hierarchical container system used by i3wm.
//! Containers form a tree where:
//! - Leaf nodes contain windows (wrapped in Tiles)
//! - Internal nodes contain child containers with a specific layout
//! - Each container can have layouts: SplitH, SplitV, Tabbed, or Stacked
//!
//! Uses slotmap for efficient memory management and O(1) access to nodes.

use std::rc::Rc;

use slotmap::{new_key_type, SlotMap};
use smithay::utils::{Logical, Point, Rectangle, Size};

use super::tile::Tile;
use super::{LayoutElement, Options};
use crate::utils::round_logical_in_physical_max1;
use crate::window::Mapped;
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
    focused_idx: usize,
}

/// Container data stored in slotmap
#[derive(Debug)]
pub struct ContainerData {
    /// Layout mode for this container
    layout: Layout,
    /// Child node keys (indices into the tree's SlotMap)
    children: Vec<NodeKey>,
    /// Relative sizes of children (sum normalized to 1.0 for split layouts)
    child_percents: Vec<f64>,
    /// Index of focused child
    focused_idx: usize,
    /// Cached geometry for rendering
    geometry: Rectangle<f64, Logical>,
}

/// Cached layout information for a leaf tile.
#[derive(Debug, Clone)]
pub struct LeafLayoutInfo {
    pub path: Vec<usize>,
    pub rect: Rectangle<f64, Logical>,
    pub visible: bool,
}

/// Root container tree for a workspace
#[derive(Debug)]
pub struct ContainerTree<W: LayoutElement> {
    /// SlotMap storing all nodes in the tree
    nodes: SlotMap<NodeKey, NodeData<W>>,
    /// Root node key
    root: Option<NodeKey>,
    /// Path to currently focused node (indices from root to leaf)
    focus_path: Vec<usize>,
    /// Stack of focus paths for parent/child navigation
    focus_parent_stack: Vec<Vec<usize>>,
    /// Cached layout info for leaves
    leaf_layouts: Vec<LeafLayoutInfo>,
    /// View size (output size)
    view_size: Size<f64, Logical>,
    /// Working area (view_size minus gaps/bars)
    working_area: Rectangle<f64, Logical>,
    /// Display scale
    scale: f64,
    /// Layout options
    options: Rc<Options>,
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
            child_percents: Vec::new(),
            focused_idx: 0,
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

    /// Get children keys
    pub fn children(&self) -> &[NodeKey] {
        &self.children
    }

    /// Number of children
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get focused child index
    pub fn focused_idx(&self) -> usize {
        self.focused_idx
    }

    /// Set focused child index
    pub fn set_focused_idx(&mut self, idx: usize) {
        if idx < self.children.len() {
            self.focused_idx = idx;
        }
    }

    /// Get focused child key
    pub fn focused_child_key(&self) -> Option<NodeKey> {
        self.children.get(self.focused_idx).copied()
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
        let removed_percent = if self.child_percents.len() == self.children.len() + 1 {
            self.child_percents.remove(idx)
        } else {
            0.0
        };

        // Adjust focused index if needed
        if self.focused_idx >= self.children.len() && self.focused_idx > 0 {
            self.focused_idx = self.children.len() - 1;
        }

        if self.children.is_empty() {
            self.child_percents.clear();
            return Some(key);
        }

        if self.child_percents.len() != self.children.len() {
            self.recalculate_percentages();
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
            focused_idx: 0,
        };
        container.recalculate_percentages();
        container
    }

    pub(crate) fn from_parts(
        layout: Layout,
        children: Vec<DetachedNode<W>>,
        child_percents: Vec<f64>,
        focused_idx: usize,
    ) -> Self {
        let mut container = Self {
            layout,
            children,
            child_percents,
            focused_idx,
        };
        container.normalize_child_percents();
        if container.focused_idx >= container.children.len() {
            container.focused_idx = container.children.len().saturating_sub(1);
        }
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
            root: None,
            focus_path: Vec::new(),
            focus_parent_stack: Vec::new(),
            leaf_layouts: Vec::new(),
            view_size,
            working_area,
            scale,
            options,
        }
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

    /// Get tile by key
    fn get_tile(&self, key: NodeKey) -> Option<&Tile<W>> {
        match self.nodes.get(key)? {
            NodeData::Leaf(tile) => Some(tile),
            _ => None,
        }
    }

    /// Get mutable tile by key
    fn get_tile_mut(&mut self, key: NodeKey) -> Option<&mut Tile<W>> {
        match self.nodes.get_mut(key)? {
            NodeData::Leaf(tile) => Some(tile),
            _ => None,
        }
    }

    /// Insert a new node into the slotmap
    fn insert_node(&mut self, node: NodeData<W>) -> NodeKey {
        self.nodes.insert(node)
    }

    /// Remove a node from the slotmap (and recursively all its children)
    fn remove_node_recursive(&mut self, key: NodeKey) -> Option<NodeData<W>> {
        let node = self.nodes.remove(key)?;

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
            let key = self.insert_node(NodeData::Leaf(tile));
            self.root = Some(key);
            self.focus_path.clear();
            return;
        }

        // Ensure the root is a container so we can insert siblings easily
        let root_key = self.root.unwrap();
        let focus_path = if matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
            // Convert the root leaf into a container
            let old_root_key = self.root.take().unwrap();
            let mut container = ContainerData::new(Layout::SplitH);
            container.add_child(old_root_key);
            container.set_focused_idx(0);

            let container_key = self.insert_node(NodeData::Container(container));
            self.root = Some(container_key);
            self.focus_path = vec![0];
            self.focus_path.clone()
        } else {
            self.focus_path.clone()
        };

        // Insert as sibling in the parent container
        if focus_path.is_empty() {
            // Append to root container
            if let Some(root_key) = self.root {
                let tile_key = self.insert_node(NodeData::Leaf(tile));
                if let Some(NodeData::Container(container)) = self.get_node_mut(root_key) {
                    let insert_idx = container.children.len();
                    container.insert_child(insert_idx, tile_key);
                    container.set_focused_idx(insert_idx);
                    self.focus_path = vec![insert_idx];
                }
            }
            return;
        }

        let parent_path = &focus_path[..focus_path.len() - 1];
        let current_idx = *focus_path.last().unwrap();

        // Get parent container and insert
        let tile_key = self.insert_node(NodeData::Leaf(tile));
        if let Some(parent_key) = self.get_node_key_at_path(parent_path) {
            if let Some(NodeData::Container(parent_container)) = self.get_node_mut(parent_key) {
                let insert_idx = current_idx + 1;
                parent_container.insert_child(insert_idx, tile_key);
                parent_container.set_focused_idx(insert_idx);

                self.focus_path.truncate(parent_path.len());
                self.focus_path.push(insert_idx);
                self.focus_to_first_leaf_from_path();
                return;
            }
        }

        // Fallback: append to root container
        if let Some(root_key) = self.root {
            if let Some(NodeData::Container(container)) = self.get_node_mut(root_key) {
                let insert_idx = container.children.len();
                container.insert_child(insert_idx, tile_key);
                container.set_focused_idx(insert_idx);
                self.focus_path = vec![insert_idx];
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

    /// Helper: navigate to first leaf from current focus_path
    fn focus_to_first_leaf_from_path(&mut self) {
        let mut current_path = self.focus_path.clone();

        loop {
            if let Some(key) = self.get_node_key_at_path(&current_path) {
                match self.get_node(key) {
                    Some(NodeData::Leaf(_)) => {
                        // Reached a leaf
                        self.focus_path = current_path;
                        return;
                    }
                    Some(NodeData::Container(container)) => {
                        if container.children.is_empty() {
                            return;
                        }
                        // Navigate to focused child
                        current_path.push(container.focused_idx);
                    }
                    None => return,
                }
            } else {
                return;
            }
        }
    }

    fn clear_focus_history(&mut self) {
        self.focus_parent_stack.clear();
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
        let key = self.get_node_key_at_path(&self.focus_path)?;
        self.get_tile(key).map(|tile| tile.window())
    }

    /// Get the currently focused window (mutable)
    pub fn focused_window_mut(&mut self) -> Option<&mut W> {
        let path = self.focus_path.clone();
        let key = self.get_node_key_at_path(&path)?;
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

    /// Current focus path within the tree.
    pub fn focus_path(&self) -> &[usize] {
        &self.focus_path
    }

    /// Focused tile (if any).
    pub fn focused_tile(&self) -> Option<&Tile<W>> {
        let key = self.get_node_key_at_path(self.focus_path())?;
        self.get_tile(key)
    }

    /// Focused tile (mutable) if any.
    pub fn focused_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        let path = self.focus_path.clone();
        let key = self.get_node_key_at_path(&path)?;
        self.get_tile_mut(key)
    }

    /// Calculate and apply layout to the tree
    pub fn layout(&mut self) {
        self.leaf_layouts.clear();

        if let Some(root_key) = self.root {
            let mut path = Vec::new();
            let mut area = self.working_area;
            let gap = self.options.layout.gaps;
            if gap > 0.0 {
                area.loc.x += gap;
                area.loc.y += gap;
                area.size.w = (area.size.w - gap * 2.0).max(0.0);
                area.size.h = (area.size.h - gap * 2.0).max(0.0);
            }
            self.layout_node(root_key, area, &mut path, true);
        }
    }

    /// Helper: recursively layout a node
    fn layout_node(
        &mut self,
        node_key: NodeKey,
        rect: Rectangle<f64, Logical>,
        path: &mut Vec<usize>,
        visible: bool,
    ) {
        // We need to work around borrow checker by getting info first
        let node_info = match self.get_node(node_key) {
            Some(NodeData::Leaf(_)) => {
                // Handle leaf
                if let Some(NodeData::Leaf(tile)) = self.get_node_mut(node_key) {
                    let size = Size::from((rect.size.w, rect.size.h));
                    tile.request_tile_size(size, false, None);
                    self.leaf_layouts.push(LeafLayoutInfo {
                        path: path.clone(),
                        rect,
                        visible,
                    });
                }
                return;
            }
            Some(NodeData::Container(container)) => {
                // Collect container info
                (
                    container.layout,
                    container.children.clone(),
                    container.child_percents.clone(),
                    container.focused_idx,
                )
            }
            None => return,
        };

        let (layout, children, child_percents, focused_idx) = node_info;

        // Update container geometry
        if let Some(NodeData::Container(container)) = self.get_node_mut(node_key) {
            container.set_geometry(rect);
        }

        if children.is_empty() {
            return;
        }

        let gap = self.options.layout.gaps;

        match layout {
            Layout::SplitH => {
                // Horizontal split
                let child_count = children.len();
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_width = (rect.size.w - total_gap).max(0.0);

                let total_percent: f64 = child_percents.iter().copied().sum();
                let percents: Vec<f64> = if total_percent > f64::EPSILON {
                    child_percents.iter().map(|p| p / total_percent).collect()
                } else {
                    vec![1.0 / child_count as f64; child_count]
                };

                let mut cursor_x = rect.loc.x;
                let mut used_width = 0.0;

                for (idx, &child_key) in children.iter().enumerate() {
                    let percent = percents[idx];
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
                    self.layout_node(child_key, child_rect, path, visible);
                    path.pop();

                    used_width += width;
                    if idx + 1 < child_count {
                        cursor_x += width + gap;
                    }
                }
            }
            Layout::SplitV => {
                // Vertical split
                let child_count = children.len();
                let total_gap = if child_count > 1 {
                    gap * (child_count as f64 - 1.0)
                } else {
                    0.0
                };
                let available_height = (rect.size.h - total_gap).max(0.0);

                let total_percent: f64 = child_percents.iter().copied().sum();
                let percents: Vec<f64> = if total_percent > f64::EPSILON {
                    child_percents.iter().map(|p| p / total_percent).collect()
                } else {
                    vec![1.0 / child_count as f64; child_count]
                };

                let mut cursor_y = rect.loc.y;
                let mut used_height = 0.0;

                for (idx, &child_key) in children.iter().enumerate() {
                    let percent = percents[idx];
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
                    self.layout_node(child_key, child_rect, path, visible);
                    path.pop();

                    used_height += height;
                    if idx + 1 < child_count {
                        cursor_y += height + gap;
                    }
                }
            }
            Layout::Tabbed | Layout::Stacked => {
                // All children get full size, only focused is visible.
                let child_count = children.len();
                let mut inner_rect = rect;
                if gap > 0.0 {
                    inner_rect.loc.x += gap;
                    inner_rect.loc.y += gap;
                    inner_rect.size.w = (inner_rect.size.w - gap * 2.0).max(0.0);
                    inner_rect.size.h = (inner_rect.size.h - gap * 2.0).max(0.0);
                }

                let mut child_rect = inner_rect;
                let bar_row_height = self.tab_bar_row_height();
                if bar_row_height > 0.0 && child_count > 0 {
                    let bar_height = match layout {
                        Layout::Tabbed => bar_row_height,
                        Layout::Stacked => bar_row_height * child_count as f64,
                        _ => 0.0,
                    };
                    let total_bar_height = (bar_height + self.tab_bar_spacing())
                        .min(inner_rect.size.h)
                        .max(0.0);
                    child_rect.loc.y += total_bar_height;
                    child_rect.size.h = (child_rect.size.h - total_bar_height).max(0.0);
                }

                let focused_idx = focused_idx.min(children.len().saturating_sub(1));

                for (idx, &child_key) in children.iter().enumerate() {
                    path.push(idx);
                    let child_visible = visible && idx == focused_idx;
                    self.layout_node(child_key, child_rect, path, child_visible);
                    path.pop();
                }
            }
        }
    }

    fn tab_bar_row_height(&self) -> f64 {
        if self.options.layout.tab_bar.off {
            return 0.0;
        }
        round_logical_in_physical_max1(self.scale, self.options.layout.tab_bar.height)
    }

    fn tab_bar_spacing(&self) -> f64 {
        let focus_ring = self.options.layout.focus_ring;
        let border = self.options.layout.border;
        let focus_width = if focus_ring.off { 0.0 } else { focus_ring.width };
        let border_width = if border.off { 0.0 } else { border.width };
        round_logical_in_physical_max1(self.scale, focus_width.max(border_width))
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
                let tabs = container
                    .children
                    .iter()
                    .enumerate()
                    .map(|(idx, &child_key)| TabBarTab {
                        title: self.focused_title_in_subtree(child_key),
                        is_focused: idx == container.focused_idx,
                        is_urgent: self.subtree_has_urgent(child_key),
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

        for (idx, &child_key) in container.children.iter().enumerate() {
            path.push(idx);
            let child_visible = match container.layout {
                Layout::Tabbed | Layout::Stacked => idx == container.focused_idx,
                _ => true,
            };
            self.collect_tab_bar_layouts(child_key, path, out, visible && child_visible);
            path.pop();
        }
    }

    fn focused_title_in_subtree(&self, node_key: NodeKey) -> String {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => tile
                .window()
                .title()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| String::from("untitled")),
            Some(NodeData::Container(container)) => {
                let focused_idx = container
                    .focused_idx
                    .min(container.children.len().saturating_sub(1));
                let Some(child_key) = container.child_key(focused_idx) else {
                    return String::from("untitled");
                };
                self.focused_title_in_subtree(child_key)
            }
            None => String::from("untitled"),
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

        let focus_path_clone = self.focus_path.clone();

        // Navigate up the focus path to find appropriate container
        for depth in (0..focus_path_clone.len()).rev() {
            let parent_path = &focus_path_clone[..depth];
            let current_idx = if depth < focus_path_clone.len() {
                focus_path_clone[depth]
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
                        (Layout::SplitH, Direction::Left | Direction::Right) => true,
                        (Layout::SplitV, Direction::Up | Direction::Down) => true,
                        (Layout::Tabbed | Layout::Stacked, _) => true,
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
                        // Update focus to new sibling
                        if let Some(container) = self.get_container_mut(parent_key) {
                            container.set_focused_idx(new_idx);
                        }

                        self.focus_path.truncate(depth);
                        self.focus_path.push(new_idx);
                        self.focus_to_first_leaf_from_path();
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
        if let Some(path) = self.find_window(window_id) {
            self.focus_path = path;
            self.focus_to_first_leaf_from_path();
            true
        } else {
            false
        }
    }

    pub fn focus_parent(&mut self) -> bool {
        if self.focus_path.is_empty() {
            return false;
        }
        self.focus_parent_stack.push(self.focus_path.clone());
        self.focus_path.pop();
        self.focus_to_first_leaf_from_path();
        true
    }

    pub fn focus_child(&mut self) -> bool {
        let Some(path) = self.focus_parent_stack.pop() else {
            return false;
        };

        if self.get_node_key_at_path(&path).is_none() {
            self.focus_parent_stack.clear();
            return false;
        }

        self.focus_path = path;
        self.focus_to_first_leaf_from_path();
        true
    }

    // ========================================================================
    // Management methods
    // ========================================================================

    /// Remove a window by ID, returns the removed tile
    pub fn remove_window(&mut self, window_id: &W::Id) -> Option<Tile<W>> {
        let path = self.find_window(window_id)?;
        let node_key = self.get_node_key_at_path(&path)?;

        // First, remove from parent's children list BEFORE removing from slotmap
        if !path.is_empty() {
            let parent_path = &path[..path.len() - 1];
            let child_idx = *path.last().unwrap();

            if let Some(parent_key) = self.get_node_key_at_path(parent_path) {
                if let Some(container) = self.get_container_mut(parent_key) {
                    container.remove_child(child_idx);
                }
            }
        } else {
            // Was root
            self.root = None;
        }

        // Now remove from slotmap (only the leaf, not recursive)
        let node_data = self.nodes.remove(node_key)?;
        let tile = match node_data {
            NodeData::Leaf(tile) => tile,
            NodeData::Container(_) => return None, // Should never happen
        };

        let container_path = if path.is_empty() {
            Vec::new()
        } else {
            path[..path.len() - 1].to_vec()
        };

        self.cleanup_containers(container_path.clone());

        if self.root.is_none() {
            self.focus_path.clear();
        } else {
            if self.focus_path.starts_with(&path) || self.focus_path == path {
                self.focus_path = container_path;
            }
            self.focus_first_leaf();
        }

        self.layout();

        Some(tile)
    }

    /// Move window in a direction (swaps with sibling)
    pub fn move_in_direction(&mut self, direction: Direction) -> bool {
        self.clear_focus_history();
        if self.root.is_none() {
            return false;
        }

        let focus_path = self.focus_path.clone();
        if focus_path.is_empty() {
            return false;
        }

        let leaf_parent_path = &focus_path[..focus_path.len() - 1];
        let leaf_idx = *focus_path.last().unwrap();

        let parent_key = if leaf_parent_path.is_empty() {
            self.root
        } else {
            self.get_node_key_at_path(leaf_parent_path)
        };

        let Some(parent_key) = parent_key else {
            return false;
        };

        let Some(parent_layout) = self.get_container(parent_key).map(|c| c.layout()) else {
            return false;
        };

        let layout_matches = match (parent_layout, direction) {
            (Layout::SplitH, Direction::Left | Direction::Right) => true,
            (Layout::SplitV, Direction::Up | Direction::Down) => true,
            (Layout::Tabbed | Layout::Stacked, _) => true,
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
                    if leaf_idx > 0 {
                        Some(leaf_idx - 1)
                    } else {
                        None
                    }
                }
                Direction::Right | Direction::Down => {
                    if leaf_idx + 1 < child_count {
                        Some(leaf_idx + 1)
                    } else {
                        None
                    }
                }
            };

            let Some(target_idx) = target_idx else {
                return false;
            };

            let target_key = match self.get_container(parent_key).and_then(|c| c.child_key(target_idx)) {
                Some(key) => key,
                None => return false,
            };

            if matches!(parent_layout, Layout::SplitH | Layout::SplitV) {
                if let Some(target_container) = self.get_container(target_key) {
                    if target_container.layout() != parent_layout {
                        return self.move_leaf_into_container(
                            leaf_parent_path,
                            leaf_idx,
                            target_key,
                            direction,
                            target_container.focused_idx(),
                        );
                    }
                }
            }

            if let Some(container) = self.get_container_mut(parent_key) {
                container.children.swap(leaf_idx, target_idx);
                container.child_percents.swap(leaf_idx, target_idx);
                container.set_focused_idx(target_idx);
            }

            self.focus_path.truncate(leaf_parent_path.len());
            self.focus_path.push(target_idx);
            self.focus_to_first_leaf_from_path();
            return true;
        }

        if leaf_parent_path.is_empty() {
            return false;
        }

        let grandparent_path = &leaf_parent_path[..leaf_parent_path.len() - 1];
        let parent_idx = *leaf_parent_path.last().unwrap();

        self.reparent_leaf_to_grandparent(
            leaf_parent_path,
            leaf_idx,
            grandparent_path,
            parent_idx,
            direction,
        )
    }

    /// Split the focused container in a direction
    pub fn split_focused(&mut self, layout: Layout) -> bool {
        self.clear_focus_history();
        if self.root.is_none() {
            return false;
        }

        let focus_path = self.focus_path.clone();

        // Special case: if root is a leaf, wrap it in a container
        if focus_path.is_empty() {
            if let Some(root_key) = self.root {
                if matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
                    let old_root_key = self.root.take().unwrap();
                    let mut container = ContainerData::new(layout);
                    container.add_child(old_root_key);
                    let container_key = self.insert_node(NodeData::Container(container));
                    self.root = Some(container_key);
                    self.focus_path = vec![0];
                    return true;
                }
            }
        }

        if focus_path.is_empty() {
            return false;
        }

        let parent_path = &focus_path[..focus_path.len() - 1];
        let child_idx = *focus_path.last().unwrap();

        let parent_key = if parent_path.is_empty() {
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

            // Remove child from parent
            if let Some(container) = self.get_container_mut(parent_key) {
                container.remove_child(child_idx);
            }

            // Create new container with the leaf
            let mut new_container = ContainerData::new(layout);
            new_container.add_child(focused_child_key);
            let new_container_key = self.insert_node(NodeData::Container(new_container));

            // Insert new container back at same position
            if let Some(container) = self.get_container_mut(parent_key) {
                container.insert_child(child_idx, new_container_key);
            }

            // Update focus path to point inside new container
            self.focus_path.push(0);
            return true;
        }

        false
    }

    /// Change layout of focused container
    pub fn set_focused_layout(&mut self, layout: Layout) -> bool {
        let focus_path = self.focus_path.clone();

        if focus_path.is_empty() {
            if let Some(root_key) = self.root {
                if matches!(self.get_node(root_key), Some(NodeData::Leaf(_))) {
                    let old_root_key = self.root.take().unwrap();
                    let mut container = ContainerData::new(layout);
                    container.add_child(old_root_key);
                    container.set_focused_idx(0);
                    let container_key = self.insert_node(NodeData::Container(container));
                    self.root = Some(container_key);
                    self.focus_path = vec![0];
                    return true;
                }
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
                let parent_key = if parent_path.is_empty() {
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

                if let Some(container) = self.get_container_mut(parent_key) {
                    container.set_layout(layout);
                    return true;
                }
            } else {
                // It's already a container, change its layout
                if let Some(container) = self.get_container_mut(node_key) {
                    container.set_layout(layout);
                    return true;
                }
            }
        }

        false
    }

    /// Layout of the container that currently owns the focused leaf (if any).
    pub fn focused_layout(&self) -> Option<Layout> {
        if self.focus_path.is_empty() {
            let root_key = self.root?;
            self.get_container(root_key).map(|c| c.layout())
        } else {
            let parent_path = &self.focus_path[..self.focus_path.len() - 1];
            let parent_key = if parent_path.is_empty() {
                self.root?
            } else {
                self.get_node_key_at_path(parent_path)?
            };
            self.get_container(parent_key).map(|c| c.layout())
        }
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
        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => Some(0),
            Some(NodeData::Container(container)) => {
                if self.focus_path.is_empty() {
                    Some(
                        container
                            .focused_idx
                            .min(container.children.len().saturating_sub(1)),
                    )
                } else {
                    Some(self.focus_path[0])
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
                    self.focus_path.clear();
                    true
                } else {
                    false
                }
            }
            Some(NodeData::Container(container)) => {
                if idx >= container.children.len() {
                    return false;
                }
                self.focus_path = vec![idx];
                self.focus_to_first_leaf_from_path();
                true
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

        if let Some(first) = self.focus_path.get_mut(0) {
            let current = *first;
            if current == from {
                *first = to;
            } else if from < current && to >= current {
                *first = current.saturating_sub(1);
            } else if from > current && to <= current {
                *first = current + 1;
            }
        } else {
            let root_key = self.root.unwrap();
            if let Some(container) = self.get_container(root_key) {
                self.focus_path = vec![container
                    .focused_idx
                    .min(container.children.len().saturating_sub(1))];
            }
        }

        let default_idx = self.focus_path.get(0).copied();
        if let Some(container) = self.get_container_mut(root_key) {
            container.set_focused_idx(default_idx.unwrap_or(container.focused_idx));
        }

        self.focus_to_first_leaf_from_path();
        true
    }

    /// Extract a subtree rooted at the given key into a detached representation.
    fn extract_subtree(&mut self, key: NodeKey) -> DetachedNode<W> {
        let node_data = self
            .nodes
            .remove(key)
            .expect("node key must exist when extracting subtree");

        match node_data {
            NodeData::Leaf(tile) => DetachedNode::Leaf(tile),
            NodeData::Container(container) => {
                let mut children = Vec::new();
                for child_key in container.children {
                    children.push(self.extract_subtree(child_key));
                }
                DetachedNode::Container(DetachedContainer::from_parts(
                    container.layout,
                    children,
                    container.child_percents,
                    container.focused_idx,
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
                    child_keys.push(self.insert_subtree(child));
                }

                if let Some(node) = self.get_container_mut(container_key) {
                    node.children = child_keys;
                    node.child_percents = container.child_percents;
                    if node.child_percents.len() != node.children.len() {
                        node.recalculate_percentages();
                    } else {
                        node.normalize_child_percents();
                    }
                    node.focused_idx = container
                        .focused_idx
                        .min(node.children.len().saturating_sub(1));
                }

                container_key
            }
        }
    }

    /// Extract all tiles from a subtree rooted at the given key.
    /// This recursively collects all tiles and removes the entire subtree from the slotmap.
    fn extract_tiles_from_subtree(&mut self, key: NodeKey) -> Vec<Tile<W>> {
        let mut tiles = Vec::new();
        self.collect_and_remove_tiles(key, &mut tiles);
        tiles
    }

    /// Recursively collect tiles from a subtree and remove all nodes
    fn collect_and_remove_tiles(&mut self, key: NodeKey, tiles: &mut Vec<Tile<W>>) {
        let node_data = match self.nodes.remove(key) {
            Some(data) => data,
            None => return,
        };

        match node_data {
            NodeData::Leaf(tile) => {
                tiles.push(tile);
            }
            NodeData::Container(container) => {
                for child_key in container.children {
                    self.collect_and_remove_tiles(child_key, tiles);
                }
            }
        }
    }

    /// Remove and return the root-level child at the given index as a detached subtree.
    pub fn take_root_child_subtree(&mut self, idx: usize) -> Option<DetachedNode<W>> {
        let root_key = self.root?;

        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => {
                if idx == 0 {
                    self.focus_path.clear();
                    let subtree = self.extract_subtree(root_key);
                    self.root = None;
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

                let remaining = self.get_container(root_key)?.children.len();

                self.cleanup_containers(Vec::new());

                match self.get_node(root_key) {
                    Some(NodeData::Leaf(_)) | None => {
                        self.focus_path.clear();
                    }
                    Some(NodeData::Container(root_container)) => {
                        if remaining > 0 {
                            let new_idx = idx.min(root_container.children.len().saturating_sub(1));
                            if let Some(container) = self.get_container_mut(root_key) {
                                container.set_focused_idx(new_idx);
                            }
                            self.focus_path = vec![new_idx];
                            self.focus_to_first_leaf_from_path();
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
        self.focus_path = path;
        true
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
            self.focus_path = path.clone();
            true
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
        for tile in tiles {
            let tile_key = self.insert_node(NodeData::Leaf(tile));
            container.add_child(tile_key);
        }
        container.set_focused_idx(0);

        let container_key = self.insert_node(NodeData::Container(container));
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
        let (insert_idx, adjust_threshold) = {
            let container_key = self.ensure_root_container();
            let container = self.get_container(container_key).unwrap();
            let prev_focus_idx = container.focused_idx();
            let idx = index.min(container.children.len());

            if let Some(container) = self.get_container_mut(container_key) {
                container.insert_child(idx, node_key);

                let adjust = if focus {
                    container.set_focused_idx(idx);
                    None
                } else if idx <= prev_focus_idx && container.children.len() > 1 {
                    container.set_focused_idx(prev_focus_idx + 1);
                    Some(idx)
                } else {
                    None
                };

                (idx, adjust)
            } else {
                (idx, None)
            }
        };

        if focus {
            self.focus_path = vec![insert_idx];
            self.focus_to_first_leaf_from_path();
        } else if let Some(threshold) = adjust_threshold {
            if let Some(first) = self.focus_path.get_mut(0) {
                if threshold <= *first {
                    *first += 1;
                }
            }
        } else if self.focus_path.is_empty() {
            if let Some(idx) = self.focused_root_index() {
                self.focus_path = vec![idx];
                self.focus_to_first_leaf_from_path();
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
            if let Some(parent) = self.get_container(parent_key) {
                let insert_idx = current_idx + 1;
                let prev_focus_idx = parent.focused_idx();

                let tile_key = self.insert_node(NodeData::Leaf(tile));

                if let Some(parent) = self.get_container_mut(parent_key) {
                    parent.insert_child(insert_idx, tile_key);

                    if focus {
                        parent.set_focused_idx(insert_idx);
                        self.focus_path = parent_path.to_vec();
                        self.focus_path.push(insert_idx);
                    } else if prev_focus_idx >= insert_idx {
                        parent.set_focused_idx(prev_focus_idx + 1);
                        if let Some(first) = self.focus_path.get_mut(parent_path.len()) {
                            if insert_idx <= *first {
                                *first += 1;
                            }
                        }
                    }

                    if focus {
                        self.focus_to_first_leaf_from_path();
                    }

                    return true;
                }
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
            let (existing_key, existing_percent) =
                if let Some(container) = self.get_container_mut(root_key) {
                    let existing_key = container.children.remove(column_idx);
                    let existing_percent = container.child_percents.remove(column_idx);
                    (existing_key, existing_percent)
                } else {
                    return false;
                };

            // Create new column container
            let mut column_container = ContainerData::new(Layout::SplitV);
            column_container.add_child(existing_key);
            column_container.set_focused_idx(0);
            let column_container_key = self.insert_node(NodeData::Container(column_container));

            // Insert back
            if let Some(container) = self.get_container_mut(root_key) {
                container.children.insert(column_idx, column_container_key);
                container
                    .child_percents
                    .insert(column_idx, existing_percent);
                container.normalize_child_percents();
            }
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

            let focus_path = if focus {
                column_container.set_focused_idx(insert_at);
                Some(vec![column_idx, insert_at])
            } else {
                None
            };

            if let Some(root_container) = self.get_container_mut(root_key) {
                root_container.set_focused_idx(column_idx);
            }

            if let Some(path) = focus_path {
                self.focus_path = path;
                self.focus_to_first_leaf_from_path();
            } else if self.focus_path.get(0) == Some(&column_idx) {
                if self.focus_path.len() > 1 {
                    if let Some(second) = self.focus_path.get_mut(1) {
                        if insert_at <= *second {
                            *second += 1;
                        }
                    }
                }
            } else if self.focus_path.is_empty() {
                self.focus_path = vec![column_idx];
                self.focus_to_first_leaf_from_path();
            }
        }

        true
    }

    // ========================================================================
    // Helper methods
    // ========================================================================

    fn cleanup_containers(&mut self, mut path: Vec<usize>) {
        loop {
            if path.is_empty() {
                if let Some(root_key) = self.root {
                    if let Some(container) = self.get_container(root_key) {
                        if container.children.is_empty() {
                            self.remove_node_recursive(root_key);
                            self.root = None;
                        }
                    }
                }
                break;
            } else {
                let last_idx = *path.last().unwrap();
                let parent_path = &path[..path.len() - 1];

                let parent_key = if parent_path.is_empty() {
                    self.root
                } else {
                    self.get_node_key_at_path(parent_path)
                };

                if let Some(parent_key) = parent_key {
                    let container_key = if let Some(container) = self.get_container(parent_key) {
                        container.child_key(last_idx)
                    } else {
                        None
                    };

                    if let Some(container_key) = container_key {
                        let mut remove_container = false;
                        let mut replace_with_child = None;
                        let mut flatten_container = false;

                        if let Some(container) = self.get_container(container_key) {
                            let parent_layout = self.get_container(parent_key).map(|p| p.layout());
                            if container.children.is_empty() {
                                remove_container = true;
                            } else if container.children.len() == 1 {
                                if parent_layout.map_or(true, |layout| layout == container.layout())
                                {
                                    replace_with_child = container.child_key(0);
                                }
                            } else if parent_layout
                                .is_some_and(|layout| layout == container.layout())
                                && matches!(container.layout(), Layout::SplitH | Layout::SplitV)
                            {
                                flatten_container = true;
                            }
                        }

                        if flatten_container {
                            self.flatten_container_into_parent(
                                parent_key,
                                parent_path,
                                last_idx,
                                container_key,
                            );
                        } else if remove_container {
                            if let Some(parent) = self.get_container_mut(parent_key) {
                                parent.remove_child(last_idx);
                            }
                            self.remove_node_recursive(container_key);
                        } else if let Some(child_key) = replace_with_child {
                            // First replace the container with its child in the parent
                            if let Some(parent) = self.get_container_mut(parent_key) {
                                parent.children[last_idx] = child_key;
                            }
                            // Then remove the now-orphaned container
                            // We need to remove only the container itself, not its child
                            self.nodes.remove(container_key);
                        }
                    }
                }

                if path.is_empty() {
                    break;
                }
                path.pop();
            }
        }
    }

    fn flatten_container_into_parent(
        &mut self,
        parent_key: NodeKey,
        parent_path: &[usize],
        child_idx: usize,
        container_key: NodeKey,
    ) {
        let Some(NodeData::Container(container)) = self.nodes.remove(container_key) else {
            return;
        };

        let child_count = container.children.len();
        if child_count == 0 {
            return;
        }

        let mut child_percents = container.child_percents;
        normalize_percents(&mut child_percents, child_count);

        let container_focus_idx = container.focused_idx;
        if let Some(parent) = self.get_container_mut(parent_key) {
            if parent.child_percents.len() != parent.children.len() {
                parent.recalculate_percentages();
            } else {
                parent.normalize_child_percents();
            }

            let parent_focus_idx = parent.focused_idx;
            let parent_percent = parent
                .child_percents
                .get(child_idx)
                .copied()
                .unwrap_or_else(|| 1.0 / parent.children.len().max(1) as f64);

            parent.children.remove(child_idx);
            if parent.child_percents.len() == parent.children.len() + 1 {
                parent.child_percents.remove(child_idx);
            }

            for (offset, child_key) in container.children.into_iter().enumerate() {
                let insert_idx = child_idx + offset;
                parent.children.insert(insert_idx, child_key);
                let child_percent = child_percents
                    .get(offset)
                    .copied()
                    .unwrap_or_else(|| 1.0 / child_count as f64);
                parent
                    .child_percents
                    .insert(insert_idx, parent_percent * child_percent);
            }

            let mut new_focus_idx = parent_focus_idx;
            if parent_focus_idx > child_idx {
                new_focus_idx = parent_focus_idx + child_count - 1;
            } else if parent_focus_idx == child_idx {
                new_focus_idx = child_idx + container_focus_idx.min(child_count - 1);
            }
            parent.set_focused_idx(new_focus_idx);

            parent.normalize_child_percents();
        }

        self.adjust_focus_after_flatten(parent_path, child_idx, child_count, parent_key);
    }

    fn adjust_focus_after_flatten(
        &mut self,
        parent_path: &[usize],
        child_idx: usize,
        child_count: usize,
        parent_key: NodeKey,
    ) {
        self.focus_parent_stack.clear();
        if !self.focus_path.starts_with(parent_path) {
            return;
        }

        let depth = parent_path.len();
        if self.focus_path.len() <= depth {
            return;
        }

        let current_idx = self.focus_path[depth];
        if current_idx > child_idx {
            self.focus_path[depth] = current_idx + child_count - 1;
        } else if current_idx == child_idx {
            if let Some(inner_idx) = self.focus_path.get(depth + 1).copied() {
                self.focus_path.remove(depth + 1);
                self.focus_path[depth] = child_idx + inner_idx;
            }
        }

        let focus_idx = self.focus_path.get(depth).copied();
        if let Some(container) = self.get_container_mut(parent_key) {
            if let Some(idx) = focus_idx {
                container.set_focused_idx(idx);
            }
        }

    }

    fn focus_first_leaf(&mut self) {
        let mut path = Vec::new();
        let mut current_key = match self.root {
            Some(key) => key,
            None => {
                self.focus_path.clear();
                return;
            }
        };

        loop {
            match self.get_node(current_key) {
                Some(NodeData::Leaf(_)) => {
                    self.focus_path = path;
                    return;
                }
                Some(NodeData::Container(container)) => {
                    if container.children.is_empty() {
                        self.focus_path = path;
                        return;
                    }
                    let idx = container
                        .focused_idx
                        .min(container.children.len().saturating_sub(1));
                    path.push(idx);
                    current_key = container.child_key(idx).unwrap();
                }
                None => {
                    self.focus_path.clear();
                    return;
                }
            }
        }
    }

    fn ensure_root_container(&mut self) -> NodeKey {
        if self.root.is_none() {
            let container = ContainerData::new(Layout::SplitH);
            let container_key = self.insert_node(NodeData::Container(container));
            self.root = Some(container_key);
            self.focus_path = Vec::new();
            return container_key;
        }

        let root_key = self.root.unwrap();
        let needs_conversion = matches!(self.get_node(root_key), Some(NodeData::Leaf(_)));

        if needs_conversion {
            let old_root_key = self.root.take().unwrap();
            let mut container = ContainerData::new(Layout::SplitH);
            container.add_child(old_root_key);
            container.set_focused_idx(0);
            self.focus_path = vec![0];
            let container_key = self.insert_node(NodeData::Container(container));
            self.root = Some(container_key);
            container_key
        } else {
            root_key
        }
    }

    fn reparent_leaf_to_grandparent(
        &mut self,
        leaf_parent_path: &[usize],
        leaf_idx: usize,
        grandparent_path: &[usize],
        parent_idx: usize,
        direction: Direction,
    ) -> bool {
        let leaf_key = match self.get_node_key_at_path(&self.focus_path) {
            Some(key) => key,
            None => return false,
        };
        let window_id = match self.get_node(leaf_key) {
            Some(NodeData::Leaf(tile)) => tile.window().id().clone(),
            _ => return false,
        };

        let leaf_parent_key = if leaf_parent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(leaf_parent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        if let Some(container) = self.get_container_mut(leaf_parent_key) {
            let _ = container.remove_child(leaf_idx);
        } else {
            return false;
        }

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
            Direction::Left | Direction::Up => parent_idx,
            Direction::Right | Direction::Down => parent_idx + 1,
        };

        if let Some(container) = self.get_container_mut(grandparent_key) {
            container.insert_child(insert_at, leaf_key);
            container.set_focused_idx(insert_at);
        } else {
            return false;
        }

        self.cleanup_containers(leaf_parent_path.to_vec());

        if let Some(path) = self.find_window(&window_id) {
            self.focus_path = path;
        }

        true
    }

    fn move_leaf_into_container(
        &mut self,
        leaf_parent_path: &[usize],
        leaf_idx: usize,
        target_key: NodeKey,
        direction: Direction,
        target_focus_idx: usize,
    ) -> bool {
        let leaf_key = match self.get_node_key_at_path(&self.focus_path) {
            Some(key) => key,
            None => return false,
        };
        let window_id = match self.get_node(leaf_key) {
            Some(NodeData::Leaf(tile)) => tile.window().id().clone(),
            _ => return false,
        };

        let leaf_parent_key = if leaf_parent_path.is_empty() {
            match self.root {
                Some(key) => key,
                None => return false,
            }
        } else {
            match self.get_node_key_at_path(leaf_parent_path) {
                Some(key) => key,
                None => return false,
            }
        };

        if let Some(container) = self.get_container_mut(leaf_parent_key) {
            let _ = container.remove_child(leaf_idx);
        } else {
            return false;
        }

        let insert_idx = match direction {
            Direction::Left | Direction::Up => target_focus_idx,
            Direction::Right | Direction::Down => target_focus_idx + 1,
        };

        if let Some(container) = self.get_container_mut(target_key) {
            let idx = insert_idx.min(container.child_count());
            container.insert_child(idx, leaf_key);
            container.set_focused_idx(idx);
        } else {
            return false;
        }

        self.cleanup_containers(leaf_parent_path.to_vec());

        if let Some(path) = self.find_window(&window_id) {
            self.focus_path = path;
        }

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
        self.debug_tree_node(root_key, &mut path, &mut out);
        out
    }

    #[cfg(test)]
    fn debug_tree_node(
        &self,
        node_key: NodeKey,
        path: &mut Vec<usize>,
        out: &mut String,
    ) where
        W::Id: std::fmt::Display,
    {
        use std::fmt::Write as _;

        let indent = "  ".repeat(path.len());
        match self.get_node(node_key) {
            Some(NodeData::Leaf(tile)) => {
                let focused = if *path == self.focus_path { " *" } else { "" };
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
                    self.debug_tree_node(*child_key, path, out);
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
        let focused_key = self.get_node_key_at_path(&self.focus_path);
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

fn normalize_percents(percents: &mut Vec<f64>, count: usize) {
    if count == 0 {
        percents.clear();
        return;
    }

    if percents.len() != count {
        let value = 1.0 / count as f64;
        percents.clear();
        percents.resize(count, value);
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
        let value = 1.0 / count as f64;
        percents.clear();
        percents.resize(count, value);
        return;
    }

    for percent in percents.iter_mut() {
        *percent /= sum;
    }
}
