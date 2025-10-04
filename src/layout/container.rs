//! i3-style container tree implementation
//!
//! This module implements the hierarchical container system used by i3wm.
//! Containers form a tree where:
//! - Leaf nodes contain windows (wrapped in Tiles)
//! - Internal nodes contain child containers with a specific layout
//! - Each container can have layouts: SplitH, SplitV, Tabbed, or Stacked

use std::rc::Rc;

use smithay::utils::{Logical, Point, Rectangle, Size};

use super::tile::Tile;
use super::{LayoutElement, Options};

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
pub enum Node<W: LayoutElement> {
    /// Container node with children
    Container(Container<W>),
    /// Leaf node containing a window
    Leaf(Tile<W>),
}

/// Cached layout information for a leaf tile.
#[derive(Debug, Clone)]
pub struct LeafLayoutInfo {
    pub path: Vec<usize>,
    pub rect: Rectangle<f64, Logical>,
    pub visible: bool,
}

/// Container in the tree hierarchy
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

/// Root container tree for a workspace
#[derive(Debug)]
pub struct ContainerTree<W: LayoutElement> {
    /// Root node of the tree
    root: Option<Node<W>>,
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
// Container Implementation
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

    /// Number of children.
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
        self.child_percents.push(0.0);
        self.recalculate_percentages();
    }

    /// Remove a child at index, returns the removed node
    pub fn remove_child(&mut self, idx: usize) -> Option<Node<W>> {
        if idx >= self.children.len() {
            return None;
        }

        let node = self.children.remove(idx);
        let _ = self.child_percents.remove(idx);

        // Adjust focused index if needed
        if self.focused_idx >= self.children.len() && self.focused_idx > 0 {
            self.focused_idx = self.children.len() - 1;
        }

        // Recalculate percentages
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
        options: Rc<Options>,
    ) -> Self {
        Self {
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
        self.clear_focus_history();
        self.root = Some(node);
    }

    /// Take the root node, leaving None
    pub fn take_root(&mut self) -> Option<Node<W>> {
        self.clear_focus_history();
        self.root.take()
    }

    /// Insert a window into the tree
    /// For now, simple implementation: if empty, create root leaf
    /// Otherwise, we'll implement proper insertion logic later
    pub fn insert_window(&mut self, tile: Tile<W>) {
        self.clear_focus_history();
        if self.root.is_none() {
            // First window becomes the root leaf.
            self.root = Some(Node::leaf(tile));
            self.focus_path.clear();
            return;
        }

        // Ensure the root is a container so we can insert siblings easily.
        let focus_path = if self.root.as_ref().is_some_and(Node::is_leaf) {
            // Convert the root leaf into a container that keeps the existing tile as the
            // focused child. Future insertions can now target the same parent container.
            let old_root = self.root.take().unwrap();
            let mut container = Container::new(Layout::SplitH);
            container.add_child(old_root);
            container.set_focused_idx(0);
            self.focus_path = vec![0];
            self.root = Some(Node::Container(container));
            self.focus_path.clone()
        } else {
            self.focus_path.clone()
        };

        // If we somehow lost focus, fall back to appending at root.
        if focus_path.is_empty() {
            if let Some(Node::Container(container)) = self.root.as_mut() {
                let insert_idx = container.children.len();
                container.insert_child(insert_idx, Node::leaf(tile));
                container.set_focused_idx(insert_idx);
                self.focus_path = vec![insert_idx];
            } else {
                // Root is guaranteed to be a container due to the conversion above.
                self.root = Some(Node::leaf(tile));
                self.focus_path.clear();
            }
            return;
        }

        let parent_path = &focus_path[..focus_path.len() - 1];
        let current_idx = *focus_path.last().unwrap();

        // Insert as a sibling in the parent container whenever possible.
        if let Some(parent_container) = self.get_container_at_path_mut(parent_path) {
            let insert_idx = current_idx + 1;
            parent_container.insert_child(insert_idx, Node::leaf(tile));
            parent_container.set_focused_idx(insert_idx);

            self.focus_path.truncate(parent_path.len());
            self.focus_path.push(insert_idx);
            self.focus_to_first_leaf_from_path();
            return;
        }

        // If the focused node wasn't inside a container (shouldn't normally happen), append to
        // the root container as a sensible fallback.
        if let Some(Node::Container(container)) = self.root.as_mut() {
            let insert_idx = container.children.len();
            container.insert_child(insert_idx, Node::leaf(tile));
            container.set_focused_idx(insert_idx);
            self.focus_path = vec![insert_idx];
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

    /// Number of root-level children (columns).
    pub fn root_children_len(&self) -> usize {
        match &self.root {
            None => 0,
            Some(Node::Leaf(_)) => 1,
            Some(Node::Container(container)) => container.children.len(),
        }
    }

    pub fn root_container(&self) -> Option<&Container<W>> {
        match &self.root {
            Some(Node::Container(container)) => Some(container),
            _ => None,
        }
    }

    pub fn root_container_mut(&mut self) -> Option<&mut Container<W>> {
        match self.root.as_mut() {
            Some(Node::Container(container)) => Some(container),
            _ => None,
        }
    }

    /// Current percent of a root child relative to the root container, if any.
    pub fn root_child_percent(&self, idx: usize) -> Option<f64> {
        match &self.root {
            Some(Node::Container(container)) => {
                if idx >= container.children.len() {
                    None
                } else {
                    Some(container.child_percent(idx))
                }
            }
            Some(Node::Leaf(_)) => {
                if idx == 0 {
                    Some(1.0)
                } else {
                    None
                }
            }
            None => None,
        }
    }

    /// Set the percent of a root child. Returns false if the root isn't a container or index invalid.
    pub fn set_root_child_percent(&mut self, idx: usize, percent: f64) -> bool {
        match self.root.as_mut() {
            Some(Node::Container(container)) => {
                if idx >= container.children.len() {
                    return false;
                }
                container.set_child_percent(idx, percent);
                true
            }
            _ => false,
        }
    }

    pub fn child_percent_at(&self, parent_path: &[usize], child_idx: usize) -> Option<f64> {
        let container = if parent_path.is_empty() {
            self.root.as_ref()?.as_container()
        } else {
            self.get_container_at_path(parent_path)
        }?;

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
        let Some(container) = self.container_at_path_mut(parent_path) else {
            return false;
        };

        if container.layout() != layout || child_idx >= container.child_count() {
            return false;
        }

        container.set_child_percent(child_idx, percent);
        true
    }

    pub fn container_info(
        &self,
        path: &[usize],
    ) -> Option<(Layout, Rectangle<f64, Logical>, usize)> {
        let container = if path.is_empty() {
            self.root.as_ref()?.as_container()
        } else {
            self.get_container_at_path(path)
        }?;

        Some((container.layout(), container.geometry(), container.child_count()))
    }

    pub fn container_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Container<W>> {
        if path.is_empty() {
            match self.root.as_mut()? {
                Node::Container(container) => Some(container),
                _ => None,
            }
        } else {
            self.get_container_at_path_mut(path)
        }
    }

    pub fn find_parent_with_layout(
        &self,
        mut path: Vec<usize>,
        layout: Layout,
    ) -> Option<(Vec<usize>, usize)> {
        while !path.is_empty() {
            let child_idx = *path.last().unwrap();
            let parent_path_vec = path[..path.len() - 1].to_vec();

            let container = if parent_path_vec.is_empty() {
                match &self.root {
                    Some(Node::Container(container)) => Some(container),
                    _ => None,
                }
            } else {
                self.get_container_at_path(&parent_path_vec)
            }?;

            if container.layout() == layout {
                return Some((parent_path_vec, child_idx));
            }

            path.pop();
        }

        None
    }

    /// Index of currently focused root child, if any.
    pub fn focused_root_index(&self) -> Option<usize> {
        match &self.root {
            None => None,
            Some(Node::Leaf(_)) => Some(0),
            Some(Node::Container(container)) => {
                if self.focus_path.is_empty() {
                    Some(container.focused_idx.min(container.children.len().saturating_sub(1)))
                } else {
                    Some(self.focus_path[0])
                }
            }
        }
    }

    /// Focus root child at index, descending to the first leaf.
    pub fn focus_root_child(&mut self, idx: usize) -> bool {
        self.clear_focus_history();
        match &self.root {
            None => false,
            Some(Node::Leaf(_)) => {
                if idx == 0 {
                    self.focus_path.clear();
                    true
                } else {
                    false
                }
            }
            Some(Node::Container(container)) => {
                if idx >= container.children.len() {
                    return false;
                }
                self.focus_path = vec![idx];
                self.focus_to_first_leaf_from_path();
                true
            }
        }
    }

    /// Move a root child from one index to another, keeping focus consistent.
    pub fn move_root_child(&mut self, from: usize, to: usize) -> bool {
        self.clear_focus_history();
        let root_container = match self.root.as_mut() {
            Some(Node::Container(container)) => container,
            Some(Node::Leaf(_)) | None => return false,
        };

        if from >= root_container.children.len() || to >= root_container.children.len() {
            return false;
        }

        let node = root_container.children.remove(from);
        let percent = root_container.child_percents.remove(from);
        root_container.children.insert(to, node);
        root_container.child_percents.insert(to, percent);
        root_container.normalize_child_percents();

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
            self.focus_path = vec![root_container.focused_idx.min(
                root_container.children.len().saturating_sub(1),
            )];
        }

        root_container.set_focused_idx(
            self.focus_path
                .get(0)
                .copied()
                .unwrap_or(root_container.focused_idx),
        );

        self.focus_to_first_leaf_from_path();
        true
    }

    /// Remove and return the root-level child at the given index.
    pub fn take_root_child(&mut self, idx: usize) -> Option<Node<W>> {
        match self.root.as_mut()? {
            Node::Leaf(_) => {
                if idx == 0 {
                    self.focus_path.clear();
                    self.root.take()
                } else {
                    None
                }
            }
            Node::Container(container) => {
                if idx >= container.children.len() {
                    return None;
                }
                let node = container.remove_child(idx)?;

                // Drop the borrow before we mutate the tree further.
                let remaining = container.children.len();
                let _ = container;

                // Collapse any redundant root wrappers and keep focus sensible.
                self.cleanup_containers(Vec::new());

                match self.root.as_mut() {
                    None => {
                        self.focus_path.clear();
                    }
                    Some(Node::Leaf(_)) => {
                        self.focus_path.clear();
                    }
                    Some(Node::Container(root_container)) => {
                        if remaining > 0 {
                            let new_idx = idx.min(root_container.children.len().saturating_sub(1));
                            root_container.set_focused_idx(new_idx);
                            self.focus_path = vec![new_idx];
                            self.focus_to_first_leaf_from_path();
                        } else {
                            self.focus_first_leaf();
                        }
                    }
                }

                Some(node)
            }
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
        if let Some(node) = self.get_node_at_path(prefix) {
            Self::collect_leaf_paths_from_node(node, &mut path, &mut results);
        }
        results
    }

    /// Move focus in a direction
    pub fn focus_in_direction(&mut self, direction: Direction) -> bool {
        self.clear_focus_history();
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

        if self.get_node_at_path(&path).is_none() {
            self.focus_parent_stack.clear();
            return false;
        }

        self.focus_path = path;
        self.focus_to_first_leaf_from_path();
        true
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

    /// Helper: get tile at a given path (immutable).
    pub fn tile_at_path(&self, path: &[usize]) -> Option<&Tile<W>> {
        match self.get_node_at_path(path)? {
            Node::Leaf(tile) => Some(tile),
            _ => None,
        }
    }

    /// Helper: get container at path (mutable)
    fn get_container_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Container<W>> {
        if path.is_empty() {
            // Root
            return self.root.as_mut()?.as_container_mut();
        }

        let mut current = self.root.as_mut()?;

        for &idx in &path[..path.len() - 1] {
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

    /// Helper: get container at path (immutable)
    fn get_container_at_path(&self, path: &[usize]) -> Option<&Container<W>> {
        if path.is_empty() {
            return self.root.as_ref()?.as_container();
        }

        let mut current = self.root.as_ref()?;

        for &idx in &path[..path.len() - 1] {
            match current {
                Node::Container(container) => {
                    current = container.children.get(idx)?;
                }
                Node::Leaf(_) => return None,
            }
        }

        match current {
            Node::Container(container) => {
                let last_idx = *path.last()?;
                container.children.get(last_idx)?.as_container()
            }
            Node::Leaf(_) => None,
        }
    }

    /// Helper: get tile at a given path (mutable).
    pub fn tile_at_path_mut(&mut self, path: &[usize]) -> Option<&mut Tile<W>> {
        match self.get_node_at_path_mut(path)? {
            Node::Leaf(tile) => Some(tile),
            _ => None,
        }
    }

    /// Collect raw pointers to tiles (immutable) in depth-first order.
    pub fn tile_ptrs(&self) -> Vec<*const Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(root) = &self.root {
            Self::collect_tile_ptrs(root, &mut tiles);
        }
        tiles
    }

    fn collect_tile_ptrs(node: &Node<W>, out: &mut Vec<*const Tile<W>>) {
        match node {
            Node::Leaf(tile) => out.push(tile as *const _),
            Node::Container(container) => {
                for child in &container.children {
                    Self::collect_tile_ptrs(child, out);
                }
            }
        }
    }

    /// Collect raw pointers to tiles (mutable) in depth-first order.
    pub fn tile_ptrs_mut(&mut self) -> Vec<*mut Tile<W>> {
        let mut tiles = Vec::new();
        if let Some(root) = &mut self.root {
            Self::collect_tile_ptrs_mut(root, &mut tiles);
        }
        tiles
    }

    fn collect_tile_ptrs_mut(node: &mut Node<W>, out: &mut Vec<*mut Tile<W>>) {
        match node {
            Node::Leaf(tile) => out.push(tile as *mut _),
            Node::Container(container) => {
                for child in &mut container.children {
                    Self::collect_tile_ptrs_mut(child, out);
                }
            }
        }
    }

    /// Move window in a direction
    /// Swaps the focused window with its sibling in the given direction
    pub fn move_in_direction(&mut self, direction: Direction) -> bool {
        self.clear_focus_history();
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
                    container.child_percents.swap(current_idx, target_idx);

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
        self.clear_focus_history();
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
                parent.insert_child(child_idx, Node::Container(new_container));

                // Update focus path to point inside new container
                self.focus_path.push(0);
                return true;
            } else {
                // It's already a container, just insert it back
                parent.insert_child(child_idx, focused_child);
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
        let node = self.remove_node_at_path(&path)?;

        let tile = match node {
            Node::Leaf(tile) => tile,
            Node::Container(_) => return None,
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

    pub fn append_leaf(&mut self, tile: Tile<W>, focus: bool) {
        self.insert_node_at_root(self.root_children_len(), Node::Leaf(tile), focus);
    }

    pub fn insert_leaf_at(&mut self, index: usize, tile: Tile<W>, focus: bool) {
        self.insert_node_at_root(index, Node::Leaf(tile), focus);
    }

    pub fn insert_node_at_root(&mut self, index: usize, node: Node<W>, focus: bool) {
        let (insert_idx, adjust_threshold) = {
            let container = self.ensure_root_container();
            let prev_focus_idx = container.focused_idx();
            let idx = index.min(container.children.len());
            container.insert_child(idx, node);

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

        if let Some(parent) = self.get_container_at_path_mut(parent_path) {
            let insert_idx = current_idx + 1;
            let prev_focus_idx = parent.focused_idx();
            parent.insert_child(insert_idx, Node::leaf(tile));

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

        // Fallback: append to root.
        self.append_leaf(tile, focus);
        true
    }

    /// Insert a new leaf into the specified root column.
    pub fn insert_leaf_in_column(
        &mut self,
        column_idx: usize,
        tile_idx: Option<usize>,
        tile: Tile<W>,
        focus: bool,
    ) -> bool {
        let result = {
            let root = self.ensure_root_container();
            if column_idx >= root.children.len() {
                return false;
            }

            if root.children[column_idx].is_leaf() {
                let existing = root.children.remove(column_idx);
                let existing_percent = root.child_percents.remove(column_idx);
                let mut column_container = Container::new(Layout::SplitV);
                column_container.add_child(existing);
                column_container.set_focused_idx(0);
                root.children
                    .insert(column_idx, Node::Container(column_container));
                root.child_percents.insert(column_idx, existing_percent);
                root.normalize_child_percents();
            }

            let column_node = root.children.get_mut(column_idx).expect("column exists");
            let column_container = column_node
                .as_container_mut()
                .expect("column converted to container");

            let insert_at = tile_idx.unwrap_or(column_container.children.len());
            let insert_at = insert_at.min(column_container.children.len());
            column_container.insert_child(insert_at, Node::Leaf(tile));

            let focus_path = if focus {
                column_container.set_focused_idx(insert_at);
                Some(vec![column_idx, insert_at])
            } else {
                None
            };

            root.set_focused_idx(column_idx);

            (insert_at, focus_path)
        };

        let inserted_idx = result.0;
        let new_focus_path = result.1;

        if let Some(path) = new_focus_path {
            self.focus_path = path;
            self.focus_to_first_leaf_from_path();
        } else if self.focus_path.get(0) == Some(&column_idx) {
            if self.focus_path.len() > 1 {
                if let Some(second) = self.focus_path.get_mut(1) {
                    if inserted_idx <= *second {
                        *second += 1;
                    }
                }
            }
        } else if self.focus_path.is_empty() {
            self.focus_path = vec![column_idx];
            self.focus_to_first_leaf_from_path();
        }

        true
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

    fn collect_leaf_paths_from_node(
        node: &Node<W>,
        path: &mut Vec<usize>,
        results: &mut Vec<Vec<usize>>,
    ) {
        match node {
            Node::Leaf(_) => results.push(path.clone()),
            Node::Container(container) => {
                for (idx, child) in container.children.iter().enumerate() {
                    path.push(idx);
                    Self::collect_leaf_paths_from_node(child, path, results);
                    path.pop();
                }
            }
        }
    }

    fn remove_node_at_path(&mut self, path: &[usize]) -> Option<Node<W>> {
        if path.is_empty() {
            return self.root.take();
        }

        let parent_path = &path[..path.len() - 1];
        let idx = *path.last()?;
        let parent = self.get_container_at_path_mut(parent_path)?;
        parent.remove_child(idx)
    }

    fn cleanup_containers(&mut self, mut path: Vec<usize>) {
        loop {
            if path.is_empty() {
                match &mut self.root {
                    Some(Node::Container(container)) => {
                        if container.children.is_empty() {
                            self.root = None;
                        } else if container.children.len() == 1 {
                            let child = container.children.remove(0);
                            let _ = container.child_percents.remove(0);
                            self.root = Some(child);
                        }
                    }
                    _ => {}
                }
                break;
            } else {
                let last_idx = *path.last().unwrap();
                let parent_path = &path[..path.len() - 1];

                let mut remove_container = false;
                let mut replace_with_child = None;

                if let Some(Node::Container(container)) = self.get_node_at_path_mut(&path) {
                    if container.children.is_empty() {
                        remove_container = true;
                    } else if container.children.len() == 1 {
                        let child = container.children.remove(0);
                        let _ = container.child_percents.remove(0);
                        replace_with_child = Some(child);
                    }
                }

                if remove_container {
                    if parent_path.is_empty() {
                        if let Some(Node::Container(parent)) = self.root.as_mut() {
                            parent.remove_child(last_idx);
                        }
                    } else if let Some(Node::Container(parent)) =
                        self.get_node_at_path_mut(parent_path)
                    {
                        parent.remove_child(last_idx);
                    }
                } else if let Some(child) = replace_with_child {
                    if parent_path.is_empty() {
                        if let Some(Node::Container(parent)) = self.root.as_mut() {
                            parent.children[last_idx] = child;
                        }
                    } else if let Some(Node::Container(parent)) =
                        self.get_node_at_path_mut(parent_path)
                    {
                        parent.children[last_idx] = child;
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
        let mut current = match &self.root {
            Some(node) => node,
            None => {
                self.focus_path.clear();
                return;
            }
        };

        loop {
            match current {
                Node::Leaf(_) => {
                    self.focus_path = path;
                    return;
                }
                Node::Container(container) => {
                    if container.children.is_empty() {
                        self.focus_path = path;
                        return;
                    }
                    let idx = container
                        .focused_idx
                        .min(container.children.len().saturating_sub(1));
                    path.push(idx);
                    current = &container.children[idx];
                }
            }
        }
    }

    fn ensure_root_container(&mut self) -> &mut Container<W> {
        if self.root.is_none() {
            self.root = Some(Node::Container(Container::new(Layout::SplitH)));
            self.focus_path = Vec::new();
        }

        let needs_conversion = matches!(self.root, Some(Node::Leaf(_)));
        if needs_conversion {
            if let Some(Node::Leaf(tile)) = self.root.take() {
                let mut container = Container::new(Layout::SplitH);
                container.add_child(Node::Leaf(tile));
                container.set_focused_idx(0);
                self.focus_path = vec![0];
                self.root = Some(Node::Container(container));
            }
        }

        match self.root {
            Some(Node::Container(ref mut container)) => container,
            _ => unreachable!(),
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
        self.leaf_layouts.clear();

        if let Some(root) = &mut self.root {
            let mut path = Vec::new();
            let mut area = self.working_area;
            let gap = self.options.layout.gaps;
            if gap > 0.0 {
                area.loc.x += gap;
                area.loc.y += gap;
                area.size.w = (area.size.w - gap * 2.0).max(0.0);
                area.size.h = (area.size.h - gap * 2.0).max(0.0);
            }
            Self::layout_node(root, area, &self.options, &mut path, true, &mut self.leaf_layouts);
        }
    }

    /// Access the cached leaf layout information from the last layout pass.
    pub fn leaf_layouts(&self) -> &[LeafLayoutInfo] {
        &self.leaf_layouts
    }

    /// Clone of the cached leaf layout information. Useful before mutating the tree while
    /// iterating over the layouts.
    pub fn leaf_layouts_cloned(&self) -> Vec<LeafLayoutInfo> {
        self.leaf_layouts.clone()
    }

    /// Current focus path within the tree.
    pub fn focus_path(&self) -> &[usize] {
        &self.focus_path
    }

    fn clear_focus_history(&mut self) {
        self.focus_parent_stack.clear();
    }

    /// Focused tile (if any).
    pub fn focused_tile(&self) -> Option<&Tile<W>> {
        self.tile_at_path(self.focus_path())
    }

    /// Focused tile (mutable) if any.
    pub fn focused_tile_mut(&mut self) -> Option<&mut Tile<W>> {
        let path = self.focus_path.clone();
        self.tile_at_path_mut(&path)
    }

    /// Layout of the container that currently owns the focused leaf (if any).
    pub fn focused_layout(&self) -> Option<Layout> {
        if self.focus_path.is_empty() {
            match &self.root {
                Some(Node::Container(container)) => Some(container.layout()),
                _ => None,
            }
        } else {
            let parent_path = &self.focus_path[..self.focus_path.len() - 1];
            self.get_container_at_path(parent_path)
                .map(|container| container.layout())
        }
    }

    /// Helper: recursively layout a node
    fn layout_node(
        node: &mut Node<W>,
        rect: Rectangle<f64, Logical>,
        options: &Options,
        path: &mut Vec<usize>,
        visible: bool,
        out: &mut Vec<LeafLayoutInfo>,
    ) {
        match node {
            Node::Leaf(tile) => {
                // Set tile size to fill allocated rectangle
                // TODO: Apply gaps from options
                let size = Size::from((rect.size.w, rect.size.h));
                tile.request_tile_size(size, false, None);
                out.push(LeafLayoutInfo {
                    path: path.clone(),
                    rect,
                    visible,
                });
            }
            Node::Container(container) => {
                container.set_geometry(rect);

                if container.children.is_empty() {
                    return;
                }

                match container.layout {
                    Layout::SplitH => {
                        // Horizontal split: divide width among children according to percents
                        let child_count = container.children.len();
                        if child_count == 0 {
                            return;
                        }
                        let gap = options.layout.gaps;
                        let total_gap = if child_count > 1 {
                            gap * (child_count as f64 - 1.0)
                        } else {
                            0.0
                        };
                        let available_width = (rect.size.w - total_gap).max(0.0);
                        let total_percent: f64 = container.child_percents.iter().copied().sum();
                        let percents: Vec<f64> = if total_percent > f64::EPSILON {
                            container
                                .child_percents
                                .iter()
                                .map(|p| p / total_percent)
                                .collect()
                        } else {
                            vec![1.0 / child_count as f64; child_count]
                        };
                        let mut cursor_x = rect.loc.x;
                        let mut used_width = 0.0;

                        for (idx, child) in container.children.iter_mut().enumerate() {
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
                            Self::layout_node(child, child_rect, options, path, visible, out);
                            path.pop();
                            used_width += width;
                            if idx + 1 < child_count {
                                cursor_x += width + gap;
                            }
                        }
                    }
                    Layout::SplitV => {
                        // Vertical split: divide height among children according to percents
                        let child_count = container.children.len();
                        if child_count == 0 {
                            return;
                        }
                        let gap = options.layout.gaps;
                        let total_gap = if child_count > 1 {
                            gap * (child_count as f64 - 1.0)
                        } else {
                            0.0
                        };
                        let available_height = (rect.size.h - total_gap).max(0.0);
                        let total_percent: f64 = container.child_percents.iter().copied().sum();
                        let percents: Vec<f64> = if total_percent > f64::EPSILON {
                            container
                                .child_percents
                                .iter()
                                .map(|p| p / total_percent)
                                .collect()
                        } else {
                            vec![1.0 / child_count as f64; child_count]
                        };
                        let mut cursor_y = rect.loc.y;
                        let mut used_height = 0.0;

                        for (idx, child) in container.children.iter_mut().enumerate() {
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
                            Self::layout_node(child, child_rect, options, path, visible, out);
                            path.pop();
                            used_height += height;
                            if idx + 1 < child_count {
                                cursor_y += height + gap;
                            }
                        }
                    }
                    Layout::Tabbed | Layout::Stacked => {
                        // For tabbed/stacked, all children get full size
                        // Only the focused child is actually visible
                        // TODO: Reserve space for tab bar / title bars
                        let gap = options.layout.gaps;
                        let mut child_rect = rect;
                        if gap > 0.0 {
                            child_rect.loc.x += gap;
                            child_rect.loc.y += gap;
                            child_rect.size.w = (child_rect.size.w - gap * 2.0).max(0.0);
                            child_rect.size.h = (child_rect.size.h - gap * 2.0).max(0.0);
                        }
                        let focused_idx = container
                            .focused_idx
                            .min(container.children.len().saturating_sub(1));

                        for (idx, child) in container.children.iter_mut().enumerate() {
                            path.push(idx);
                            let child_visible = visible && idx == focused_idx;
                            Self::layout_node(child, child_rect, options, path, child_visible, out);
                            path.pop();
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
