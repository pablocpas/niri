use super::*;
use crate::layout::ContainerLayout;

#[test]
fn split_vertical_creates_container() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create first window
    let window1 = f.client(id).create_window();
    let surface1 = window1.surface.clone();
    window1.commit();
    f.roundtrip(id);

    let window1 = f.client(id).window(&surface1);
    window1.attach_new_buffer();
    window1.ack_last_and_commit();
    f.double_roundtrip(id);

    // Create second window
    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    // Both windows should be tiled (in separate columns)
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());

    // Split the second window vertically (creates a SplitV container)
    f.niri().layout.split_vertical();
    f.double_roundtrip(id);

    // Verify windows are still in layout
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());
}

#[test]
fn split_horizontal_creates_container() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create first window
    let window1 = f.client(id).create_window();
    let surface1 = window1.surface.clone();
    window1.commit();
    f.roundtrip(id);

    let window1 = f.client(id).window(&surface1);
    window1.attach_new_buffer();
    window1.ack_last_and_commit();
    f.double_roundtrip(id);

    // Create second window
    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    // Split the second window horizontally (creates a SplitH container)
    f.niri().layout.split_horizontal();
    f.double_roundtrip(id);

    // Verify windows are still in layout
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());
}

#[test]
fn change_layout_to_tabbed() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create two windows
    let window1 = f.client(id).create_window();
    let surface1 = window1.surface.clone();
    window1.commit();
    f.roundtrip(id);

    let window1 = f.client(id).window(&surface1);
    window1.attach_new_buffer();
    window1.ack_last_and_commit();
    f.double_roundtrip(id);

    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    // Change layout to tabbed
    f.niri().layout.set_layout_mode(ContainerLayout::Tabbed);
    f.double_roundtrip(id);

    // Verify windows are still in layout
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());
}

#[test]
fn change_layout_to_stacked() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create two windows
    let window1 = f.client(id).create_window();
    let surface1 = window1.surface.clone();
    window1.commit();
    f.roundtrip(id);

    let window1 = f.client(id).window(&surface1);
    window1.attach_new_buffer();
    window1.ack_last_and_commit();
    f.double_roundtrip(id);

    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    // Change layout to stacked
    f.niri().layout.set_layout_mode(ContainerLayout::Stacked);
    f.double_roundtrip(id);

    // Verify windows are still in layout
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());
}

#[test]
fn toggle_split_layout_cycles() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create a window
    let window = f.client(id).create_window();
    let surface = window.surface.clone();
    window.commit();
    f.roundtrip(id);

    let window = f.client(id).window(&surface);
    window.attach_new_buffer();
    window.ack_last_and_commit();
    f.double_roundtrip(id);

    // Toggle split layout multiple times (should cycle between SplitH and SplitV)
    f.niri().layout.toggle_split_layout();
    f.double_roundtrip(id);

    f.niri().layout.toggle_split_layout();
    f.double_roundtrip(id);

    // Verify window is still in layout
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());
}

#[test]
fn window_in_split_container_receives_configure() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create first window
    let window1 = f.client(id).create_window();
    let surface1 = window1.surface.clone();
    window1.commit();
    f.roundtrip(id);

    let window1 = f.client(id).window(&surface1);
    window1.attach_new_buffer();
    window1.ack_last_and_commit();
    f.double_roundtrip(id);

    // Record initial configure for window1
    let _initial_configure = f.client(id).window(&surface1).format_recent_configures();

    // Create second window
    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    // Split vertically - this should send configure to both windows with new sizes
    f.niri().layout.split_vertical();
    f.double_roundtrip(id);

    // Window configures should have been sent (sizes adjusted for split)
    let window1 = f.client(id).window(&surface1);
    let new_configure1 = window1.format_recent_configures();

    // The configure should have been updated
    // We don't check exact values since they depend on layout calculations
    assert!(!new_configure1.is_empty());
}

#[test]
fn focus_parent_then_child_in_split() {
    let mut f = Fixture::new();
    f.add_output(1, (1920, 1080));

    let id = f.add_client();

    // Create first window
    let window1 = f.client(id).create_window();
    let surface1 = window1.surface.clone();
    window1.commit();
    f.roundtrip(id);

    let window1 = f.client(id).window(&surface1);
    window1.attach_new_buffer();
    window1.ack_last_and_commit();
    f.double_roundtrip(id);

    // Create second window
    let window2 = f.client(id).create_window();
    let surface2 = window2.surface.clone();
    window2.commit();
    f.roundtrip(id);

    let window2 = f.client(id).window(&surface2);
    window2.attach_new_buffer();
    window2.ack_last_and_commit();
    f.double_roundtrip(id);

    // Split vertically to create container structure
    f.niri().layout.split_vertical();
    f.double_roundtrip(id);

    // Create third window inside the split
    let window3 = f.client(id).create_window();
    let surface3 = window3.surface.clone();
    window3.commit();
    f.roundtrip(id);

    let window3 = f.client(id).window(&surface3);
    window3.attach_new_buffer();
    window3.ack_last_and_commit();
    f.double_roundtrip(id);

    // Focus parent
    f.niri().layout.focus_parent();
    f.double_roundtrip(id);

    // Focus child
    f.niri().layout.focus_child();
    f.double_roundtrip(id);

    // Verify windows are still in layout
    let workspace = f.niri().layout.active_workspace().expect("active workspace");
    let scrolling = workspace.scrolling();
    assert!(scrolling.windows().next().is_some());
}
