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
        } else {
            // TODO: Implement proper insertion based on focus and split direction
            // For now, create a horizontal split at root
            let old_root = self.root.take().unwrap();
            let mut container = Container::new(Layout::SplitH);
            container.add_child(old_root);
            container.add_child(Node::leaf(tile));
            self.root = Some(Node::Container(container));
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
        self.root.as_ref().and_then(|root| {
            Self::focused_window_in_node(root)
        })
    }

    /// Helper: get focused window from a node
    fn focused_window_in_node(node: &Node<W>) -> Option<&W> {
        match node {
            Node::Leaf(tile) => Some(tile.window()),
            Node::Container(container) => {
                container.focused_child()
                    .and_then(|child| Self::focused_window_in_node(child))
            }
        }
    }

    /// Get the currently focused window (mutable)
    pub fn focused_window_mut(&mut self) -> Option<&mut W> {
        self.root.as_mut().and_then(|root| {
            Self::focused_window_in_node_mut(root)
        })
    }

    /// Helper: get focused window from a node (mutable)
    fn focused_window_in_node_mut(node: &mut Node<W>) -> Option<&mut W> {
        match node {
            Node::Leaf(tile) => Some(tile.window_mut()),
            Node::Container(container) => {
                let idx = container.focused_idx;
                container.children.get_mut(idx)
                    .and_then(|child| Self::focused_window_in_node_mut(child))
            }
        }
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

        // TODO: Implement proper directional focus
        // For now, simple implementation within containers
        false
    }

    /// Move window in a direction
    pub fn move_in_direction(&mut self, direction: Direction) -> bool {
        if self.root.is_none() {
            return false;
        }

        // TODO: Implement proper window movement
        false
    }

    /// Split the focused container in a direction
    pub fn split_focused(&mut self, layout: Layout) -> bool {
        if self.root.is_none() {
            return false;
        }

        // TODO: Implement container splitting
        // This should wrap the focused leaf in a new container with the given layout
        false
    }

    /// Change layout of focused container
    pub fn set_focused_layout(&mut self, layout: Layout) -> bool {
        if let Some(root) = &mut self.root {
            // Find focused container and change its layout
            // TODO: Implement proper focus tracking to parent containers
            if let Some(container) = root.as_container_mut() {
                container.set_layout(layout);
                return true;
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
