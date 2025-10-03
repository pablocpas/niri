//! i3-style container tree implementation
//!
//! This module implements the hierarchical container system used by i3wm.
//! Containers form a tree where:
//! - Leaf nodes contain windows (wrapped in Tiles)
//! - Internal nodes contain child containers with a specific layout
//! - Each container can have layouts: SplitH, SplitV, Tabbed, or Stacked

use std::rc::Rc;

use smithay::utils::{Logical, Point, Rectangle, Scale, Size};

use super::tile::Tile;
use super::{LayoutElement, Options};
use crate::animation::Clock;

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

/// Node type in the container tree
#[derive(Debug)]
pub enum Node<W: LayoutElement> {
    /// Container node with children
    Container(Container<W>),
    /// Leaf node containing a window
    Leaf(Tile<W>),
}

/// Container in the tree hierarchy
#[derive(Debug)]
pub struct Container<W: LayoutElement> {
    /// Layout mode for this container
    layout: Layout,
    /// Child nodes (containers or leaves)
    children: Vec<Node<W>>,
    /// Index of focused child
    focused_idx: usize,
    /// Percentage of parent space this container occupies (0.0-1.0)
    percent: f64,
    /// Cached geometry for rendering
    geometry: Rectangle<f64, Logical>,
}

/// Root container tree for a workspace
#[derive(Debug)]
pub struct ContainerTree<W: LayoutElement> {
    /// Root node of the tree
    root: Option<Node<W>>,
    /// Path to currently focused node (indices from root to leaf)
    focus_path: Vec<usize>,
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

// ============================================================================
// Container Implementation
// ============================================================================

impl<W: LayoutElement> Container<W> {
    /// Create a new container with given layout
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            children: Vec::new(),
            focused_idx: 0,
            percent: 1.0,
            geometry: Rectangle::from_loc_and_size((0.0, 0.0), (0.0, 0.0)),
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

    /// Get children
    pub fn children(&self) -> &[Node<W>] {
        &self.children
    }

