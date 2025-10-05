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

const MIN_CHILD_PERCENT: f64 = 0.05;

/// Node type in the container tree
#[derive(Debug)]
pub enum NodeData<W: LayoutElement> {
    /// Container node with children (stored as keys)
    Container(ContainerData),
    /// Leaf node containing a tile
    Leaf(Tile<W>),
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
    /// Percentage of parent space this container occupies (0.0-1.0)
    percent: f64,
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
// Legacy Node/Container types for API compatibility
// ============================================================================

/// Legacy wrapper for API compatibility
#[derive(Debug)]
pub enum Node<W: LayoutElement> {
    /// Container node with children
    Container(Container<W>),
    /// Leaf node containing a window
    Leaf(Tile<W>),
}

/// Legacy container wrapper
#[derive(Debug)]
pub struct Container<W: LayoutElement> {
    /// Layout mode for this container
    layout: Layout,
    /// Child nodes (containers or leaves)
    children: Vec<Node<W>>,
    /// Relative sizes of children (sum normalized to 1.0 for split layouts)
    child_percents: Vec<f64>,
    /// Index of focused child
    focused_idx: usize,
    /// Percentage of parent space this container occupies (0.0-1.0)
    percent: f64,
    /// Cached geometry for rendering
    geometry: Rectangle<f64, Logical>,
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
            percent: 1.0,
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
        self.children.push(node_key);
        self.child_percents.push(0.0);
        self.recalculate_percentages();
    }

    /// Remove a child at index, returns the removed node key
    pub fn remove_child(&mut self, idx: usize) -> Option<NodeKey> {
        if idx >= self.children.len() {
            return None;
        }

        let key = self.children.remove(idx);
        let _ = self.child_percents.remove(idx);

        // Adjust focused index if needed
        if self.focused_idx >= self.children.len() && self.focused_idx > 0 {
            self.focused_idx = self.children.len() - 1;
        }

        // Recalculate percentages
        if !self.children.is_empty() {
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
        self.children.insert(idx, node_key);
        self.child_percents.insert(idx, 0.0);
        self.recalculate_percentages();
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
        let sum: f64 = self.child_percents.iter().copied().sum();
        if sum <= f64::EPSILON {
            self.recalculate_percentages();
        } else {
            for percent in &mut self.child_percents {
                *percent /= sum;
            }
        }
    }

    pub fn child_percent(&self, idx: usize) -> f64 {
        self.child_percents.get(idx).copied().unwrap_or(0.0)
    }

    pub fn set_child_percent(&mut self, idx: usize, percent: f64) {
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

    /// Get root node (legacy API - stub)
    pub fn root(&self) -> Option<&Node<W>> {
        // Can't return Node from NodeData without cloning entire tree
        None
    }

    /// Get root node (mutable) - legacy API stub
    pub fn root_mut(&mut self) -> Option<&mut Node<W>> {
        // Can't return Node from NodeData without cloning entire tree
        None
    }

    /// Set root node (legacy API - converts to NodeData)
    pub fn set_root(&mut self, node: Node<W>) {
        self.clear_focus_history();
        let key = self.node_to_slotmap(node);
        self.root = Some(key);
    }

    /// Take the root node, leaving None
    pub fn take_root(&mut self) -> Option<Node<W>> {
        self.clear_focus_history();
        let key = self.root.take()?;
        let node_data = self.remove_node_recursive(key)?;
        Some(self.slotmap_to_node(node_data))
    }

    /// Convert legacy Node to slotmap representation
    fn node_to_slotmap(&mut self, node: Node<W>) -> NodeKey {
        match node {
            Node::Container(container) => {
                let mut child_keys = Vec::new();
                for child in container.children {
                    let child_key = self.node_to_slotmap(child);
                    child_keys.push(child_key);
                }
                let container_data = NodeData::Container(ContainerData {
                    layout: container.layout,
                    children: child_keys,
                    child_percents: container.child_percents,
                    focused_idx: container.focused_idx,
                    percent: container.percent,
                    geometry: container.geometry,
                });
                self.insert_node(container_data)
            }
            Node::Leaf(tile) => {
                self.insert_node(NodeData::Leaf(tile))
            }
        }
    }

    /// Convert slotmap NodeData to legacy Node
    fn slotmap_to_node(&self, node_data: NodeData<W>) -> Node<W> {
        match node_data {
            NodeData::Container(container_data) => {
                let mut children = Vec::new();
                for &child_key in &container_data.children {
                    if let Some(child_data) = self.get_node(child_key) {
                        // We need to clone child_data here since we can't move it
                        // This is expensive but necessary for the legacy API
                        // TODO: Consider removing legacy API entirely
                    }
                }
                Node::Container(Container {
                    layout: container_data.layout,
                    children,
                    child_percents: container_data.child_percents,
                    focused_idx: container_data.focused_idx,
                    percent: container_data.percent,
                    geometry: container_data.geometry,
                })
            }
            NodeData::Leaf(tile) => Node::Leaf(tile),
        }
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
        self.root.map_or(0, |root_key| self.count_windows_in_node(root_key))
    }

    /// Helper: count windows in a node
    fn count_windows_in_node(&self, node_key: NodeKey) -> usize {
        match self.get_node(node_key) {
            Some(NodeData::Leaf(_)) => 1,
            Some(NodeData::Container(container)) => {
                container.children.iter()
                    .map(|&child_key| self.count_windows_in_node(child_key))
                    .sum()
            }
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
                (container.layout, container.children.clone(), container.child_percents.clone(), container.focused_idx)
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
                let total_gap = if child_count > 1 { gap * (child_count as f64 - 1.0) } else { 0.0 };
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
                let total_gap = if child_count > 1 { gap * (child_count as f64 - 1.0) } else { 0.0 };
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
                // All children get full size, only focused is visible
                let mut child_rect = rect;
                if gap > 0.0 {
                    child_rect.loc.x += gap;
                    child_rect.loc.y += gap;
                    child_rect.size.w = (child_rect.size.w - gap * 2.0).max(0.0);
                    child_rect.size.h = (child_rect.size.h - gap * 2.0).max(0.0);
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

        let node_data = self.remove_node_recursive(node_key)?;
        let tile = match node_data {
            NodeData::Leaf(tile) => tile,
            NodeData::Container(_) => return None,
        };

        // Remove from parent's children list
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
                    let layout_matches = match (container.layout, direction) {
                        (Layout::SplitH, Direction::Left | Direction::Right) => true,
                        (Layout::SplitV, Direction::Up | Direction::Down) => true,
                        (Layout::Tabbed | Layout::Stacked, _) => true,
                        _ => false,
                    };

                    if !layout_matches {
                        continue;
                    }

                    let child_count = container.children.len();
                    let target_idx = match direction {
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

                    if let Some(target_idx) = target_idx {
                        // Swap children
                        if let Some(container) = self.get_container_mut(parent_key) {
                            container.children.swap(current_idx, target_idx);
                            container.child_percents.swap(current_idx, target_idx);
                            container.set_focused_idx(target_idx);
                        }

                        self.focus_path.truncate(depth);
                        self.focus_path.push(target_idx);
                        self.focus_to_first_leaf_from_path();

                        return true;
                    }
                }
            }
        }

        false
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
        Some((container.layout(), container.geometry(), container.child_count()))
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
                    Some(container.focused_idx.min(container.children.len().saturating_sub(1)))
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
                self.focus_path = vec![container.focused_idx.min(
                    container.children.len().saturating_sub(1),
                )];
            }
        }

        let default_idx = self.focus_path.get(0).copied();
        if let Some(container) = self.get_container_mut(root_key) {
            container.set_focused_idx(
                default_idx.unwrap_or(container.focused_idx),
            );
        }

        self.focus_to_first_leaf_from_path();
        true
    }

