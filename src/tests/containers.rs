use std::collections::BTreeSet;

use crate::layout::ContainerLayout;
use client::ClientId;
use tiri_ipc::{LayoutTreeLayout, LayoutTreeNode};
use wayland_client::protocol::wl_surface::WlSurface;

use super::*;

fn set_up() -> (Fixture, ClientId) {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));
    let id = f.add_client();
    (f, id)
}

fn add_window(f: &mut Fixture, id: ClientId, size: (u16, u16)) -> WlSurface {
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.set_size(size.0, size.1);
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    surface
}

fn active_workspace_window_count(f: &mut Fixture) -> usize {
    f.niri()
        .layout
        .active_workspace()
        .expect("active workspace")
        .windows()
        .count()
}

fn active_window_id(f: &mut Fixture) -> u64 {
    f.niri()
        .layout
        .active_workspace()
        .expect("active workspace")
        .active_window()
        .expect("active window")
        .id()
        .get()
}

fn layout_root(f: &mut Fixture) -> LayoutTreeNode {
    f.niri()
        .layout
        .layout_tree()
        .root
        .expect("layout tree root should exist")
}

fn leaf_count(node: &LayoutTreeNode) -> usize {
    if node.window_id.is_some() {
        1
    } else {
        node.children.iter().map(leaf_count).sum()
    }
}

fn collect_leaf_ids(node: &LayoutTreeNode, ids: &mut Vec<u64>) {
    if let Some(id) = node.window_id {
        ids.push(id);
    }

    for child in &node.children {
        collect_leaf_ids(child, ids);
    }
}

fn focused_leaf_count(node: &LayoutTreeNode) -> usize {
    let this = usize::from(node.window_id.is_some() && node.focused);
    this + node.children.iter().map(focused_leaf_count).sum::<usize>()
}

#[test]
fn split_vertical_creates_nested_splitv_subtree() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (110, 110));
    add_window(&mut f, id, (220, 220));

    f.niri().layout.split_vertical();
    f.double_roundtrip(id);
    add_window(&mut f, id, (330, 330));

    let root = layout_root(&mut f);
    assert_eq!(root.layout, Some(LayoutTreeLayout::SplitH));
    assert_eq!(leaf_count(&root), 3);
    assert_eq!(root.children.len(), 2);

    let nested_splitv = root
        .children
        .iter()
        .find(|child| child.layout == Some(LayoutTreeLayout::SplitV))
        .expect("expected a nested SplitV container");
    assert_eq!(nested_splitv.children.len(), 2);
    assert!(nested_splitv
        .children
        .iter()
        .all(|child| child.window_id.is_some()));
}

#[test]
fn split_horizontal_creates_three_root_leaf_children() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (110, 110));
    add_window(&mut f, id, (220, 220));

    f.niri().layout.split_horizontal();
    f.double_roundtrip(id);
    add_window(&mut f, id, (330, 330));

    let root = layout_root(&mut f);
    assert_eq!(root.layout, Some(LayoutTreeLayout::SplitH));
    assert_eq!(leaf_count(&root), 3);
    assert_eq!(root.children.len(), 3);
    assert!(root.children.iter().all(|child| child.window_id.is_some()));
}

#[test]
fn change_layout_to_tabbed_keeps_all_windows_and_moves_focus() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (100, 100));
    add_window(&mut f, id, (200, 200));
    add_window(&mut f, id, (300, 300));

    f.niri().layout.set_layout_mode(ContainerLayout::Tabbed);
    f.double_roundtrip(id);

    let root = layout_root(&mut f);
    assert_eq!(root.layout, Some(LayoutTreeLayout::Tabbed));
    assert_eq!(leaf_count(&root), 3);
    assert_eq!(root.children.len(), 3);
    assert_eq!(focused_leaf_count(&root), 1);
}

#[test]
fn change_layout_to_stacked_keeps_all_windows_and_moves_focus() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (100, 100));
    add_window(&mut f, id, (200, 200));
    add_window(&mut f, id, (300, 300));

    f.niri().layout.set_layout_mode(ContainerLayout::Stacked);
    f.double_roundtrip(id);

    let root = layout_root(&mut f);
    assert_eq!(root.layout, Some(LayoutTreeLayout::Stacked));
    assert_eq!(leaf_count(&root), 3);
    assert_eq!(root.children.len(), 3);
    assert_eq!(focused_leaf_count(&root), 1);
}

#[test]
fn toggle_split_layout_twice_restores_root_layout() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (120, 120));
    add_window(&mut f, id, (240, 240));

    let initial_root = layout_root(&mut f);
    assert_eq!(initial_root.layout, Some(LayoutTreeLayout::SplitH));
    assert_eq!(initial_root.children.len(), 2);

    f.niri().layout.toggle_split_layout();
    f.double_roundtrip(id);
    let toggled_root = layout_root(&mut f);
    assert_eq!(toggled_root.layout, Some(LayoutTreeLayout::SplitV));
    assert_eq!(leaf_count(&toggled_root), 2);

    f.niri().layout.toggle_split_layout();
    f.double_roundtrip(id);
    let restored_root = layout_root(&mut f);
    assert_eq!(restored_root.layout, Some(LayoutTreeLayout::SplitH));
    assert_eq!(restored_root.children.len(), 2);
    assert_eq!(leaf_count(&restored_root), 2);
}

#[test]
fn focus_parent_then_child_in_split_preserves_focused_window() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (100, 100));
    add_window(&mut f, id, (200, 200));

    f.niri().layout.split_vertical();
    f.double_roundtrip(id);
    add_window(&mut f, id, (300, 300));

    let before = active_window_id(&mut f);
    f.niri().layout.focus_parent();
    f.double_roundtrip(id);
    f.niri().layout.focus_child();
    f.double_roundtrip(id);
    let after = active_window_id(&mut f);

    assert_eq!(before, after);
}

#[test]
fn mixed_container_ops_keep_tree_leaf_ids_unique() {
    let (mut f, id) = set_up();
    add_window(&mut f, id, (120, 120));
    add_window(&mut f, id, (200, 200));
    add_window(&mut f, id, (280, 280));

    f.niri().layout.split_vertical();
    f.double_roundtrip(id);
    f.niri().layout.set_layout_mode(ContainerLayout::Tabbed);
    f.double_roundtrip(id);
    f.niri().layout.focus_window_down_or_top();
    f.double_roundtrip(id);
    f.niri().layout.set_layout_mode(ContainerLayout::SplitV);
    f.double_roundtrip(id);
    f.niri().layout.split_horizontal();
    f.double_roundtrip(id);
    add_window(&mut f, id, (360, 360));

    let root = layout_root(&mut f);
    let mut leaf_ids = Vec::new();
    collect_leaf_ids(&root, &mut leaf_ids);

    let unique = leaf_ids.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        leaf_ids.len(),
        unique.len(),
        "leaf window ids must be unique"
    );
    assert_eq!(leaf_ids.len(), leaf_count(&root));
    assert_eq!(leaf_ids.len(), active_workspace_window_count(&mut f));
}