    /// Get mutable children
    pub fn children_mut(&mut self) -> &mut Vec<Node<W>> {
        &mut self.children
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

    /// Get focused child node
    pub fn focused_child(&self) -> Option<&Node<W>> {
        self.children.get(self.focused_idx)
    }

    /// Get focused child node (mutable)
    pub fn focused_child_mut(&mut self) -> Option<&mut Node<W>> {
        self.children.get_mut(self.focused_idx)
    }

    /// Add a child node
    pub fn add_child(&mut self, node: Node<W>) {
        self.children.push(node);
        // Recalculate percentages equally
        let count = self.children.len() as f64;
        for child in &mut self.children {
            child.set_percent(1.0 / count);
        }
    }

    /// Remove a child at index, returns the removed node
    pub fn remove_child(&mut self, idx: usize) -> Option<Node<W>> {
        if idx >= self.children.len() {
            return None;
        }

        let node = self.children.remove(idx);

        // Adjust focused index if needed
        if self.focused_idx >= self.children.len() && self.focused_idx > 0 {
            self.focused_idx = self.children.len() - 1;
        }

        // Recalculate percentages
        if !self.children.is_empty() {
            let count = self.children.len() as f64;
            for child in &mut self.children {
                child.set_percent(1.0 / count);
            }
        }

        Some(node)
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
// Node Implementation
// ============================================================================

impl<W: LayoutElement> Node<W> {
    /// Create a container node
    pub fn container(layout: Layout) -> Self {
        Node::Container(Container::new(layout))
    }

    /// Create a leaf node
    pub fn leaf(tile: Tile<W>) -> Self {
        Node::Leaf(tile)
    }

    /// Check if this is a container
    pub fn is_container(&self) -> bool {
        matches!(self, Node::Container(_))
    }

    /// Check if this is a leaf
    pub fn is_leaf(&self) -> bool {
        matches!(self, Node::Leaf(_))
    }

    /// Get as container (if it is one)
    pub fn as_container(&self) -> Option<&Container<W>> {
        match self {
            Node::Container(c) => Some(c),
            _ => None,
        }
    }

    /// Get as container (mutable)
    pub fn as_container_mut(&mut self) -> Option<&mut Container<W>> {
        match self {
            Node::Container(c) => Some(c),
            _ => None,
        }
    }

    /// Get as leaf tile (if it is one)
    pub fn as_leaf(&self) -> Option<&Tile<W>> {
        match self {
            Node::Leaf(t) => Some(t),
            _ => None,
        }
    }

    /// Get as leaf tile (mutable)
    pub fn as_leaf_mut(&mut self) -> Option<&mut Tile<W>> {
        match self {
            Node::Leaf(t) => Some(t),
            _ => None,
        }
    }

    /// Set the percentage of parent space this node occupies
    pub fn set_percent(&mut self, percent: f64) {
        match self {
            Node::Container(c) => c.percent = percent,
            Node::Leaf(_) => {
                // Leaves don't have percentage, it's managed by parent
            }
        }
    }

    /// Get the window from this node (if it's a leaf)
    pub fn window(&self) -> Option<&W> {
        match self {
            Node::Leaf(tile) => Some(tile.window()),
            _ => None,
        }
    }

    /// Get the window from this node (mutable)
    pub fn window_mut(&mut self) -> Option<&mut W> {
        match self {
            Node::Leaf(tile) => Some(tile.window_mut()),
            _ => None,
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
        clock: Clock,
        options: Rc<Options>,
    ) -> Self {
        Self {
            root: None,
            focus_path: Vec::new(),
            view_size,
            working_area,
            scale,
            clock,
            options,
        }
    }

    /// Check if tree is empty
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Get root node
    pub fn root(&self) -> Option<&Node<W>> {
        self.root.as_ref()
    }

    /// Get root node (mutable)
    pub fn root_mut(&mut self) -> Option<&mut Node<W>> {
        self.root.as_mut()
    }

    /// Set root node
    pub fn set_root(&mut self, node: Node<W>) {
        self.root = Some(node);
    }

    /// Take the root node, leaving None
    pub fn take_root(&mut self) -> Option<Node<W>> {
        self.root.take()
    }

    /// Insert a window into the tree
    /// For now, simple implementation: if empty, create root leaf
    /// Otherwise, we'll implement proper insertion logic later
    pub fn insert_window(&mut self, tile: Tile<W>) {
        if self.root.is_none() {
            // First window - create root leaf
            self.root = Some(Node::leaf(tile));
            self.focus_path = vec![];
        } else {
            // TODO: Implement proper insertion based on focus and split direction
            // For now, create a horizontal split at root
            let old_root = self.root.take().unwrap();
            let mut container = Container::new(Layout::SplitH);
            container.add_child(old_root);
            container.add_child(Node::leaf(tile));
            container.set_focused_idx(1); // Focus new window
            self.root = Some(Node::Container(container));
            self.focus_path = vec![1]; // Focus new window
        }
    }

    /// Find a window by ID and return path to it
    pub fn find_window(&self, window_id: &W::Id) -> Option<Vec<usize>> {
        self.root.as_ref().and_then(|root| {
            Self::find_window_in_node(root, window_id, &mut Vec::new())
        })
    }

    /// Helper: recursively find window in node
    fn find_window_in_node(
        node: &Node<W>,
        window_id: &W::Id,
        path: &mut Vec<usize>,
    ) -> Option<Vec<usize>> {
        match node {
            Node::Leaf(tile) => {
                if tile.window().id() == window_id {
                    Some(path.clone())
                } else {
                    None
                }
            }
            Node::Container(container) => {
                for (idx, child) in container.children.iter().enumerate() {
                    path.push(idx);
                    if let Some(result) = Self::find_window_in_node(child, window_id, path) {
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
        let node = self.get_node_at_path(&self.focus_path)?;
        node.window()
    }

    /// Get the currently focused window (mutable)
    pub fn focused_window_mut(&mut self) -> Option<&mut W> {
        let path = self.focus_path.clone();
        self.get_node_at_path_mut(&path)?.window_mut()
    }

    /// Helper: get node at path (mutable)
    fn get_node_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Node<W>> {
        let mut current = self.root.as_mut()?;

        for &idx in path {
            match current {
                Node::Container(container) => {
                    current = container.children.get_mut(idx)?;
                }
                Node::Leaf(_) => return None,
            }
        }

        Some(current)
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

    /// Move focus in a direction
    pub fn focus_in_direction(&mut self, direction: Direction) -> bool {
        if self.root.is_none() {
            return false;
        }

        // Clone focus_path to avoid borrow checker issues
        let focus_path_clone = self.focus_path.clone();

        // Strategy: navigate within the parent container that matches the direction
        // For horizontal directions (Left/Right), look for SplitH parent
        // For vertical directions (Up/Down), look for SplitV parent

        // Navigate up the focus path to find appropriate container
        for depth in (0..focus_path_clone.len()).rev() {
            let parent_path = &focus_path_clone[..depth];
            let current_idx = if depth < focus_path_clone.len() {
                focus_path_clone[depth]
            } else {
                continue;
            };

            if let Some(container) = self.get_container_at_path_mut(parent_path) {
                // Check if this container's layout matches the direction
                let layout_matches = match (container.layout, direction) {
                    (Layout::SplitH, Direction::Left | Direction::Right) => true,
                    (Layout::SplitV, Direction::Up | Direction::Down) => true,
                    (Layout::Tabbed | Layout::Stacked, _) => true, // Can navigate in tabbed/stacked
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
                    container.set_focused_idx(new_idx);

                    // Update focus_path: truncate at depth and add new path to first leaf
                    self.focus_path.truncate(depth);
                    self.focus_path.push(new_idx);
                    self.focus_to_first_leaf_from_path();
                    return true;
                }
            }
        }

        false
    }

    /// Helper: navigate to first leaf from current focus_path
    fn focus_to_first_leaf_from_path(&mut self) {
        let mut current_path = self.focus_path.clone();

        loop {
            if let Some(node) = self.get_node_at_path(&current_path) {
                match node {
                    Node::Leaf(_) => {
                        // Reached a leaf, update focus_path
                        self.focus_path = current_path;
                        return;
                    }
                    Node::Container(container) => {
                        if container.children.is_empty() {
                            return;
                        }
                        // Navigate to focused child
                        current_path.push(container.focused_idx);
                    }
                }
            } else {
                return;
            }
        }
    }

    /// Helper: get node at path
    fn get_node_at_path(&self, path: &[usize]) -> Option<&Node<W>> {
        let mut current = self.root.as_ref()?;

        for &idx in path {
            match current {
                Node::Container(container) => {
                    current = container.children.get(idx)?;
                }
                Node::Leaf(_) => return None,
            }
        }

        Some(current)
    }

    /// Helper: get container at path (mutable)
    fn get_container_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Container<W>> {
        if path.is_empty() {
            // Root
            return self.root.as_mut()?.as_container_mut();
        }

        let mut current = self.root.as_mut()?;

        for &idx in &path[..path.len()-1] {
            match current {
                Node::Container(container) => {
                    current = container.children.get_mut(idx)?;
                }
                Node::Leaf(_) => return None,
            }
        }

        // Get final container
        match current {
            Node::Container(container) => {
                let last_idx = *path.last()?;
                container.children.get_mut(last_idx)?.as_container_mut()
            }
            Node::Leaf(_) => None,
        }
    }

    /// Move window in a direction
    /// Swaps the focused window with its sibling in the given direction
    pub fn move_in_direction(&mut self, direction: Direction) -> bool {
        if self.root.is_none() {
            return false;
        }

        let focus_path = self.focus_path.clone();

        // Strategy: similar to focus navigation, but swap windows instead
        // Navigate up the focus path to find appropriate container
        for depth in (0..focus_path.len()).rev() {
            let parent_path = &focus_path[..depth];
            let current_idx = if depth < focus_path.len() {
                focus_path[depth]
            } else {
                continue;
            };

            if let Some(container) = self.get_container_at_path_mut(parent_path) {
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

                // Calculate target index
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
                    container.children.swap(current_idx, target_idx);

                    // Update focus_path to follow the moved window
                    self.focus_path.truncate(depth);
                    self.focus_path.push(target_idx);
                    self.focus_to_first_leaf_from_path();

                    // Update container's focused_idx
                    if let Some(container) = self.get_container_at_path_mut(parent_path) {
                        container.set_focused_idx(target_idx);
                    }

                    return true;
                }
            }
        }

        false
    }

    /// Split the focused container in a direction
    /// This creates a new container around the focused leaf with the specified layout
    pub fn split_focused(&mut self, layout: Layout) -> bool {
        if self.root.is_none() {
            return false;
        }

        let focus_path = self.focus_path.clone();

        // Special case: if root is a leaf, wrap it in a container
        if focus_path.is_empty() {
            if let Some(Node::Leaf(_)) = &self.root {
                let old_root = self.root.take().unwrap();
                let mut container = Container::new(layout);
                container.add_child(old_root);
                self.root = Some(Node::Container(container));
                self.focus_path = vec![0];
                return true;
            }
        }

        // Find parent and focused child
        if focus_path.is_empty() {
            return false;
        }

        let parent_path = &focus_path[..focus_path.len() - 1];
        let child_idx = *focus_path.last().unwrap();

        // Get parent container
        let parent = if parent_path.is_empty() {
            // Parent is root
            match &mut self.root {
                Some(Node::Container(c)) => c,
                _ => return false,
            }
        } else {
            match self.get_node_at_path_mut(parent_path) {
                Some(Node::Container(c)) => c,
                _ => return false,
            }
        };

        // Remove the focused child
        if let Some(focused_child) = parent.remove_child(child_idx) {
            // Only split if it's a leaf
            if matches!(focused_child, Node::Leaf(_)) {
                // Create new container with the leaf
                let mut new_container = Container::new(layout);
                new_container.add_child(focused_child);

                // Insert new container back at same position
                parent.children.insert(child_idx, Node::Container(new_container));

                // Update focus path to point inside new container
                self.focus_path.push(0);
                return true;
            } else {
                // It's already a container, just insert it back
                parent.children.insert(child_idx, focused_child);
            }
        }

        false
    }

    /// Change layout of focused container
    /// If focused node is a leaf, changes its parent container's layout
    pub fn set_focused_layout(&mut self, layout: Layout) -> bool {
        let focus_path = self.focus_path.clone();

        // If focus is on a leaf, use parent container
        if let Some(node) = self.get_node_at_path(&focus_path) {
            if node.is_leaf() {
                // Get parent container
                if focus_path.is_empty() {
                    return false;
                }

                let parent_path = &focus_path[..focus_path.len() - 1];
                if let Some(container) = self.get_container_at_path_mut(parent_path) {
                    container.set_layout(layout);
                    return true;
                }
            } else {
                // It's already a container, change its layout
                if let Some(container) = self.get_container_at_path_mut(&focus_path) {
                    container.set_layout(layout);
                    return true;
                }
            }
        }

        false
    }

    /// Remove a window by ID, returns the removed tile
    pub fn remove_window(&mut self, window_id: &W::Id) -> Option<Tile<W>> {
        let path = self.find_window(window_id)?;

        // Navigate to parent and remove
        // TODO: Implement proper removal with tree cleanup
        None
    }

    /// Get all windows in the tree (depth-first traversal)
    pub fn all_windows(&self) -> Vec<&W> {
        let mut windows = Vec::new();
        if let Some(root) = &self.root {
            Self::collect_windows_from_node(root, &mut windows);
        }
        windows
    }

    /// Helper: collect all windows from a node
    fn collect_windows_from_node<'a>(node: &'a Node<W>, windows: &mut Vec<&'a W>) {
        match node {
            Node::Leaf(tile) => windows.push(tile.window()),
            Node::Container(container) => {
                for child in &container.children {
                    Self::collect_windows_from_node(child, windows);
                }
            }
        }
    }

    /// Get all tiles in the tree (depth-first traversal)
    pub fn all_tiles(&self) -> Vec<&Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(root) = &self.root {
            Self::collect_tiles_from_node(root, &mut tiles);
        }
        tiles
    }

    /// Helper: collect all tiles from a node
    fn collect_tiles_from_node<'a>(node: &'a Node<W>, tiles: &mut Vec<&'a Tile<W>>) {
        match node {
            Node::Leaf(tile) => tiles.push(tile),
            Node::Container(container) => {
                for child in &container.children {
                    Self::collect_tiles_from_node(child, tiles);
                }
            }
        }
    }

    /// Count total number of windows in tree
    pub fn window_count(&self) -> usize {
        self.root.as_ref().map_or(0, |root| Self::count_windows_in_node(root))
    }

    /// Helper: count windows in a node
    fn count_windows_in_node(node: &Node<W>) -> usize {
        match node {
            Node::Leaf(_) => 1,
            Node::Container(container) => {
                container.children.iter()
                    .map(|child| Self::count_windows_in_node(child))
                    .sum()
            }
        }
    }

    /// Calculate and apply layout to the tree
    /// This computes geometry for all containers and tiles
    pub fn layout(&mut self) {
        if let Some(root) = &mut self.root {
            Self::layout_node(root, self.working_area, &self.options);
        }
    }

    /// Helper: recursively layout a node
    fn layout_node(node: &mut Node<W>, rect: Rectangle<f64, Logical>, options: &Options) {
        match node {
            Node::Leaf(tile) => {
                // Set tile size to fill allocated rectangle
                // TODO: Apply gaps from options
                let size = Size::from((rect.size.w, rect.size.h));
                tile.request_tile_size(size, false, None);
                // Tiles will be updated by workspace
            }
            Node::Container(container) => {
                container.set_geometry(rect);

                if container.children.is_empty() {
                    return;
                }

                match container.layout {
                    Layout::SplitH => {
                        // Horizontal split: divide width among children
                        let child_count = container.children.len() as f64;
                        let child_width = rect.size.w / child_count;

                        for (idx, child) in container.children.iter_mut().enumerate() {
                            let child_rect = Rectangle::from_loc_and_size(
                                (rect.loc.x + (idx as f64 * child_width), rect.loc.y),
                                (child_width, rect.size.h),
                            );
                            Self::layout_node(child, child_rect, options);
                        }
                    }
                    Layout::SplitV => {
                        // Vertical split: divide height among children
                        let child_count = container.children.len() as f64;
                        let child_height = rect.size.h / child_count;

                        for (idx, child) in container.children.iter_mut().enumerate() {
                            let child_rect = Rectangle::from_loc_and_size(
                                (rect.loc.x, rect.loc.y + (idx as f64 * child_height)),
                                (rect.size.w, child_height),
                            );
                            Self::layout_node(child, child_rect, options);
                        }
                    }
                    Layout::Tabbed | Layout::Stacked => {
                        // For tabbed/stacked, all children get full size
                        // Only the focused child is actually visible
                        // TODO: Reserve space for tab bar / title bars
                        for child in container.children.iter_mut() {
                            Self::layout_node(child, rect, options);
                        }
                    }
                }
            }
        }
    }
}

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