    /// Remove and return the root-level child at the given index.
    pub fn take_root_child(&mut self, idx: usize) -> Option<Node<W>> {
        let root_key = self.root?;

        match self.get_node(root_key) {
            Some(NodeData::Leaf(_)) => {
                if idx == 0 {
                    self.focus_path.clear();
                    let node_data = self.remove_node_recursive(root_key)?;
                    self.root = None;
                    Some(self.slotmap_to_node(node_data))
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

                // Remove from container
                if let Some(container) = self.get_container_mut(root_key) {
                    container.remove_child(idx);
                }

                let remaining = self.get_container(root_key)?.children.len();

                // Cleanup containers
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

                let node_data = self.remove_node_recursive(child_key)?;
                Some(self.slotmap_to_node(node_data))
            }
            None => None,
        }
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

    pub fn append_leaf(&mut self, tile: Tile<W>, focus: bool) {
        self.insert_node_at_root(self.root_children_len(), Node::Leaf(tile), focus);
    }

    pub fn insert_leaf_at(&mut self, index: usize, tile: Tile<W>, focus: bool) {
        self.insert_node_at_root(index, Node::Leaf(tile), focus);
    }

    pub fn insert_node_at_root(&mut self, index: usize, node: Node<W>, focus: bool) {
        let node_key = self.node_to_slotmap(node);

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

    pub fn append_node_at_root(&mut self, node: Node<W>, focus: bool) {
        let len = self.root_children_len();
        self.insert_node_at_root(len, node, focus);
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
            let (existing_key, existing_percent) = if let Some(container) = self.get_container_mut(root_key) {
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
                container.child_percents.insert(column_idx, existing_percent);
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
                        } else if container.children.len() == 1 {
                            let child_key = container.child_key(0).unwrap();
                            self.root = Some(child_key);
                            self.remove_node_recursive(root_key);
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

                        if let Some(container) = self.get_container(container_key) {
                            if container.children.is_empty() {
                                remove_container = true;
                            } else if container.children.len() == 1 {
                                replace_with_child = container.child_key(0);
                            }
                        }

                        if remove_container {
                            if let Some(parent) = self.get_container_mut(parent_key) {
                                parent.remove_child(last_idx);
                            }
                            self.remove_node_recursive(container_key);
                        } else if let Some(child_key) = replace_with_child {
                            self.remove_node_recursive(container_key);
                            if let Some(parent) = self.get_container_mut(parent_key) {
                                parent.children[last_idx] = child_key;
                            }
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

// ============================================================================
// Legacy Node/Container Implementation (for compatibility)
// ============================================================================

impl<W: LayoutElement> Container<W> {
    /// Create a new container with given layout
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            children: Vec::new(),
            child_percents: Vec::new(),
            focused_idx: 0,
            percent: 1.0,
            geometry: Rectangle::from_size(Size::from((0.0, 0.0))),
        }
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
    }

    pub fn children(&self) -> &[Node<W>] {
        &self.children
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    pub fn focused_idx(&self) -> usize {
        self.focused_idx
    }

    pub fn set_focused_idx(&mut self, idx: usize) {
        if idx < self.children.len() {
            self.focused_idx = idx;
        }
    }

    pub fn focused_child(&self) -> Option<&Node<W>> {
        self.children.get(self.focused_idx)
    }

    pub fn focused_child_mut(&mut self) -> Option<&mut Node<W>> {
        self.children.get_mut(self.focused_idx)
    }

    pub fn add_child(&mut self, node: Node<W>) {
        self.children.push(node);
        self.child_percents.push(0.0);
        self.recalculate_percentages();
    }

    pub fn remove_child(&mut self, idx: usize) -> Option<Node<W>> {
        if idx >= self.children.len() {
            return None;
        }

        let node = self.children.remove(idx);
        let _ = self.child_percents.remove(idx);

        if self.focused_idx >= self.children.len() && self.focused_idx > 0 {
            self.focused_idx = self.children.len() - 1;
        }

        if !self.children.is_empty() {
            self.recalculate_percentages();
        }

        Some(node)
    }

    pub fn insert_child(&mut self, idx: usize, node: Node<W>) {
        let idx = idx.min(self.children.len());
        self.children.insert(idx, node);
        self.child_percents.insert(idx, 0.0);
        self.recalculate_percentages();
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
        let sum: f64 = self.child_percents.iter().copied().sum();
        if sum <= f64::EPSILON {
            self.recalculate_percentages();
        } else {
            for percent in &mut self.child_percents {
                *percent /= sum;
            }
        }
    }

    pub fn child_percent(&self, idx: usize) -> f64 {
        self.child_percents.get(idx).copied().unwrap_or(0.0)
    }

    pub fn set_child_percent(&mut self, idx: usize, percent: f64) {
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

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    pub fn len(&self) -> usize {
        self.children.len()
    }

    pub fn set_geometry(&mut self, geometry: Rectangle<f64, Logical>) {
        self.geometry = geometry;
    }

    pub fn geometry(&self) -> Rectangle<f64, Logical> {
        self.geometry
    }
}

impl<W: LayoutElement> Node<W> {
    pub fn container(layout: Layout) -> Self {
        Node::Container(Container::new(layout))
    }

    pub fn leaf(tile: Tile<W>) -> Self {
        Node::Leaf(tile)
    }

    pub fn is_container(&self) -> bool {
        matches!(self, Node::Container(_))
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, Node::Leaf(_))
    }

    pub fn as_container(&self) -> Option<&Container<W>> {
        match self {
            Node::Container(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_container_mut(&mut self) -> Option<&mut Container<W>> {
        match self {
            Node::Container(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_leaf(&self) -> Option<&Tile<W>> {
        match self {
            Node::Leaf(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_leaf_mut(&mut self) -> Option<&mut Tile<W>> {
        match self {
            Node::Leaf(t) => Some(t),
            _ => None,
        }
    }

    pub fn set_percent(&mut self, percent: f64) {
        match self {
            Node::Container(c) => c.percent = percent,
            Node::Leaf(_) => {}
        }
    }

    pub fn window(&self) -> Option<&W> {
        match self {
            Node::Leaf(tile) => Some(tile.window()),
            _ => None,
        }
    }

    pub fn window_mut(&mut self) -> Option<&mut W> {
        match self {
            Node::Leaf(tile) => Some(tile.window_mut()),
            _ => None,
        }
    }
}
