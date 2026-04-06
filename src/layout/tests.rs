use std::cell::{Cell, OnceCell, RefCell};

use insta::assert_snapshot;
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use smithay::output::{Mode, PhysicalProperties, Subpixel};
use smithay::utils::{Logical, Point, Rectangle, Size};
use tiri_config::utils::{Flag, MergeWith as _};
use tiri_config::workspace::WorkspaceName;
use tiri_config::{
    Config, FloatOrInt, OutputName, Struts, TabIndicatorLength, TabIndicatorPosition,
    WorkspaceReference,
};

use super::container::{ContainerTree, Direction, Layout as ContainerLayout};
use super::tile::Tile;
use super::*;

mod animations;
mod fullscreen;

impl<W: LayoutElement> Default for Layout<W> {
    fn default() -> Self {
        Self::with_options(Clock::with_time(Duration::ZERO), Default::default())
    }
}

fn make_test_output(name: &str) -> Output {
    let output = Output::new(
        name.to_string(),
        PhysicalProperties {
            size: Size::from((1280, 720)),
            subpixel: Subpixel::Unknown,
            make: String::new(),
            model: String::new(),
            serial_number: String::new(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: Size::from((1280, 720)),
            refresh: 60000,
        }),
        None,
        None,
        None,
    );
    output.user_data().insert_if_missing(|| OutputName {
        connector: name.to_string(),
        make: None,
        model: None,
        serial: None,
    });
    output
}

#[derive(Debug)]
struct TestWindowInner {
    id: usize,
    parent_id: Cell<Option<usize>>,
    bbox: Cell<Rectangle<i32, Logical>>,
    initial_bbox: Rectangle<i32, Logical>,
    requested_size: Cell<Option<Size<i32, Logical>>>,
    // Emulates the window ignoring the compositor-provided size.
    forced_size: Cell<Option<Size<i32, Logical>>>,
    min_size: Size<i32, Logical>,
    max_size: Size<i32, Logical>,
    pending_sizing_mode: Cell<SizingMode>,
    pending_activated: Cell<bool>,
    sizing_mode: Cell<SizingMode>,
    is_windowed_fullscreen: Cell<bool>,
    is_pending_windowed_fullscreen: Cell<bool>,
    animate_next_configure: Cell<bool>,
    animation_snapshot: RefCell<Option<LayoutElementRenderSnapshot>>,
    is_urgent: Cell<bool>,
    rules: ResolvedWindowRules,
}

#[derive(Debug, Clone)]
struct TestWindow(Rc<TestWindowInner>);

#[derive(Debug, Clone, Arbitrary)]
struct TestWindowParams {
    #[proptest(strategy = "1..=5usize")]
    id: usize,
    #[proptest(strategy = "arbitrary_parent_id()")]
    parent_id: Option<usize>,
    is_floating: bool,
    is_urgent: bool,
    #[proptest(strategy = "arbitrary_bbox()")]
    bbox: Rectangle<i32, Logical>,
    #[proptest(strategy = "arbitrary_min_max_size()")]
    min_max_size: (Size<i32, Logical>, Size<i32, Logical>),
    #[proptest(strategy = "prop::option::of(arbitrary_rules())")]
    rules: Option<ResolvedWindowRules>,
}

impl TestWindowParams {
    pub fn new(id: usize) -> Self {
        Self {
            id,
            parent_id: None,
            is_floating: false,
            is_urgent: false,
            bbox: Rectangle::from_size(Size::from((100, 200))),
            min_max_size: Default::default(),
            rules: None,
        }
    }
}

impl TestWindow {
    fn new(params: TestWindowParams) -> Self {
        Self(Rc::new(TestWindowInner {
            id: params.id,
            parent_id: Cell::new(params.parent_id),
            bbox: Cell::new(params.bbox),
            initial_bbox: params.bbox,
            requested_size: Cell::new(None),
            forced_size: Cell::new(None),
            min_size: params.min_max_size.0,
            max_size: params.min_max_size.1,
            pending_sizing_mode: Cell::new(SizingMode::Normal),
            pending_activated: Cell::new(false),
            sizing_mode: Cell::new(SizingMode::Normal),
            is_windowed_fullscreen: Cell::new(false),
            is_pending_windowed_fullscreen: Cell::new(false),
            animate_next_configure: Cell::new(false),
            animation_snapshot: RefCell::new(None),
            is_urgent: Cell::new(params.is_urgent),
            rules: params.rules.unwrap_or_default(),
        }))
    }

    fn communicate(&self) -> bool {
        let mut changed = false;

        let size = self.0.forced_size.get().or(self.0.requested_size.get());
        if let Some(size) = size {
            assert!(size.w >= 0);
            assert!(size.h >= 0);

            let mut new_bbox = self.0.initial_bbox;
            if size.w != 0 {
                new_bbox.size.w = size.w;
            }
            if size.h != 0 {
                new_bbox.size.h = size.h;
            }

            if self.0.bbox.get() != new_bbox {
                if self.0.animate_next_configure.get() {
                    self.0.animation_snapshot.replace(Some(RenderSnapshot {
                        contents: Vec::new(),
                        blocked_out_contents: Vec::new(),
                        block_out_from: None,
                        size: self.0.bbox.get().size.to_f64(),
                        texture: OnceCell::new(),
                        blocked_out_texture: OnceCell::new(),
                    }));
                }

                self.0.bbox.set(new_bbox);
                changed = true;
            }
        }

        self.0.animate_next_configure.set(false);

        if self.0.sizing_mode.get() != self.0.pending_sizing_mode.get() {
            self.0.sizing_mode.set(self.0.pending_sizing_mode.get());
            changed = true;
        }

        if self.0.is_windowed_fullscreen.get() != self.0.is_pending_windowed_fullscreen.get() {
            self.0
                .is_windowed_fullscreen
                .set(self.0.is_pending_windowed_fullscreen.get());
            changed = true;
        }

        changed
    }

    fn set_urgent(&self, urgent: bool) {
        self.0.is_urgent.set(urgent);
    }
}

impl LayoutElement for TestWindow {
    type Id = usize;

    fn id(&self) -> &Self::Id {
        &self.0.id
    }

    fn title(&self) -> Option<String> {
        Some(format!("Window {}", self.0.id))
    }

    fn size(&self) -> Size<i32, Logical> {
        self.0.bbox.get().size
    }

    fn buf_loc(&self) -> Point<i32, Logical> {
        (0, 0).into()
    }

    fn is_in_input_region(&self, _point: Point<f64, Logical>) -> bool {
        false
    }

    fn request_size(
        &mut self,
        size: Size<i32, Logical>,
        mode: SizingMode,
        _animate: bool,
        _transaction: Option<Transaction>,
    ) {
        if self.0.requested_size.get() != Some(size) {
            self.0.requested_size.set(Some(size));
            self.0.animate_next_configure.set(true);
        }

        self.0.pending_sizing_mode.set(mode);

        if mode.is_fullscreen() {
            self.0.is_pending_windowed_fullscreen.set(false);
        }
    }

    fn min_size(&self) -> Size<i32, Logical> {
        self.0.min_size
    }

    fn max_size(&self) -> Size<i32, Logical> {
        self.0.max_size
    }

    fn is_wl_surface(&self, _wl_surface: &WlSurface) -> bool {
        false
    }

    fn set_preferred_scale_transform(&self, _scale: output::Scale, _transform: Transform) {}

    fn has_ssd(&self) -> bool {
        false
    }

    fn output_enter(&self, _output: &Output) {}

    fn output_leave(&self, _output: &Output) {}

    fn set_offscreen_data(&self, _data: Option<OffscreenData>) {}

    fn set_activated(&mut self, active: bool) {
        self.0.pending_activated.set(active);
    }

    fn set_bounds(&self, _bounds: Size<i32, Logical>) {}

    fn is_ignoring_opacity_window_rule(&self) -> bool {
        false
    }

    fn configure_intent(&self) -> ConfigureIntent {
        ConfigureIntent::CanSend
    }

    fn send_pending_configure(&mut self) {}

    fn set_active_in_column(&mut self, _active: bool) {}

    fn set_floating(&mut self, _floating: bool) {}

    fn sizing_mode(&self) -> SizingMode {
        self.0.sizing_mode.get()
    }

    fn pending_sizing_mode(&self) -> SizingMode {
        self.0.pending_sizing_mode.get()
    }

    fn requested_size(&self) -> Option<Size<i32, Logical>> {
        self.0.requested_size.get()
    }

    fn is_pending_windowed_fullscreen(&self) -> bool {
        self.0.is_pending_windowed_fullscreen.get()
    }

    fn request_windowed_fullscreen(&mut self, value: bool) {
        self.0.is_pending_windowed_fullscreen.set(value);
    }

    fn is_child_of(&self, parent: &Self) -> bool {
        self.0.parent_id.get() == Some(parent.0.id)
    }

    fn refresh(&self) {}

    fn rules(&self) -> &ResolvedWindowRules {
        &self.0.rules
    }

    fn take_animation_snapshot(&mut self) -> Option<LayoutElementRenderSnapshot> {
        self.0.animation_snapshot.take()
    }

    fn set_interactive_resize(&mut self, _data: Option<InteractiveResizeData>) {}

    fn cancel_interactive_resize(&mut self) {}

    fn on_commit(&mut self, _serial: Serial) {}

    fn interactive_resize_data(&self) -> Option<InteractiveResizeData> {
        None
    }

    fn is_urgent(&self) -> bool {
        self.0.is_urgent.get()
    }
}

fn arbitrary_size() -> impl Strategy<Value = Size<i32, Logical>> {
    any::<(u16, u16)>().prop_map(|(w, h)| Size::from((w.max(1).into(), h.max(1).into())))
}

fn arbitrary_bbox() -> impl Strategy<Value = Rectangle<i32, Logical>> {
    any::<(i16, i16, u16, u16)>().prop_map(|(x, y, w, h)| {
        let loc: Point<i32, _> = Point::from((x.into(), y.into()));
        let size: Size<i32, _> = Size::from((w.max(1).into(), h.max(1).into()));
        Rectangle::new(loc, size)
    })
}

fn arbitrary_size_change() -> impl Strategy<Value = SizeChange> {
    prop_oneof![
        (0..).prop_map(SizeChange::SetFixed),
        (0f64..).prop_map(SizeChange::SetProportion),
        any::<i32>().prop_map(SizeChange::AdjustFixed),
        any::<f64>().prop_map(SizeChange::AdjustProportion),
        // Interactive resize can have negative values here.
        Just(SizeChange::SetFixed(-100)),
    ]
}

fn arbitrary_position_change() -> impl Strategy<Value = PositionChange> {
    prop_oneof![
        (-1000f64..1000f64).prop_map(PositionChange::SetFixed),
        any::<f64>().prop_map(PositionChange::SetProportion),
        (-1000f64..1000f64).prop_map(PositionChange::AdjustFixed),
        any::<f64>().prop_map(PositionChange::AdjustProportion),
        any::<f64>().prop_map(PositionChange::SetFixed),
        any::<f64>().prop_map(PositionChange::AdjustFixed),
    ]
}

fn arbitrary_min_max() -> impl Strategy<Value = (i32, i32)> {
    prop_oneof![
        Just((0, 0)),
        (1..65536).prop_map(|n| (n, n)),
        (1..65536).prop_map(|min| (min, 0)),
        (1..).prop_map(|max| (0, max)),
        (1..65536, 1..).prop_map(|(min, max): (i32, i32)| (min, max.max(min))),
    ]
}

fn arbitrary_min_max_size() -> impl Strategy<Value = (Size<i32, Logical>, Size<i32, Logical>)> {
    prop_oneof![
        5 => (arbitrary_min_max(), arbitrary_min_max()).prop_map(
            |((min_w, max_w), (min_h, max_h))| {
                let min_size = Size::from((min_w, min_h));
                let max_size = Size::from((max_w, max_h));
                (min_size, max_size)
            },
        ),
        1 => arbitrary_min_max().prop_map(|(w, h)| {
            let size = Size::from((w, h));
            (size, size)
        }),
    ]
}

prop_compose! {
    fn arbitrary_rules()(
        focus_ring in arbitrary_focus_ring(),
        border in arbitrary_border(),
    ) -> ResolvedWindowRules {
        ResolvedWindowRules {
            focus_ring,
            border,
            ..ResolvedWindowRules::default()
        }
    }
}

fn arbitrary_view_offset_gesture_delta() -> impl Strategy<Value = f64> {
    prop_oneof![(-10f64..10f64), (-50000f64..50000f64),]
}

fn arbitrary_resize_edge() -> impl Strategy<Value = ResizeEdge> {
    prop_oneof![
        Just(ResizeEdge::RIGHT),
        Just(ResizeEdge::BOTTOM),
        Just(ResizeEdge::LEFT),
        Just(ResizeEdge::TOP),
        Just(ResizeEdge::BOTTOM_RIGHT),
        Just(ResizeEdge::BOTTOM_LEFT),
        Just(ResizeEdge::TOP_RIGHT),
        Just(ResizeEdge::TOP_LEFT),
        Just(ResizeEdge::empty()),
    ]
}

fn arbitrary_scale() -> impl Strategy<Value = f64> {
    prop_oneof![Just(1.), Just(1.5), Just(2.),]
}

fn arbitrary_msec_delta() -> impl Strategy<Value = i32> {
    prop_oneof![
        1 => Just(-1000),
        2 => Just(-10),
        1 => Just(0),
        2 => Just(10),
        6 => Just(1000),
    ]
}

fn arbitrary_parent_id() -> impl Strategy<Value = Option<usize>> {
    prop_oneof![
        5 => Just(None),
        1 => prop::option::of(1..=5usize),
    ]
}

fn arbitrary_scroll_direction() -> impl Strategy<Value = ScrollDirection> {
    prop_oneof![Just(ScrollDirection::Left), Just(ScrollDirection::Right)]
}

fn arbitrary_column_display() -> impl Strategy<Value = ColumnDisplay> {
    prop_oneof![Just(ColumnDisplay::Normal), Just(ColumnDisplay::Tabbed)]
}

fn arbitrary_mark_mode() -> impl Strategy<Value = MarkMode> {
    prop_oneof![
        Just(MarkMode::Replace),
        Just(MarkMode::Add),
        Just(MarkMode::Toggle),
    ]
}

#[derive(Debug, Clone, Arbitrary)]
enum Op {
    AddOutput(#[proptest(strategy = "1..=5usize")] usize),
    AddScaledOutput {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "arbitrary_scale()")]
        scale: f64,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<tiri_config::LayoutPart>>,
    },
    RemoveOutput(#[proptest(strategy = "1..=5usize")] usize),
    FocusOutput(#[proptest(strategy = "1..=5usize")] usize),
    UpdateOutputLayoutConfig {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<tiri_config::LayoutPart>>,
    },
    AddNamedWorkspace {
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
        #[proptest(strategy = "prop::option::of(1..=5usize)")]
        output_name: Option<usize>,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<tiri_config::LayoutPart>>,
    },
    UnnameWorkspace {
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
    },
    UpdateWorkspaceLayoutConfig {
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
        #[proptest(strategy = "prop::option::of(arbitrary_layout_part().prop_map(Box::new))")]
        layout_config: Option<Box<tiri_config::LayoutPart>>,
    },
    AddWindow {
        params: TestWindowParams,
    },
    AddWindowNextTo {
        params: TestWindowParams,
        #[proptest(strategy = "1..=5usize")]
        next_to_id: usize,
    },
    AddWindowToNamedWorkspace {
        params: TestWindowParams,
        #[proptest(strategy = "1..=5usize")]
        ws_name: usize,
    },
    CloseWindow(#[proptest(strategy = "1..=5usize")] usize),
    FullscreenWindow(#[proptest(strategy = "1..=5usize")] usize),
    SetFullscreenWindow {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        is_fullscreen: bool,
    },
    ToggleWindowedFullscreen(#[proptest(strategy = "1..=5usize")] usize),
    FocusColumnLeft,
    FocusColumnRight,
    FocusColumnFirst,
    FocusColumnLast,
    FocusColumnRightOrFirst,
    FocusColumnLeftOrLast,
    FocusColumn(#[proptest(strategy = "1..=5usize")] usize),
    FocusWindowOrMonitorUp(#[proptest(strategy = "1..=2u8")] u8),
    FocusWindowOrMonitorDown(#[proptest(strategy = "1..=2u8")] u8),
    FocusColumnOrMonitorLeft(#[proptest(strategy = "1..=2u8")] u8),
    FocusColumnOrMonitorRight(#[proptest(strategy = "1..=2u8")] u8),
    FocusWindowDown,
    FocusWindowUp,
    FocusWindowDownOrColumnLeft,
    FocusWindowDownOrColumnRight,
    FocusWindowUpOrColumnLeft,
    FocusWindowUpOrColumnRight,
    FocusWindowOrWorkspaceDown,
    FocusWindowOrWorkspaceUp,
    FocusWindow(#[proptest(strategy = "1..=5usize")] usize),
    FocusWindowInColumn(#[proptest(strategy = "1..=5u8")] u8),
    FocusWindowTop,
    FocusWindowBottom,
    FocusWindowDownOrTop,
    FocusWindowUpOrBottom,
    MoveColumnLeft,
    MoveColumnRight,
    MoveColumnToFirst,
    MoveColumnToLast,
    MoveColumnLeftOrToMonitorLeft(#[proptest(strategy = "1..=2u8")] u8),
    MoveColumnRightOrToMonitorRight(#[proptest(strategy = "1..=2u8")] u8),
    MoveColumnToIndex(#[proptest(strategy = "1..=5usize")] usize),
    MoveWindowDown,
    MoveWindowUp,
    MoveWindowDownOrToWorkspaceDown,
    MoveWindowUpOrToWorkspaceUp,
    ConsumeOrExpelWindowLeft {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ConsumeOrExpelWindowRight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ConsumeWindowIntoColumn,
    ExpelWindowFromColumn,
    SwapWindowInDirection(#[proptest(strategy = "arbitrary_scroll_direction()")] ScrollDirection),
    ToggleColumnTabbedDisplay,
    SetColumnDisplay(#[proptest(strategy = "arbitrary_column_display()")] ColumnDisplay),
    CenterColumn,
    CenterWindow {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    CenterVisibleColumns,
    FocusWorkspaceDown,
    FocusWorkspaceUp,
    FocusWorkspace(#[proptest(strategy = "0..=4usize")] usize),
    FocusWorkspaceAutoBackAndForth(#[proptest(strategy = "0..=4usize")] usize),
    FocusWorkspacePrevious,
    MoveWindowToWorkspaceDown(bool),
    MoveWindowToWorkspaceUp(bool),
    MoveWindowToWorkspace {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        window_id: Option<usize>,
        #[proptest(strategy = "0..=4usize")]
        workspace_idx: usize,
    },
    MoveColumnToWorkspaceDown(bool),
    MoveColumnToWorkspaceUp(bool),
    MoveColumnToWorkspace(#[proptest(strategy = "0..=4usize")] usize, bool),
    MoveWorkspaceDown,
    MoveWorkspaceUp,
    MoveWorkspaceToIndex {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
        #[proptest(strategy = "0..=4usize")]
        target_idx: usize,
    },
    MoveWorkspaceToMonitor {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
        #[proptest(strategy = "0..=5usize")]
        output_id: usize,
    },
    SetWorkspaceName {
        #[proptest(strategy = "1..=5usize")]
        new_ws_name: usize,
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
    },
    UnsetWorkspaceName {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        ws_name: Option<usize>,
    },
    MoveWindowToOutput {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        window_id: Option<usize>,
        #[proptest(strategy = "1..=5usize")]
        output_id: usize,
        #[proptest(strategy = "proptest::option::of(0..=4usize)")]
        target_ws_idx: Option<usize>,
    },
    MoveColumnToOutput {
        #[proptest(strategy = "1..=5usize")]
        output_id: usize,
        #[proptest(strategy = "proptest::option::of(0..=4usize)")]
        target_ws_idx: Option<usize>,
        activate: bool,
    },
    SwitchPresetColumnWidth,
    SwitchPresetColumnWidthBack,
    SwitchPresetWindowWidth {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SwitchPresetWindowWidthBack {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SwitchPresetWindowHeight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SwitchPresetWindowHeightBack {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    MaximizeColumn,
    MaximizeWindowToEdges {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SetColumnWidth(#[proptest(strategy = "arbitrary_size_change()")] SizeChange),
    SetWindowWidth {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        #[proptest(strategy = "arbitrary_size_change()")]
        change: SizeChange,
    },
    SetWindowHeight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        #[proptest(strategy = "arbitrary_size_change()")]
        change: SizeChange,
    },
    ResetWindowHeight {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ExpandColumnToAvailableWidth,
    ToggleWindowFloating {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    SetWindowFloating {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        floating: bool,
    },
    FocusFloating,
    FocusTiling,
    SwitchFocusFloatingTiling,
    MoveFloatingWindow {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
        #[proptest(strategy = "arbitrary_position_change()")]
        x: PositionChange,
        #[proptest(strategy = "arbitrary_position_change()")]
        y: PositionChange,
        animate: bool,
    },
    SetParent {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "prop::option::of(1..=5usize)")]
        new_parent_id: Option<usize>,
    },
    SetForcedSize {
        #[proptest(strategy = "1..=5usize")]
        id: usize,
        #[proptest(strategy = "proptest::option::of(arbitrary_size())")]
        size: Option<Size<i32, Logical>>,
    },
    Communicate(#[proptest(strategy = "1..=5usize")] usize),
    Refresh {
        is_active: bool,
    },
    AdvanceAnimations {
        #[proptest(strategy = "arbitrary_msec_delta()")]
        msec_delta: i32,
    },
    CompleteAnimations,
    MoveWorkspaceToOutput(#[proptest(strategy = "1..=5usize")] usize),
    ViewOffsetGestureBegin {
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "proptest::option::of(0..=4usize)")]
        workspace_idx: Option<usize>,
        is_touchpad: bool,
    },
    ViewOffsetGestureUpdate {
        #[proptest(strategy = "arbitrary_view_offset_gesture_delta()")]
        delta: f64,
        timestamp: Duration,
        is_touchpad: bool,
    },
    ViewOffsetGestureEnd {
        is_touchpad: Option<bool>,
    },
    WorkspaceSwitchGestureBegin {
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        is_touchpad: bool,
    },
    WorkspaceSwitchGestureUpdate {
        #[proptest(strategy = "-400f64..400f64")]
        delta: f64,
        timestamp: Duration,
        is_touchpad: bool,
    },
    WorkspaceSwitchGestureEnd {
        is_touchpad: Option<bool>,
    },
    OverviewGestureBegin,
    OverviewGestureUpdate {
        #[proptest(strategy = "-400f64..400f64")]
        delta: f64,
        timestamp: Duration,
    },
    OverviewGestureEnd,
    InteractiveMoveBegin {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        px: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        py: f64,
    },
    InteractiveMoveUpdate {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        dx: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        dy: f64,
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        px: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        py: f64,
    },
    InteractiveMoveEnd {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
    },
    DndUpdate {
        #[proptest(strategy = "1..=5usize")]
        output_idx: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        px: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        py: f64,
    },
    DndEnd,
    InteractiveResizeBegin {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "arbitrary_resize_edge()")]
        edges: ResizeEdge,
    },
    InteractiveResizeUpdate {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
        #[proptest(strategy = "-20000f64..20000f64")]
        dx: f64,
        #[proptest(strategy = "-20000f64..20000f64")]
        dy: f64,
    },
    InteractiveResizeEnd {
        #[proptest(strategy = "1..=5usize")]
        window: usize,
    },
    ToggleOverview,
    UpdateConfig {
        #[proptest(strategy = "arbitrary_layout_part().prop_map(Box::new)")]
        layout_config: Box<tiri_config::LayoutPart>,
    },
    // Container tree operations (i3-like)
    FocusParent,
    FocusChild,
    SplitHorizontal,
    SplitVertical,
    SetLayoutSplitH,
    SetLayoutSplitV,
    SetLayoutTabbed,
    SetLayoutStacked,
    ToggleSplitLayout,
    ToggleLayoutAll,
    // Mark operations
    MarkFocused {
        #[proptest(strategy = "1..=3usize")]
        mark_id: usize,
        #[proptest(strategy = "arbitrary_mark_mode()")]
        mode: MarkMode,
    },
    // Scratchpad operations
    MoveWindowToScratchpad {
        #[proptest(strategy = "proptest::option::of(1..=5usize)")]
        id: Option<usize>,
    },
    ScratchpadShow,
}

impl Op {
    fn apply(self, layout: &mut Layout<TestWindow>) {
        match self {
            Op::AddOutput(id) => {
                let name = format!("output{id}");
                if layout.outputs().any(|o| o.name() == name) {
                    return;
                }

                let output = Output::new(
                    name.clone(),
                    PhysicalProperties {
                        size: Size::from((1280, 720)),
                        subpixel: Subpixel::Unknown,
                        make: String::new(),
                        model: String::new(),
                        serial_number: String::new(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: Size::from((1280, 720)),
                        refresh: 60000,
                    }),
                    None,
                    None,
                    None,
                );
                output.user_data().insert_if_missing(|| OutputName {
                    connector: name,
                    make: None,
                    model: None,
                    serial: None,
                });
                layout.add_output(output.clone(), None);
            }
            Op::AddScaledOutput {
                id,
                scale,
                layout_config,
            } => {
                let name = format!("output{id}");
                if layout.outputs().any(|o| o.name() == name) {
                    return;
                }

                let output = Output::new(
                    name.clone(),
                    PhysicalProperties {
                        size: Size::from((1280, 720)),
                        subpixel: Subpixel::Unknown,
                        make: String::new(),
                        model: String::new(),
                        serial_number: String::new(),
                    },
                );
                output.change_current_state(
                    Some(Mode {
                        size: Size::from((1280, 720)),
                        refresh: 60000,
                    }),
                    None,
                    Some(smithay::output::Scale::Fractional(scale)),
                    None,
                );
                output.user_data().insert_if_missing(|| OutputName {
                    connector: name,
                    make: None,
                    model: None,
                    serial: None,
                });
                layout.add_output(output.clone(), layout_config.map(|x| *x));
            }
            Op::RemoveOutput(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.remove_output(&output);
            }
            Op::FocusOutput(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_output(&output);
            }
            Op::UpdateOutputLayoutConfig { id, layout_config } => {
                let name = format!("output{id}");
                let Some(mon) = layout.monitors_mut().find(|m| m.output_name() == &name) else {
                    return;
                };

                mon.update_layout_config(layout_config.map(|x| *x));
            }
            Op::AddNamedWorkspace {
                ws_name,
                output_name,
                layout_config,
            } => {
                layout.ensure_named_workspace(&WorkspaceConfig {
                    name: WorkspaceName(format!("ws{ws_name}")),
                    open_on_output: output_name.map(|name| format!("output{name}")),
                    layout: layout_config.map(|x| tiri_config::WorkspaceLayoutPart(*x)),
                });
            }
            Op::UnnameWorkspace { ws_name } => {
                layout.unname_workspace(&format!("ws{ws_name}"));
            }
            Op::UpdateWorkspaceLayoutConfig {
                ws_name,
                layout_config,
            } => {
                let ws_name = format!("ws{ws_name}");
                let Some(ws) = layout
                    .workspaces_mut()
                    .find(|ws| ws.name() == Some(&ws_name))
                else {
                    return;
                };

                ws.update_layout_config(layout_config.map(|x| *x));
            }
            Op::SetWorkspaceName {
                new_ws_name,
                ws_name,
            } => {
                let ws_ref =
                    ws_name.map(|ws_name| WorkspaceReference::Name(format!("ws{ws_name}")));
                layout.set_workspace_name(format!("ws{new_ws_name}"), ws_ref);
            }
            Op::UnsetWorkspaceName { ws_name } => {
                let ws_ref =
                    ws_name.map(|ws_name| WorkspaceReference::Name(format!("ws{ws_name}")));
                layout.unset_workspace_name(ws_ref);
            }
            Op::AddWindow { mut params } => {
                if layout.has_window(&params.id) {
                    return;
                }
                if let Some(parent_id) = params.parent_id {
                    if parent_id_causes_loop(layout, params.id, parent_id) {
                        params.parent_id = None;
                    }
                }

                let is_floating = params.is_floating;
                let win = TestWindow::new(params);
                layout.add_window(
                    win,
                    AddWindowTarget::Auto,
                    None,
                    None,
                    false,
                    is_floating,
                    ActivateWindow::default(),
                );
            }
            Op::AddWindowNextTo {
                mut params,
                next_to_id,
            } => {
                let mut found_next_to = false;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    let win_id = move_.tile.window().0.id;
                    if win_id == params.id {
                        return;
                    }
                    if win_id == next_to_id {
                        found_next_to = true;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == params.id {
                                        return;
                                    }

                                    if win.0.id == next_to_id {
                                        found_next_to = true;
                                    }
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == params.id {
                                    return;
                                }

                                if win.0.id == next_to_id {
                                    found_next_to = true;
                                }
                            }
                        }
                    }
                }

                if !found_next_to {
                    return;
                }

                if let Some(parent_id) = params.parent_id {
                    if parent_id_causes_loop(layout, params.id, parent_id) {
                        params.parent_id = None;
                    }
                }

                let is_floating = params.is_floating;
                let win = TestWindow::new(params);
                layout.add_window(
                    win,
                    AddWindowTarget::NextTo(&next_to_id),
                    None,
                    None,
                    false,
                    is_floating,
                    ActivateWindow::default(),
                );
            }
            Op::AddWindowToNamedWorkspace {
                mut params,
                ws_name,
            } => {
                let ws_name = format!("ws{ws_name}");
                let mut ws_id = None;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    if move_.tile.window().0.id == params.id {
                        return;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == params.id {
                                        return;
                                    }
                                }

                                if ws
                                    .name
                                    .as_ref()
                                    .is_some_and(|name| name.eq_ignore_ascii_case(&ws_name))
                                {
                                    ws_id = Some(ws.id());
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == params.id {
                                    return;
                                }
                            }

                            if ws
                                .name
                                .as_ref()
                                .is_some_and(|name| name.eq_ignore_ascii_case(&ws_name))
                            {
                                ws_id = Some(ws.id());
                            }
                        }
                    }
                }

                let Some(ws_id) = ws_id else {
                    return;
                };

                if let Some(parent_id) = params.parent_id {
                    if parent_id_causes_loop(layout, params.id, parent_id) {
                        params.parent_id = None;
                    }
                }

                let is_floating = params.is_floating;
                let win = TestWindow::new(params);
                layout.add_window(
                    win,
                    AddWindowTarget::Workspace(ws_id),
                    None,
                    None,
                    false,
                    is_floating,
                    ActivateWindow::default(),
                );
            }
            Op::CloseWindow(id) => {
                layout.remove_window(&id, Transaction::new());
            }
            Op::FullscreenWindow(id) => {
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_fullscreen(&id);
            }
            Op::SetFullscreenWindow {
                window,
                is_fullscreen,
            } => {
                if !layout.has_window(&window) {
                    return;
                }
                layout.set_fullscreen(&window, is_fullscreen);
            }
            Op::ToggleWindowedFullscreen(id) => {
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_windowed_fullscreen(&id);
            }
            Op::FocusColumnLeft => layout.focus_left(),
            Op::FocusColumnRight => layout.focus_right(),
            Op::FocusColumnFirst => layout.focus_column_first(),
            Op::FocusColumnLast => layout.focus_column_last(),
            Op::FocusColumnRightOrFirst => layout.focus_column_right_or_first(),
            Op::FocusColumnLeftOrLast => layout.focus_column_left_or_last(),
            Op::FocusColumn(index) => layout.focus_column(index),
            Op::FocusWindowOrMonitorUp(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_window_up_or_output(&output);
            }
            Op::FocusWindowOrMonitorDown(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_window_down_or_output(&output);
            }
            Op::FocusColumnOrMonitorLeft(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_column_left_or_output(&output);
            }
            Op::FocusColumnOrMonitorRight(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.focus_column_right_or_output(&output);
            }
            Op::FocusWindowDown => layout.focus_down(),
            Op::FocusWindowUp => layout.focus_up(),
            Op::FocusWindowDownOrColumnLeft => layout.focus_down_or_left(),
            Op::FocusWindowDownOrColumnRight => layout.focus_down_or_right(),
            Op::FocusWindowUpOrColumnLeft => layout.focus_up_or_left(),
            Op::FocusWindowUpOrColumnRight => layout.focus_up_or_right(),
            Op::FocusWindowOrWorkspaceDown => layout.focus_window_or_workspace_down(),
            Op::FocusWindowOrWorkspaceUp => layout.focus_window_or_workspace_up(),
            Op::FocusWindow(id) => layout.activate_window(&id),
            Op::FocusWindowInColumn(index) => layout.focus_window_in_column(index),
            Op::FocusWindowTop => layout.focus_window_top(),
            Op::FocusWindowBottom => layout.focus_window_bottom(),
            Op::FocusWindowDownOrTop => layout.focus_window_down_or_top(),
            Op::FocusWindowUpOrBottom => layout.focus_window_up_or_bottom(),
            Op::MoveColumnLeft => layout.move_left(),
            Op::MoveColumnRight => layout.move_right(),
            Op::MoveColumnToFirst => layout.move_column_to_first(),
            Op::MoveColumnToLast => layout.move_column_to_last(),
            Op::MoveColumnLeftOrToMonitorLeft(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_column_left_or_to_output(&output);
            }
            Op::MoveColumnRightOrToMonitorRight(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_column_right_or_to_output(&output);
            }
            Op::MoveColumnToIndex(index) => layout.move_column_to_index(index),
            Op::MoveWindowDown => layout.move_down(),
            Op::MoveWindowUp => layout.move_up(),
            Op::MoveWindowDownOrToWorkspaceDown => layout.move_down_or_to_workspace_down(),
            Op::MoveWindowUpOrToWorkspaceUp => layout.move_up_or_to_workspace_up(),
            Op::ConsumeOrExpelWindowLeft { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.consume_or_expel_window_left(id.as_ref());
            }
            Op::ConsumeOrExpelWindowRight { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.consume_or_expel_window_right(id.as_ref());
            }
            Op::ConsumeWindowIntoColumn => layout.consume_into_column(),
            Op::ExpelWindowFromColumn => layout.expel_from_column(),
            Op::SwapWindowInDirection(direction) => layout.swap_window_in_direction(direction),
            Op::ToggleColumnTabbedDisplay => layout.toggle_column_tabbed_display(),
            Op::SetColumnDisplay(display) => layout.set_column_display(display),
            Op::CenterColumn => layout.center_column(),
            Op::CenterWindow { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.center_window(id.as_ref());
            }
            Op::CenterVisibleColumns => layout.center_visible_columns(),
            Op::FocusWorkspaceDown => layout.switch_workspace_down(),
            Op::FocusWorkspaceUp => layout.switch_workspace_up(),
            Op::FocusWorkspace(idx) => layout.switch_workspace(idx),
            Op::FocusWorkspaceAutoBackAndForth(idx) => {
                layout.switch_workspace_auto_back_and_forth(idx)
            }
            Op::FocusWorkspacePrevious => layout.switch_workspace_previous(),
            Op::MoveWindowToWorkspaceDown(focus) => layout.move_to_workspace_down(focus),
            Op::MoveWindowToWorkspaceUp(focus) => layout.move_to_workspace_up(focus),
            Op::MoveWindowToWorkspace {
                window_id,
                workspace_idx,
            } => {
                let window_id = window_id.filter(|id| layout.has_window(id));
                layout.move_to_workspace(window_id.as_ref(), workspace_idx, ActivateWindow::Smart);
            }
            Op::MoveColumnToWorkspaceDown(focus) => layout.move_column_to_workspace_down(focus),
            Op::MoveColumnToWorkspaceUp(focus) => layout.move_column_to_workspace_up(focus),
            Op::MoveColumnToWorkspace(idx, focus) => layout.move_column_to_workspace(idx, focus),
            Op::MoveWindowToOutput {
                window_id,
                output_id: id,
                target_ws_idx,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                let mon = layout.monitor_for_output(&output).unwrap();

                let window_id = window_id.filter(|id| layout.has_window(id));
                let target_ws_idx = target_ws_idx.filter(|idx| mon.workspaces.len() > *idx);
                layout.move_to_output(
                    window_id.as_ref(),
                    &output,
                    target_ws_idx,
                    ActivateWindow::Smart,
                );
            }
            Op::MoveColumnToOutput {
                output_id: id,
                target_ws_idx,
                activate,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_column_to_output(&output, target_ws_idx, activate);
            }
            Op::MoveWorkspaceDown => layout.move_workspace_down(),
            Op::MoveWorkspaceUp => layout.move_workspace_up(),
            Op::MoveWorkspaceToIndex {
                ws_name: Some(ws_name),
                target_idx,
            } => {
                let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
                    return;
                };

                let Some((old_idx, old_output)) = monitors.iter().find_map(|monitor| {
                    monitor
                        .workspaces
                        .iter()
                        .enumerate()
                        .find_map(|(i, ws)| {
                            if ws.name == Some(format!("ws{ws_name}")) {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .map(|i| (i, monitor.output.clone()))
                }) else {
                    return;
                };

                layout.move_workspace_to_idx(Some((Some(old_output), old_idx)), target_idx)
            }
            Op::MoveWorkspaceToIndex {
                ws_name: None,
                target_idx,
            } => layout.move_workspace_to_idx(None, target_idx),
            Op::MoveWorkspaceToMonitor {
                ws_name: None,
                output_id: id,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.move_workspace_to_output(&output);
            }
            Op::MoveWorkspaceToMonitor {
                ws_name: Some(ws_name),
                output_id: id,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
                    return;
                };

                let Some((old_idx, old_output)) = monitors.iter().find_map(|monitor| {
                    monitor
                        .workspaces
                        .iter()
                        .enumerate()
                        .find_map(|(i, ws)| {
                            if ws.name == Some(format!("ws{ws_name}")) {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .map(|i| (i, monitor.output.clone()))
                }) else {
                    return;
                };

                layout.move_workspace_to_output_by_index(old_idx, Some(old_output), &output);
            }
            Op::SwitchPresetColumnWidth => layout.toggle_width(true),
            Op::SwitchPresetColumnWidthBack => layout.toggle_width(false),
            Op::SwitchPresetWindowWidth { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_width(id.as_ref(), true);
            }
            Op::SwitchPresetWindowWidthBack { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_width(id.as_ref(), false);
            }
            Op::SwitchPresetWindowHeight { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_height(id.as_ref(), true);
            }
            Op::SwitchPresetWindowHeightBack { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_height(id.as_ref(), false);
            }
            Op::MaximizeColumn => layout.toggle_full_width(),
            Op::MaximizeWindowToEdges { id } => {
                let id = id.or_else(|| layout.focus().map(|win| *win.id()));
                let Some(id) = id else {
                    return;
                };
                if !layout.has_window(&id) {
                    return;
                }
                layout.toggle_maximized(&id);
            }
            Op::SetColumnWidth(change) => layout.set_column_width(change),
            Op::SetWindowWidth { id, change } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.set_window_width(id.as_ref(), change);
            }
            Op::SetWindowHeight { id, change } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.set_window_height(id.as_ref(), change);
            }
            Op::ResetWindowHeight { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.reset_window_height(id.as_ref());
            }
            Op::ExpandColumnToAvailableWidth => layout.expand_column_to_available_width(),
            Op::ToggleWindowFloating { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.toggle_window_floating(id.as_ref());
            }
            Op::SetWindowFloating { id, floating } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.set_window_floating(id.as_ref(), floating);
            }
            Op::FocusFloating => {
                layout.focus_floating();
            }
            Op::FocusTiling => {
                layout.focus_tiling();
            }
            Op::SwitchFocusFloatingTiling => {
                layout.switch_focus_floating_tiling();
            }
            Op::MoveFloatingWindow { id, x, y, animate } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.move_floating_window(id.as_ref(), x, y, animate);
            }
            Op::SetParent {
                id,
                mut new_parent_id,
            } => {
                if !layout.has_window(&id) {
                    return;
                }

                if let Some(parent_id) = new_parent_id {
                    if parent_id_causes_loop(layout, id, parent_id) {
                        new_parent_id = None;
                    }
                }

                let mut update = false;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    if move_.tile.window().0.id == id {
                        move_.tile.window().0.parent_id.set(new_parent_id);
                        update = true;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        'outer: for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == id {
                                        win.0.parent_id.set(new_parent_id);
                                        update = true;
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        'outer: for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == id {
                                    win.0.parent_id.set(new_parent_id);
                                    update = true;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }

                if update {
                    if let Some(new_parent_id) = new_parent_id {
                        layout.descendants_added(&new_parent_id);
                    }
                }
            }
            Op::SetForcedSize { id, size } => {
                for (_mon, win) in layout.windows() {
                    if win.0.id == id {
                        win.0.forced_size.set(size);
                        return;
                    }
                }
            }
            Op::Communicate(id) => {
                let mut update = false;

                if let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move {
                    if move_.tile.window().0.id == id {
                        if move_.tile.window().communicate() {
                            update = true;
                        }

                        if update {
                            // FIXME: serial.
                            layout.update_window(&id, None);
                        }
                        return;
                    }
                }

                match &mut layout.monitor_set {
                    MonitorSet::Normal { monitors, .. } => {
                        'outer: for mon in monitors {
                            for ws in &mut mon.workspaces {
                                for win in ws.windows() {
                                    if win.0.id == id {
                                        if win.communicate() {
                                            update = true;
                                        }
                                        break 'outer;
                                    }
                                }
                            }
                        }
                    }
                    MonitorSet::NoOutputs { workspaces, .. } => {
                        'outer: for ws in workspaces {
                            for win in ws.windows() {
                                if win.0.id == id {
                                    if win.communicate() {
                                        update = true;
                                    }
                                    break 'outer;
                                }
                            }
                        }
                    }
                }

                if update {
                    // FIXME: serial.
                    layout.update_window(&id, None);
                }
            }
            Op::Refresh { is_active } => {
                layout.refresh(is_active);
            }
            Op::AdvanceAnimations { msec_delta } => {
                let mut now = layout.clock.now_unadjusted();
                if msec_delta >= 0 {
                    now = now.saturating_add(Duration::from_millis(msec_delta as u64));
                } else {
                    now = now.saturating_sub(Duration::from_millis(-msec_delta as u64));
                }
                layout.clock.set_unadjusted(now);
                layout.advance_animations();
            }
            Op::CompleteAnimations => {
                layout.clock.set_complete_instantly(true);
                layout.advance_animations();
                layout.clock.set_complete_instantly(false);
            }
            Op::MoveWorkspaceToOutput(id) => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.move_workspace_to_output(&output);
            }
            Op::ViewOffsetGestureBegin {
                output_idx: id,
                workspace_idx,
                is_touchpad: normalize,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.view_offset_gesture_begin(&output, workspace_idx, normalize);
            }
            Op::ViewOffsetGestureUpdate {
                delta,
                timestamp,
                is_touchpad,
            } => {
                layout.view_offset_gesture_update(delta, timestamp, is_touchpad);
            }
            Op::ViewOffsetGestureEnd { is_touchpad } => {
                layout.view_offset_gesture_end(is_touchpad);
            }
            Op::WorkspaceSwitchGestureBegin {
                output_idx: id,
                is_touchpad,
            } => {
                let name = format!("output{id}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };

                layout.workspace_switch_gesture_begin(&output, is_touchpad);
            }
            Op::WorkspaceSwitchGestureUpdate {
                delta,
                timestamp,
                is_touchpad,
            } => {
                layout.workspace_switch_gesture_update(delta, timestamp, is_touchpad);
            }
            Op::WorkspaceSwitchGestureEnd { is_touchpad } => {
                layout.workspace_switch_gesture_end(is_touchpad);
            }
            Op::OverviewGestureBegin => {
                layout.overview_gesture_begin();
            }
            Op::OverviewGestureUpdate { delta, timestamp } => {
                layout.overview_gesture_update(delta, timestamp);
            }
            Op::OverviewGestureEnd => {
                layout.overview_gesture_end();
            }
            Op::InteractiveMoveBegin {
                window,
                output_idx,
                px,
                py,
            } => {
                let name = format!("output{output_idx}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.interactive_move_begin(window, &output, Point::from((px, py)));
            }
            Op::InteractiveMoveUpdate {
                window,
                dx,
                dy,
                output_idx,
                px,
                py,
            } => {
                let name = format!("output{output_idx}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.interactive_move_update(
                    &window,
                    Point::from((dx, dy)),
                    output,
                    Point::from((px, py)),
                );
            }
            Op::InteractiveMoveEnd { window } => {
                layout.interactive_move_end(&window);
            }
            Op::DndUpdate { output_idx, px, py } => {
                let name = format!("output{output_idx}");
                let Some(output) = layout.outputs().find(|o| o.name() == name).cloned() else {
                    return;
                };
                layout.dnd_update(output, Point::from((px, py)));
            }
            Op::DndEnd => {
                layout.dnd_end();
            }
            Op::InteractiveResizeBegin { window, edges } => {
                layout.interactive_resize_begin(window, edges);
            }
            Op::InteractiveResizeUpdate { window, dx, dy } => {
                layout.interactive_resize_update(&window, Point::from((dx, dy)));
            }
            Op::InteractiveResizeEnd { window } => {
                layout.interactive_resize_end(&window);
            }
            Op::ToggleOverview => {
                layout.toggle_overview();
            }
            Op::UpdateConfig { layout_config } => {
                let options = Options {
                    layout: tiri_config::Layout::from_part(&layout_config),
                    ..Default::default()
                };

                layout.update_options(options);
            }
            // Container tree operations (i3-like)
            Op::FocusParent => layout.focus_parent(),
            Op::FocusChild => layout.focus_child(),
            Op::SplitHorizontal => layout.split_horizontal(),
            Op::SplitVertical => layout.split_vertical(),
            Op::SetLayoutSplitH => layout.set_layout_mode(ContainerLayout::SplitH),
            Op::SetLayoutSplitV => layout.set_layout_mode(ContainerLayout::SplitV),
            Op::SetLayoutTabbed => layout.set_layout_mode(ContainerLayout::Tabbed),
            Op::SetLayoutStacked => layout.set_layout_mode(ContainerLayout::Stacked),
            Op::ToggleSplitLayout => layout.toggle_split_layout(),
            Op::ToggleLayoutAll => layout.toggle_layout_all(),
            // Mark operations
            Op::MarkFocused { mark_id, mode } => {
                layout.mark_focused(format!("mark{mark_id}"), mode);
            }
            // Scratchpad operations
            Op::MoveWindowToScratchpad { id } => {
                let id = id.filter(|id| layout.has_window(id));
                layout.move_window_to_scratchpad(id.as_ref());
            }
            Op::ScratchpadShow => layout.scratchpad_show(),
        }
    }
}

fn marks_for(layout: &Layout<TestWindow>, id: usize) -> Vec<String> {
    layout
        .workspaces()
        .find_map(|(_, _, ws)| {
            ws.tiles()
                .find(|tile| *tile.window().id() == id)
                .map(|tile| tile.marks().to_vec())
        })
        .unwrap_or_default()
}

fn set_window_urgent(layout: &mut Layout<TestWindow>, id: usize, urgent: bool) {
    layout.with_windows_mut(|win, _output| {
        if *win.id() == id {
            win.set_urgent(urgent);
        }
    });
}

fn window_layout(layout: &Layout<TestWindow>, id: usize) -> tiri_ipc::WindowLayout {
    let mut found = None;
    layout.with_windows(|win, _output, _ws_id, layout| {
        if *win.id() == id {
            found = Some(layout);
        }
    });
    found.expect("window layout should be present")
}

fn requested_width(layout: &Layout<TestWindow>, id: usize) -> i32 {
    layout
        .windows()
        .find(|(_, win)| *win.id() == id)
        .and_then(|(_, win)| win.requested_size())
        .map(|size| size.w)
        .expect("expected requested size")
}

fn requested_size(layout: &Layout<TestWindow>, id: usize) -> Size<i32, Logical> {
    layout
        .windows()
        .find(|(_, win)| *win.id() == id)
        .and_then(|(_, win)| win.requested_size())
        .expect("expected requested size")
}

fn tile_rect(layout: &Layout<TestWindow>, id: usize) -> Rectangle<f64, Logical> {
    for (_, _, ws) in layout.workspaces() {
        for (tile, pos, _visible) in ws.tiles_with_render_positions() {
            if *tile.window().id() == id {
                return Rectangle::new(pos, tile.tile_size());
            }
        }
    }

    panic!("tile not found for window {id}");
}

fn assert_no_internal_vertical_seams(layout: &Layout<TestWindow>, ids: &[usize]) {
    let mut rects = Vec::new();
    for (_, _, ws) in layout.workspaces() {
        for (tile, pos, visible) in ws.tiles_with_render_positions() {
            if !visible {
                continue;
            }
            if ids.contains(tile.window().id()) {
                rects.push(Rectangle::new(pos, tile.tile_size()));
            }
        }
    }

    assert_eq!(
        rects.len(),
        ids.len(),
        "expected {} visible tiled rects",
        ids.len()
    );
    rects.sort_by(|a, b| a.loc.y.total_cmp(&b.loc.y));

    let eps = 0.001;
    for pair in rects.windows(2) {
        let top = pair[0];
        let bottom = pair[1];
        let seam = bottom.loc.y - (top.loc.y + top.size.h);
        assert!(
            seam.abs() <= eps,
            "found internal vertical seam of {seam} between {:?} and {:?}",
            top,
            bottom
        );
    }
}

#[test]
fn split_vertical_has_no_internal_transparent_seams_with_multiple_windows() {
    let options = Options {
        layout: tiri_config::Layout {
            gaps: 0.,
            border: tiri_config::Border {
                off: false,
                width: 2.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let layout = check_ops_with_options(
        options,
        [
            Op::AddScaledOutput {
                id: 1,
                scale: 1.3,
                layout_config: None,
            },
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::Communicate(1),
            Op::SplitVertical,
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::Communicate(3),
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::Communicate(4),
            Op::AddWindow {
                params: TestWindowParams::new(5),
            },
            Op::Communicate(5),
            Op::Communicate(1),
            Op::Communicate(2),
            Op::Communicate(3),
            Op::Communicate(4),
            Op::Communicate(5),
        ],
    );

    assert_no_internal_vertical_seams(&layout, &[1, 2, 3, 4, 5]);
}

#[test]
fn split_vertical_no_seams_after_tabbed_roundtrip() {
    let options = Options {
        layout: tiri_config::Layout {
            gaps: 0.,
            border: tiri_config::Border {
                off: false,
                width: 2.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let layout = check_ops_with_options(
        options,
        [
            Op::AddScaledOutput {
                id: 1,
                scale: 1.3,
                layout_config: None,
            },
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::Communicate(1),
            Op::SplitVertical,
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::Communicate(3),
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::Communicate(4),
            Op::AddWindow {
                params: TestWindowParams::new(5),
            },
            Op::Communicate(5),
            Op::FocusParent,
            Op::SetLayoutTabbed,
            Op::SetLayoutSplitV,
            Op::Communicate(1),
            Op::Communicate(2),
            Op::Communicate(3),
            Op::Communicate(4),
            Op::Communicate(5),
        ],
    );

    assert_no_internal_vertical_seams(&layout, &[1, 2, 3, 4, 5]);
}

#[test]
fn split_vertical_no_seams_after_stacked_roundtrip() {
    let options = Options {
        layout: tiri_config::Layout {
            gaps: 0.,
            border: tiri_config::Border {
                off: false,
                width: 2.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };

    let layout = check_ops_with_options(
        options,
        [
            Op::AddScaledOutput {
                id: 1,
                scale: 1.3,
                layout_config: None,
            },
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::Communicate(1),
            Op::SplitVertical,
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::Communicate(2),
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::Communicate(3),
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::Communicate(4),
            Op::AddWindow {
                params: TestWindowParams::new(5),
            },
            Op::Communicate(5),
            Op::FocusParent,
            Op::SetLayoutStacked,
            Op::SetLayoutSplitV,
            Op::Communicate(1),
            Op::Communicate(2),
            Op::Communicate(3),
            Op::Communicate(4),
            Op::Communicate(5),
        ],
    );

    assert_no_internal_vertical_seams(&layout, &[1, 2, 3, 4, 5]);
}

#[test]
fn auto_insertion_after_split_preserves_existing_columns() {
    let id1 = 1;
    let id2 = 2;
    let id3 = 3;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(id1),
        },
        Op::Communicate(id1),
        Op::AddWindow {
            params: TestWindowParams::new(id2),
        },
        Op::Communicate(id1),
        Op::Communicate(id2),
        Op::SplitVertical,
        Op::FocusChild,
        Op::AddWindow {
            params: TestWindowParams::new(id3),
        },
        Op::Communicate(id2),
        Op::Communicate(id3),
    ]);

    let pos1 = window_layout(&layout, id1)
        .pos_in_tiling_layout
        .expect("window 1 should be tiled");
    let pos2 = window_layout(&layout, id2)
        .pos_in_tiling_layout
        .expect("window 2 should be tiled");
    let pos3 = window_layout(&layout, id3)
        .pos_in_tiling_layout
        .expect("window 3 should be tiled");

    // Existing windows should stay in distinct root children after the split operation.
    assert_ne!(pos1.0, pos2.0);
    // Auto-inserted window should preserve existing placements rather than collapsing indices.
    assert_ne!(pos3, pos1);
    assert_ne!(pos3, pos2);
}

#[test]
fn ipc_layout_uses_root_child_and_leaf_indices_for_single_window() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ]);

    assert_eq!(window_layout(&layout, 1).pos_in_tiling_layout, Some((1, 1)));
}

#[test]
fn ipc_layout_uses_leaf_index_within_root_child() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::Communicate(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::SplitVertical,
        Op::FocusChild,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::Communicate(2),
        Op::Communicate(3),
    ]);

    let mut positions = vec![
        window_layout(&layout, 1)
            .pos_in_tiling_layout
            .expect("window 1 should be tiled"),
        window_layout(&layout, 2)
            .pos_in_tiling_layout
            .expect("window 2 should be tiled"),
        window_layout(&layout, 3)
            .pos_in_tiling_layout
            .expect("window 3 should be tiled"),
    ];
    positions.sort();

    assert_eq!(positions, vec![(1, 1), (2, 1), (2, 2)]);
}

#[test]
fn auto_add_window_does_not_inherit_floating_from_focused_window() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowFloating {
            id: Some(1),
            floating: true,
        },
        Op::FocusFloating,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&2));
    assert!(window_layout(&layout, 2).pos_in_tiling_layout.is_some());
}

#[test]
fn add_window_next_to_floating_does_not_inherit_floating() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowFloating {
            id: Some(1),
            floating: true,
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(2),
            next_to_id: 1,
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&2));
    assert!(window_layout(&layout, 2).pos_in_tiling_layout.is_some());
}

#[test]
fn add_window_next_to_floating_keeps_explicit_floating() {
    let mut params = TestWindowParams::new(2);
    params.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowFloating {
            id: Some(1),
            floating: true,
        },
        Op::AddWindowNextTo {
            params,
            next_to_id: 1,
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&2));
}

#[test]
fn auto_add_window_inherits_grouped_floating_after_split() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowFloating {
            id: Some(1),
            floating: true,
        },
        Op::FocusFloating,
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(workspace.is_floating(&2));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV)
    );
    assert_eq!(
        workspace.floating().root_layout_for_window(&2),
        Some(ContainerLayout::SplitV)
    );
}

#[test]
fn add_window_next_to_grouped_floating_inherits_group() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowFloating {
            id: Some(1),
            floating: true,
        },
        Op::FocusFloating,
        Op::SplitVertical,
        Op::AddWindowNextTo {
            params: TestWindowParams::new(2),
            next_to_id: 1,
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(workspace.is_floating(&2));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV)
    );
    assert_eq!(
        workspace.floating().root_layout_for_window(&2),
        Some(ContainerLayout::SplitV)
    );
}

#[test]
fn open_window_joins_grouped_floating_even_when_tiling_is_empty() {
    // Sway parity: in floating mode with an explicitly split floating container,
    // opening a regular window should join that floating container even if tiling is empty.
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert_eq!(workspace.tiling().tiles().count(), 0);
        assert_eq!(workspace.floating().tiles().count(), 2);
        assert_eq!(
            workspace.floating().root_layout_for_window(&1),
            Some(ContainerLayout::SplitV)
        );
        assert_eq!(
            workspace.floating().root_layout_for_window(&2),
            Some(ContainerLayout::SplitV)
        );
    }

    check_ops_on_layout(
        &mut layout,
        [Op::FocusParent, Op::ToggleWindowFloating { id: None }],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.floating_is_active());
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 2);
}

#[test]
fn floating_split_after_refocus_targets_refocused_window() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::CompleteAnimations,
    ]);

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindow(1),
            Op::SplitHorizontal,
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::Communicate(4),
            Op::CompleteAnimations,
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&4));

    let r1 = tile_rect(&layout, 1);
    let r2 = tile_rect(&layout, 2);
    let r3 = tile_rect(&layout, 3);
    let r4 = tile_rect(&layout, 4);

    // After refocusing window 1 and splitting horizontally, window 4 should
    // be inserted alongside window 1 (top split), not near the previously
    // focused last window.
    assert!((r4.loc.y - r1.loc.y).abs() <= 1.0);
    assert!(r4.loc.y + 1.0 < r2.loc.y);
    assert!(r4.loc.y + 1.0 < r3.loc.y);
}

#[test]
fn floating_initial_size_is_stable_across_focus_changes_and_width_resize() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
    ]);

    let initial_size = requested_size(&layout, 1);
    assert_eq!(
        initial_size,
        Size::from((640, 540)),
        "first floating request should use the deterministic 50% x 75% preset"
    );

    check_ops_on_layout(&mut layout, [Op::FocusOutput(2), Op::FocusOutput(1)]);

    assert_eq!(
        requested_size(&layout, 1),
        initial_size,
        "output focus changes should not mutate stored initial floating size"
    );

    check_ops_on_layout(
        &mut layout,
        [Op::SetWindowWidth {
            id: Some(1),
            change: SizeChange::SetFixed(500),
        }],
    );

    let resized = requested_size(&layout, 1);
    assert_eq!(resized.w, 500);
    assert_eq!(
        resized.h, initial_size.h,
        "explicit width resize should keep current floating height"
    );
}

fn apply_parity_replay_op(layout: &mut Layout<TestWindow>, op: &str, next_id: &mut usize) {
    match op {
        "focus_left" => layout.focus_left(),
        "focus_right" => layout.focus_right(),
        "focus_up" => layout.focus_up(),
        "focus_down" => layout.focus_down(),
        "split_h" => layout.split_horizontal(),
        "split_v" => layout.split_vertical(),
        "layout_splith" => layout.set_layout_mode(ContainerLayout::SplitH),
        "layout_splitv" => layout.set_layout_mode(ContainerLayout::SplitV),
        "layout_toggle_split" => layout.toggle_split_layout(),
        "layout_tabbed" => layout.set_layout_mode(ContainerLayout::Tabbed),
        "layout_stacked" => layout.set_layout_mode(ContainerLayout::Stacked),
        "focus_parent" => layout.focus_parent(),
        "focus_child" => layout.focus_child(),
        "toggle_floating" => layout.toggle_window_floating(None),
        "toggle_focus_mode" => layout.switch_focus_floating_tiling(),
        "toggle_fullscreen" => {
            if let Some(id) = layout.focus().map(|win| win.id().clone()) {
                layout.toggle_fullscreen(&id);
            }
        }
        "close_focused" => {
            let ids = layout.close_window_ids_for_active_selection();
            for id in ids {
                layout.remove_window(&id, Transaction::new());
            }
        }
        "open_window" => {
            layout.add_window(
                TestWindow::new(TestWindowParams::new(*next_id)),
                AddWindowTarget::Auto,
                None,
                None,
                false,
                false,
                ActivateWindow::default(),
            );
            *next_id += 1;
        }
        _ => panic!("unsupported op in replay: {op}"),
    }
}

#[test]
#[ignore = "parity test invalidated by floating-native fullscreen; tree shape changed"]
fn parity_seed1_step53_replay_includes_floating_roundtrip_shape() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
        ],
    );

    let ops = [
        "focus_up",
        "close_focused",
        "focus_right",
        "split_v",
        "focus_up",
        "toggle_floating",
        "focus_child",
        "focus_child",
        "layout_stacked",
        "split_h",
        "focus_up",
        "toggle_floating",
        "focus_left",
        "layout_stacked",
        "focus_parent",
        "open_window",
        "focus_left",
        "focus_child",
        "layout_splith",
        "split_v",
        "open_window",
        "focus_up",
        "layout_toggle_split",
        "focus_left",
        "focus_left",
        "focus_left",
        "toggle_focus_mode",
        "focus_left",
        "layout_stacked",
        "split_h",
        "focus_parent",
        "focus_left",
        "toggle_focus_mode",
        "split_h",
        "focus_child",
        "toggle_floating",
        "toggle_fullscreen",
        "split_v",
        "layout_tabbed",
        "split_v",
        "split_h",
        "focus_child",
        "layout_splitv",
        "focus_left",
        "focus_parent",
        "toggle_fullscreen",
        "open_window",
        "focus_up",
        "focus_down",
        "open_window",
        "layout_splitv",
        "focus_up",
        "layout_toggle_split",
        "toggle_floating",
    ];

    let mut next_id = 5usize;
    for op in ops {
        match op {
            "focus_left" => layout.focus_left(),
            "focus_right" => layout.focus_right(),
            "focus_up" => layout.focus_up(),
            "focus_down" => layout.focus_down(),
            "split_h" => layout.split_horizontal(),
            "split_v" => layout.split_vertical(),
            "layout_splith" => layout.set_layout_mode(ContainerLayout::SplitH),
            "layout_splitv" => layout.set_layout_mode(ContainerLayout::SplitV),
            "layout_toggle_split" => layout.toggle_split_layout(),
            "layout_tabbed" => layout.set_layout_mode(ContainerLayout::Tabbed),
            "layout_stacked" => layout.set_layout_mode(ContainerLayout::Stacked),
            "focus_parent" => layout.focus_parent(),
            "focus_child" => layout.focus_child(),
            "toggle_floating" => layout.toggle_window_floating(None),
            "toggle_focus_mode" => layout.switch_focus_floating_tiling(),
            "toggle_fullscreen" => {
                if let Some(id) = layout.focus().map(|win| win.id().clone()) {
                    layout.toggle_fullscreen(&id);
                }
            }
            "close_focused" => {
                if let Some(id) = layout.focus().map(|win| win.id().clone()) {
                    layout.remove_window(&id, Transaction::new());
                }
            }
            "open_window" => {
                layout.add_window(
                    TestWindow::new(TestWindowParams::new(next_id)),
                    AddWindowTarget::Auto,
                    None,
                    None,
                    false,
                    false,
                    ActivateWindow::default(),
                );
                next_id += 1;
            }
            _ => panic!("unsupported op in replay: {op}"),
        }
    }

    let workspace = layout.active_workspace().expect("active workspace");
    let raw_tree = workspace.tiling().debug_tree();
    let tree = raw_tree.replace(" *", "");
    assert!(
        !tree.contains("Tabbed"),
        "seed replay should not keep a tabbed wrapper after floating roundtrip:\n{tree}"
    );
    assert!(
        tree.contains("SplitH\n      Window 2\n      SplitH\n        SplitV\n          Window 5"),
        "expected sway-like nested split structure around step 53 replay:\n{tree}"
    );
    assert!(
        raw_tree.contains("SplitV\n          Window 5 *")
            || raw_tree.contains("SplitH\n        SplitV\n          Window 5\n        Window 7 *")
            || raw_tree.contains("Window 5 *"),
        "focus after toggle_floating should stay within the restored subtree:\n{raw_tree}"
    );
}

#[test]
fn parity_seed2_step60_toggle_floating_restores_stacked_subtree_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let mut next_id = 5usize;
    let ops = [
        "focus_right",
        "focus_right",
        "focus_right",
        "layout_tabbed",
        "focus_down",
        "layout_splitv",
        "split_v",
        "open_window",
        "split_h",
        "open_window",
        "focus_left",
        "close_focused",
        "focus_down",
        "focus_parent",
        "open_window",
        "focus_parent",
        "toggle_floating",
        "layout_stacked",
        "toggle_focus_mode",
        "focus_child",
        "toggle_floating",
        "layout_splith",
        "focus_left",
        "focus_left",
        "layout_tabbed",
        "focus_child",
        "layout_toggle_split",
        "layout_stacked",
        "focus_parent",
        "toggle_focus_mode",
        "focus_down",
        "toggle_fullscreen",
        "focus_down",
        "split_v",
        "split_v",
        "focus_left",
        "focus_down",
        "layout_toggle_split",
        "focus_down",
        "focus_up",
        "toggle_floating",
        "toggle_floating",
        "layout_tabbed",
        "toggle_floating",
        "toggle_fullscreen",
        "focus_down",
        "focus_child",
        "focus_parent",
        "toggle_focus_mode",
        "layout_tabbed",
        "open_window",
        "layout_tabbed",
        "layout_tabbed",
        "focus_child",
        "focus_down",
        "focus_parent",
        "focus_child",
        "toggle_focus_mode",
        "split_v",
    ];
    for op in ops {
        apply_parity_replay_op(&mut layout, op, &mut next_id);
    }

    apply_parity_replay_op(&mut layout, "toggle_floating", &mut next_id);

    let ws = layout.active_workspace().expect("active workspace");
    let tree = ws.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.starts_with("Tabbed\n  Window 8\n  Stacked\n    SplitV\n"),
        "step60 toggle_floating should restore the floating subtree under the tabbed workspace root like sway:\n{tree}"
    );
    assert!(
        tree.contains("Stacked\n    SplitV\n      Window 1")
            && tree.contains("    SplitV\n      Window 7"),
        "step60 toggle_floating should restore the stacked subtree with the splitv child holding window 7 like sway:\n{tree}"
    );
    assert_eq!(
        ws.tiling().focus_path(),
        vec![1, 1, 0],
        "step60 focus should land on the restored floating leaf like sway",
    );
}

#[test]
fn parity_seed1_focus_parent_on_single_child_floating_wrapper_keeps_floating_mode() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
        ],
    );

    let ops = [
        "focus_up",
        "close_focused",
        "focus_right",
        "split_v",
        "focus_up",
        "toggle_floating",
        "focus_child",
        "focus_child",
        "layout_stacked",
        "split_h",
        "focus_up",
        "toggle_floating",
        "focus_left",
        "layout_stacked",
        "focus_parent",
        "open_window",
        "focus_left",
        "focus_child",
        "layout_splith",
        "split_v",
        "open_window",
        "focus_up",
        "layout_toggle_split",
        "focus_left",
        "focus_left",
        "focus_left",
        "toggle_focus_mode",
        "focus_left",
        "layout_stacked",
        "split_h",
        "focus_parent",
        "focus_left",
        "toggle_focus_mode",
        "split_h",
        "focus_child",
        "toggle_floating",
        "toggle_fullscreen",
        "split_v",
        "layout_tabbed",
        "split_v",
        "split_h",
        "focus_child",
        "layout_splitv",
        "focus_left",
        "focus_parent",
        "toggle_fullscreen",
        "open_window",
        "focus_up",
        "focus_down",
        "open_window",
        "layout_splitv",
        "focus_up",
        "layout_toggle_split",
        "toggle_floating",
        "focus_parent",
        "toggle_floating",
        "split_h",
        "layout_splitv",
        "layout_splith",
        "open_window",
        "toggle_floating",
        "toggle_floating",
    ];

    let mut next_id = 5usize;
    for op in ops {
        match op {
            "focus_left" => layout.focus_left(),
            "focus_right" => layout.focus_right(),
            "focus_up" => layout.focus_up(),
            "focus_down" => layout.focus_down(),
            "split_h" => layout.split_horizontal(),
            "split_v" => layout.split_vertical(),
            "layout_splith" => layout.set_layout_mode(ContainerLayout::SplitH),
            "layout_splitv" => layout.set_layout_mode(ContainerLayout::SplitV),
            "layout_toggle_split" => layout.toggle_split_layout(),
            "layout_tabbed" => layout.set_layout_mode(ContainerLayout::Tabbed),
            "layout_stacked" => layout.set_layout_mode(ContainerLayout::Stacked),
            "focus_parent" => layout.focus_parent(),
            "focus_child" => layout.focus_child(),
            "toggle_floating" => layout.toggle_window_floating(None),
            "toggle_focus_mode" => layout.switch_focus_floating_tiling(),
            "toggle_fullscreen" => {
                if let Some(id) = layout.focus().map(|win| win.id().clone()) {
                    layout.toggle_fullscreen(&id);
                }
            }
            "close_focused" => {
                if let Some(id) = layout.focus().map(|win| win.id().clone()) {
                    layout.remove_window(&id, Transaction::new());
                }
            }
            "open_window" => {
                layout.add_window(
                    TestWindow::new(TestWindowParams::new(next_id)),
                    AddWindowTarget::Auto,
                    None,
                    None,
                    false,
                    false,
                    ActivateWindow::default(),
                );
                next_id += 1;
            }
            _ => panic!("unsupported op in replay: {op}"),
        }
    }

    let workspace = layout.active_workspace().expect("active workspace");
    let focus_id = layout.focus().map(|w| *w.id());
    assert!(workspace.floating_is_active());
    if let Some(id) = focus_id {
        assert!(!workspace.floating().selected_is_container(Some(&id)));
        assert!(!workspace.floating().wrapper_selected_for_window(&id));
    }

    layout.focus_parent();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.floating_is_active(),
        "focus_parent on this redundant single-child floating wrapper should keep floating mode (sway parity)",
    );
    let focus_id = layout.focus().map(|w| *w.id()).expect("focused window");
    assert!(!workspace.floating().selected_is_container(Some(&focus_id)));
    assert!(!workspace.floating().wrapper_selected_for_window(&focus_id));

    layout.add_window(
        TestWindow::new(TestWindowParams::new(next_id)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace_after_open = layout.active_workspace().expect("active workspace");
    assert!(
        workspace_after_open.floating_is_active(),
        "open_window after focus_parent in this scenario should keep floating mode (sway parity)",
    );
    let focus_id_after_open = layout
        .focus()
        .map(|w| *w.id())
        .expect("focused window after open");
    assert_eq!(
        focus_id_after_open, focus_id,
        "open_window should not steal focus from active floating window in this scenario"
    );
}

#[test]
fn floating_toggle_single_selected_container_moves_to_tiling() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::FocusParent,
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        let focus_id = layout
            .focus()
            .map(|window| *window.id())
            .expect("focused window");
        assert!(
            workspace.floating().selected_is_container(Some(&focus_id)),
            "test precondition: expected floating container selection before toggle"
        );
    }

    layout.toggle_window_floating(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace.floating_is_active(),
        "toggle_floating on a single-window floating container selection should switch to tiling"
    );
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 1);
}

#[test]
fn floating_toggle_multi_window_selected_container_moves_to_tiling() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusParent,
    ]);

    let selected_ids = layout.close_window_ids_for_active_selection();
    assert!(
        selected_ids.len() >= 3,
        "test precondition: expected multi-window floating container selection before toggle"
    );

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        let focus_id = layout
            .focus()
            .map(|window| *window.id())
            .expect("focused window");
        assert!(
            workspace.floating().selected_is_container(Some(&focus_id)),
            "test precondition: expected floating container selection before toggle"
        );
    }

    layout.toggle_window_floating(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace.floating_is_active(),
        "toggle_floating on a multi-window floating container selection should switch to tiling",
    );
    for id in selected_ids {
        assert!(
            !workspace.is_floating(&id),
            "window {id} should be restored to tiling when toggling selected floating container",
        );
    }
}

#[test]
fn floating_toggle_selected_tiling_container_roundtrips_through_workspace_context() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::SplitHorizontal,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusParent,
    ]);

    let tree_before = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .debug_tree()
        .replace(" *", "");
    let selected_ids = layout.close_window_ids_for_active_selection();
    assert_eq!(
        selected_ids,
        vec![3, 4],
        "precondition: focus-parent should select the nested tiling container",
    );

    layout.toggle_window_floating(None);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert_eq!(workspace.tiling().tiles().count(), 2);
        assert_eq!(workspace.floating().tiles().count(), 2);
        for id in &selected_ids {
            assert!(
                workspace.is_floating(id),
                "window {id} should move into the floating container during the first toggle",
            );
        }
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusParent,
            Op::FocusParent,
            Op::ToggleWindowFloating { id: None },
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree_after = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        !workspace.floating_is_active(),
        "toggle_floating from floating workspace-context should restore the subtree to tiling",
    );
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 4);
    assert_eq!(
        tree_after, tree_before,
        "the restored tiling tree should match the original subtree layout after the full roundtrip",
    );
    for id in selected_ids {
        assert!(
            !workspace.is_floating(&id),
            "window {id} should return to tiling after the second toggle",
        );
    }
}

#[test]
fn floating_toggle_workspace_subtree_roundtrips_all_windows_back_to_tiling() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusParent,
        Op::FocusParent,
        Op::FocusParent,
    ]);

    let tree_before = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .debug_tree()
        .replace(" *", "");
    let selected_ids = layout.close_window_ids_for_active_selection();
    assert_eq!(
        selected_ids,
        vec![1, 2, 3],
        "precondition: focus-parent twice should target the whole tiling workspace subtree",
    );

    layout.toggle_window_floating(None);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert_eq!(workspace.tiling().tiles().count(), 0);
        assert_eq!(workspace.floating().tiles().count(), 3);
        for id in &selected_ids {
            assert!(
                workspace.is_floating(id),
                "window {id} should move into the floating workspace subtree during the first toggle",
            );
        }
    }

    check_ops_on_layout(
        &mut layout,
        [Op::FocusParent, Op::ToggleWindowFloating { id: None }],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree_after = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        !workspace.floating_is_active(),
        "unfloating an all-windows workspace subtree should return focus mode to tiling",
    );
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 3);
    assert_eq!(
        tree_after, tree_before,
        "restoring the whole workspace subtree should recover the original tiling tree",
    );
    for id in selected_ids {
        assert!(
            !workspace.is_floating(&id),
            "window {id} should return to tiling after restoring the whole workspace subtree",
        );
    }
}

#[test]
fn floating_single_window_roundtrip_does_not_reintroduce_implicit_split_wrapper() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ]);

    // Defer a split hint on single tiling leaf, then roundtrip through floating.
    layout.split_horizontal();
    layout.toggle_window_floating(None);
    layout.focus_up();
    layout.set_layout_mode(ContainerLayout::Stacked);
    layout.set_layout_mode(ContainerLayout::SplitV);
    layout.toggle_window_floating(None);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(!workspace.floating_is_active());
        assert_eq!(workspace.floating().tiles().count(), 0);
        assert_eq!(workspace.tiling().tiles().count(), 1);
        let tree = workspace.tiling().debug_tree();
        assert!(
            !tree.contains("SplitH")
                && !tree.contains("SplitV")
                && !tree.contains("Tabbed")
                && !tree.contains("Stacked"),
            "floating->tiling roundtrip for a single implicit container should restore a leaf root:\n{tree}",
        );
    }

    // Toggling back to floating should now match sway semantics (no hidden split wrapper in tiling).
    layout.toggle_window_floating(None);
    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.floating_is_active());
    assert_eq!(workspace.floating().tiles().count(), 1);
    assert_eq!(workspace.tiling().tiles().count(), 0);
}

#[test]
fn empty_workspace_layout_commands_do_not_wrap_next_open() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::CloseWindow(1),
    ]);

    layout.set_layout_mode(ContainerLayout::Tabbed);
    layout.focus_child();
    layout.set_layout_mode(ContainerLayout::SplitH);
    layout.toggle_fullscreen(&99999);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 1);
    let tree = workspace.tiling().debug_tree();
    assert!(
        !tree.contains("SplitH")
            && !tree.contains("SplitV")
            && !tree.contains("Tabbed")
            && !tree.contains("Stacked"),
        "open_window after empty-workspace layout commands should create a leaf root:\n{tree}",
    );
}

#[test]
fn empty_workspace_layout_applies_on_second_open() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::Tabbed);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    {
        let workspace = layout.active_workspace().expect("active workspace");
        let tree = workspace.tiling().debug_tree();
        assert!(
            !tree.contains("Tabbed"),
            "first open on empty workspace must still be a leaf root:\n{tree}",
        );
    }

    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.contains("Tabbed"),
        "second open should apply pending empty-workspace layout:\n{tree}",
    );
}

#[test]
fn i3_167_workspace_layout_tabbed_groups_second_open() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::Tabbed);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Tabbed"),
        "workspace layout tabbed should group the second open into a tabbed container:\n{tree}",
    );
}

#[test]
fn i3_167_workspace_layout_stacked_groups_second_open() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::Stacked);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Stacked"),
        "workspace layout stacked should group the second open into a stacked container:\n{tree}",
    );
}

#[test]
fn i3_167_workspace_layout_stacked_reinserts_after_floating_roundtrip() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::Stacked);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.toggle_window_floating(None);
    layout.toggle_window_floating(None);
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Stacked"),
        "workspace layout stacked should still apply after floating roundtrip reinsertion:\n{tree}",
    );
}

#[test]
fn i3_167_empty_workspace_layout_can_switch_back_to_splith() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::Stacked);
    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.remove_window(&1, Transaction::new());
    layout.remove_window(&2, Transaction::new());

    layout.set_layout_mode(ContainerLayout::SplitH);
    layout.add_window(
        TestWindow::new(TestWindowParams::new(3)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(4)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        !tree.contains("Stacked"),
        "after resetting empty workspace layout to splith, new opens should no longer land in stacked:\n{tree}",
    );
}

#[test]
fn i3_167_empty_workspace_layout_can_switch_back_to_splitv() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::Tabbed);
    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.remove_window(&1, Transaction::new());
    layout.remove_window(&2, Transaction::new());

    layout.set_layout_mode(ContainerLayout::SplitV);
    layout.add_window(
        TestWindow::new(TestWindowParams::new(3)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(4)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("SplitV"),
        "after resetting empty workspace layout to splitv, new opens should land in a vertical split:\n{tree}",
    );
    assert!(
        !tree.contains("Tabbed"),
        "after resetting empty workspace layout to splitv, new opens should no longer land in tabbed:\n{tree}",
    );
}

#[test]
fn workspace_split_from_workspace_context_keeps_floating_mode_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::ToggleWindowFloating { id: None },
        Op::FocusParent,
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(
            workspace.floating_is_active(),
            "precondition: floating mode must be active"
        );
        assert!(
            workspace.debug_floating_workspace_context(),
            "precondition: focus_parent on floating leaf should put us in workspace context",
        );
        assert_eq!(workspace.tiling().tiles().count(), 1);
    }

    layout.split_horizontal();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.floating_is_active(),
        "workspace split in floating workspace-context should keep floating mode (sway parity)",
    );
    assert!(
        workspace.debug_floating_workspace_context(),
        "workspace split in this path should keep workspace command context",
    );
}

#[test]
fn empty_workspace_uses_workspace_command_context_like_sway() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert_eq!(
            workspace.debug_command_context(),
            "workspace",
            "empty workspace commands should target workspace context",
        );
    }

    layout.split_horizontal();
    layout.set_layout_mode(ContainerLayout::Tabbed);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.starts_with("Tabbed\n"),
        "empty-workspace commands should persist and apply once tiling appears:\n{tree}",
    );
}

#[test]
fn top_level_leaf_layout_noops_when_matching_workspace_layout_like_sway() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    // Empty-workspace split commands set workspace layout state in sway.
    layout.split_horizontal();
    layout.split_vertical();

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let before = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .debug_tree();
    assert!(
        !before.contains("SplitH")
            && !before.contains("SplitV")
            && !before.contains("Tabbed")
            && !before.contains("Stacked"),
        "precondition: first tiling window should remain a leaf root:\n{before}",
    );

    layout.set_layout_mode(ContainerLayout::SplitV);

    let after = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .debug_tree();
    assert_eq!(
        after, before,
        "layout_splitv on top-level leaf should no-op when workspace layout already is SplitV",
    );
}

#[test]
fn top_level_leaf_toggle_split_uses_workspace_layout_state_like_sway() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    // Seed workspace split layout while empty.
    layout.set_layout_mode(ContainerLayout::SplitH);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let before = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .debug_tree();
    assert!(
        !before.contains("SplitH")
            && !before.contains("SplitV")
            && !before.contains("Tabbed")
            && !before.contains("Stacked"),
        "precondition: single top-level window should be a leaf root:\n{before}",
    );

    layout.toggle_split_layout();

    let workspace = layout.active_workspace().expect("active workspace");
    let after = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        after.starts_with("SplitV\n  Window 1"),
        "toggle_split on top-level leaf should wrap using workspace split state:\n{after}",
    );
}

#[test]
fn workspace_toggle_split_uses_prev_split_layout_like_sway() {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, [Op::AddOutput(1)]);

    layout.set_layout_mode(ContainerLayout::SplitV);
    layout.set_layout_mode(ContainerLayout::Tabbed);
    layout.toggle_split_layout();

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.starts_with("SplitV\n"),
        "layout toggle split from tabbed workspace layout should restore previous split layout:\n{tree}",
    );
}

#[test]
fn single_leaf_stacked_layout_wraps_immediately() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
        ],
    );

    layout.set_layout_mode(ContainerLayout::Stacked);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.starts_with("Stacked\n  Window 1"),
        "layout_stacked on a single tiling leaf should wrap immediately:\n{tree}",
    );
}

#[test]
fn repeated_layout_split_on_nested_single_child_split_is_noop() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
        ],
    );

    // Build sway-like base shape: SplitV(root) -> Window via pending layout + split command.
    layout.set_layout_mode(ContainerLayout::SplitV);
    layout.split_horizontal();

    // First explicit layout_splitv on the focused leaf under preserved single-child root wraps once.
    layout.set_layout_mode(ContainerLayout::SplitV);
    // Repeating it should be a no-op (must not keep nesting splitv wrappers).
    layout.set_layout_mode(ContainerLayout::SplitV);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    let splitv_count = tree.match_indices("SplitV").count();
    assert_eq!(
        splitv_count, 2,
        "repeated layout_splitv should not keep nesting single-child SplitV wrappers:\n{tree}",
    );
}

#[test]
fn layout_splith_on_single_child_preserved_split_stays_flat() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
        ],
    );

    // Seed a preserved single-child SplitH root.
    layout.set_layout_mode(ContainerLayout::SplitH);
    layout.split_horizontal();

    {
        let workspace = layout.active_workspace().expect("active workspace");
        let tree = workspace.tiling().debug_tree().replace(" *", "");
        assert!(
            tree.starts_with("SplitH\n  Window 1"),
            "precondition:\n{tree}"
        );
    }

    layout.set_layout_mode(ContainerLayout::SplitH);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.starts_with("SplitH\n  Window 1"),
        "layout_splith on focused leaf inside preserved single-child SplitH should stay flat:\n{tree}",
    );
}

#[test]
fn closing_tab_in_nested_tabbed_container_keeps_tabbed_parent() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::SetLayoutTabbed,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::CloseWindow(4),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&1));
    assert!(!workspace.is_floating(&2));
    assert!(!workspace.is_floating(&3));

    let r1 = tile_rect(&layout, 1);
    let r2 = tile_rect(&layout, 2);
    let r3 = tile_rect(&layout, 3);

    // Window 1 stays in the split parent lane.
    assert!((r1.loc.x - r2.loc.x).abs() > 1.0);

    // Remaining windows in the nested container must keep tabbed geometry
    // (same content rect), not be flattened into split siblings.
    assert!((r2.loc.x - r3.loc.x).abs() <= 1.0);
    assert!((r2.loc.y - r3.loc.y).abs() <= 1.0);
}

#[test]
fn tiling_focus_parent_on_root_split_sets_workspace_intent_like_sway() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::FocusParent,
        Op::SplitHorizontal,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);

    let r1 = tile_rect(&layout, 1);
    let r2 = tile_rect(&layout, 2);
    let r3 = tile_rect(&layout, 3);

    // Match sway/i3 workspace semantics:
    // splitting at root focus-parent does not immediately reflow existing children;
    // it changes the workspace-level split target used for the next sibling insert.
    assert!((r1.loc.x - r2.loc.x).abs() <= 1.0);
    assert!((r1.loc.y - r2.loc.y).abs() > 1.0);
    assert!((r3.loc.x - r1.loc.x).abs() > 1.0);
    assert!((r3.loc.y - r1.loc.y).abs() <= 1.0);
}

#[test]
fn tiling_selected_parent_controls_new_window_insertion_target() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::FocusParent,
        Op::FocusParent,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let r1 = tile_rect(&layout, 1);
    let r2 = tile_rect(&layout, 2);
    let r3 = tile_rect(&layout, 3);
    let r4 = tile_rect(&layout, 4);

    // Window 4 should be inserted at the selected horizontal parent level,
    // not inside the nested vertical split.
    assert!((r4.loc.y - r2.loc.y).abs() <= 1.0);
    assert!((r4.loc.x - r1.loc.x).abs() > 1.0);
    assert!(r4.loc.y + 1.0 < r3.loc.y);
}

#[test]
fn tiling_focus_parent_once_inserts_as_sibling_of_selected_container() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::FocusParent,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let r1 = tile_rect(&layout, 1);
    let r2 = tile_rect(&layout, 2);
    let r3 = tile_rect(&layout, 3);
    let r4 = tile_rect(&layout, 4);

    // After one focus-parent from window 1, selected container is the nested SplitV.
    // New window should insert as sibling of that container in the root SplitH.
    assert!((r4.loc.y - r2.loc.y).abs() <= 1.0);
    assert!((r4.loc.x - r1.loc.x).abs() > 1.0);
    assert!(r4.loc.y + 1.0 < r3.loc.y);
}

#[test]
fn floating_focus_parent_ignores_redundant_single_child_wrapper() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusParent,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(!workspace.floating().wrapper_selected_for_window(&1));
    assert!(!workspace.floating().selected_is_container(Some(&1)));
    assert!(workspace.floating_is_active());
}

#[test]
fn floating_focus_parent_at_wrapper_keeps_floating_mode() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::SplitVertical,
        Op::FocusParent,
        Op::ToggleWindowFloating { id: None },
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert!(workspace.is_floating(&2));
        assert!(!workspace.is_floating(&1));
    }

    check_ops_on_layout(&mut layout, [Op::FocusParent]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.floating_is_active(),
        "focus_parent at floating wrapper should keep floating mode (sway parity)",
    );
}

#[test]
fn tiling_focus_parent_on_root_inserts_new_window_as_sibling() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::SetLayoutStacked,
        Op::FocusParent,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.contains("SplitH\n  Stacked\n    Window 1\n    Window 2\n    Window 3\n  Window 4")
            || tree.contains(
                "SplitH\n  Window 4\n  Stacked\n    Window 1\n    Window 2\n    Window 3"
            ),
        "expected new window to be inserted as sibling of selected root container:\n{tree}"
    );
}

#[test]
fn tiling_workspace_context_keeps_root_selection_and_focus_child_returns_to_it() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);

    for _ in 0..4 {
        let workspace = layout.active_workspace().expect("active workspace");
        if workspace.debug_handler_context() == "workspace" {
            break;
        }
        check_ops_on_layout(&mut layout, [Op::FocusParent]);
    }

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert_eq!(workspace.debug_handler_context(), "workspace");
        assert!(workspace.is_tiling_workspace_context_active());
        assert!(
            workspace.tiling().selected_is_container(),
            "workspace context should retain the selected root tiling container",
        );
        assert_eq!(workspace.tiling().selected_path(), Vec::<usize>::new());
    }

    check_ops_on_layout(&mut layout, [Op::FocusChild]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.debug_handler_context(), "tiling_container");
    assert!(
        workspace.tiling().selected_is_container(),
        "focus_child from workspace context should return to the remembered root child container",
    );
    assert_eq!(workspace.tiling().selected_path(), vec![1]);
}

#[test]
fn parity_seed2_toggle_fullscreen_keeps_tiling_container_selection() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let mut next_id = 5usize;
    let ops = [
        "focus_right",
        "focus_right",
        "focus_right",
        "layout_tabbed",
        "focus_down",
        "layout_splitv",
        "split_v",
        "open_window",
        "split_h",
        "open_window",
        "focus_left",
        "close_focused",
        "focus_down",
        "focus_parent",
        "open_window",
        "focus_parent",
        "toggle_floating",
        "layout_stacked",
        "toggle_focus_mode",
        "focus_child",
        "toggle_floating",
        "layout_splith",
        "focus_left",
        "focus_left",
        "layout_tabbed",
        "focus_child",
        "layout_toggle_split",
        "layout_stacked",
        "focus_parent",
        "toggle_focus_mode",
        "focus_down",
    ];

    for op in ops {
        apply_parity_replay_op(&mut layout, op, &mut next_id);
    }

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(
            workspace.tiling().selected_is_container(),
            "replay precondition: focus-parent selection must be active before toggle_fullscreen",
        );
    }

    apply_parity_replay_op(&mut layout, "toggle_fullscreen", &mut next_id);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.tiling().selected_is_container(),
        "toggle_fullscreen should not clear the active tiling container selection in this sway parity path",
    );
}

#[test]
fn parity_seed2_step42_toggle_floating_restores_workspace_subtree_to_tiling() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let mut next_id = 5usize;
    let ops = [
        "focus_right",
        "focus_right",
        "focus_right",
        "layout_tabbed",
        "focus_down",
        "layout_splitv",
        "split_v",
        "open_window",
        "split_h",
        "open_window",
        "focus_left",
        "close_focused",
        "focus_down",
        "focus_parent",
        "open_window",
        "focus_parent",
        "toggle_floating",
        "layout_stacked",
        "toggle_focus_mode",
        "focus_child",
        "toggle_floating",
        "layout_splith",
        "focus_left",
        "focus_left",
        "layout_tabbed",
        "focus_child",
        "layout_toggle_split",
        "layout_stacked",
        "focus_parent",
        "toggle_focus_mode",
        "focus_down",
        "toggle_fullscreen",
        "focus_down",
        "split_v",
        "split_v",
        "focus_left",
        "focus_down",
        "layout_toggle_split",
        "focus_down",
        "focus_up",
        "toggle_floating",
        "toggle_floating",
    ];

    for op in ops {
        apply_parity_replay_op(&mut layout, op, &mut next_id);
    }

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace.floating_is_active(),
        "step 42 second toggle_floating should restore focus mode to tiling (sway parity)",
    );
    assert_eq!(
        workspace.floating().tiles().count(),
        0,
        "step 42 second toggle_floating should empty floating workspace subtree",
    );
    assert_eq!(
        workspace.tiling().tiles().count(),
        6,
        "step 42 second toggle_floating should restore all windows to tiling",
    );
}

#[test]
fn parity_seed2_step42_unfloat_from_floating_workspace_context_preserves_workspace_context() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let mut next_id = 5usize;
    let ops = [
        "focus_right",
        "focus_right",
        "focus_right",
        "layout_tabbed",
        "focus_down",
        "layout_splitv",
        "split_v",
        "open_window",
        "split_h",
        "open_window",
        "focus_left",
        "close_focused",
        "focus_down",
        "focus_parent",
        "open_window",
        "focus_parent",
        "toggle_floating",
        "layout_stacked",
        "toggle_focus_mode",
        "focus_child",
        "toggle_floating",
        "layout_splith",
        "focus_left",
        "focus_left",
        "layout_tabbed",
        "focus_child",
        "layout_toggle_split",
        "layout_stacked",
        "focus_parent",
        "toggle_focus_mode",
        "focus_down",
        "toggle_fullscreen",
        "focus_down",
        "split_v",
        "split_v",
        "focus_left",
        "focus_down",
        "layout_toggle_split",
        "focus_down",
        "focus_up",
        "toggle_floating",
        "toggle_floating",
    ];

    for op in ops {
        apply_parity_replay_op(&mut layout, op, &mut next_id);
    }

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.debug_command_context(),
        "workspace",
        "restoring from floating workspace context should keep workspace command context like sway",
    );
}

#[test]
fn parity_seed2_step43_layout_tabbed_wraps_workspace_subtree_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let mut next_id = 5usize;
    let ops = [
        "focus_right",
        "focus_right",
        "focus_right",
        "layout_tabbed",
        "focus_down",
        "layout_splitv",
        "split_v",
        "open_window",
        "split_h",
        "open_window",
        "focus_left",
        "close_focused",
        "focus_down",
        "focus_parent",
        "open_window",
        "focus_parent",
        "toggle_floating",
        "layout_stacked",
        "toggle_focus_mode",
        "focus_child",
        "toggle_floating",
        "layout_splith",
        "focus_left",
        "focus_left",
        "layout_tabbed",
        "focus_child",
        "layout_toggle_split",
        "layout_stacked",
        "focus_parent",
        "toggle_focus_mode",
        "focus_down",
        "toggle_fullscreen",
        "focus_down",
        "split_v",
        "split_v",
        "focus_left",
        "focus_down",
        "layout_toggle_split",
        "focus_down",
        "focus_up",
        "toggle_floating",
        "toggle_floating",
    ];

    for op in ops {
        apply_parity_replay_op(&mut layout, op, &mut next_id);
    }

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert_eq!(
            workspace.debug_command_context(),
            "workspace",
            "step 42 should leave layout commands targeting the workspace like sway",
        );
    }

    layout.set_layout_mode(ContainerLayout::Tabbed);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.starts_with("Tabbed\n  Stacked\n"),
        "workspace-context layout_tabbed should wrap the restored tiling subtree like sway:\n{tree}"
    );
    assert_eq!(
        workspace.tiling().focus_path(),
        vec![0, 1],
        "focus should remain on the same leaf inside the wrapped workspace subtree",
    );
}

#[test]
fn parity_seed2_step50_open_window_targets_tiling_from_floating_workspace_context_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
    ]);

    let mut next_id = 5usize;
    let ops = [
        "focus_right",
        "focus_right",
        "focus_right",
        "layout_tabbed",
        "focus_down",
        "layout_splitv",
        "split_v",
        "open_window",
        "split_h",
        "open_window",
        "focus_left",
        "close_focused",
        "focus_down",
        "focus_parent",
        "open_window",
        "focus_parent",
        "toggle_floating",
        "layout_stacked",
        "toggle_focus_mode",
        "focus_child",
        "toggle_floating",
        "layout_splith",
        "focus_left",
        "focus_left",
        "layout_tabbed",
        "focus_child",
        "layout_toggle_split",
        "layout_stacked",
        "focus_parent",
        "toggle_focus_mode",
        "focus_down",
        "toggle_fullscreen",
        "focus_down",
        "split_v",
        "split_v",
        "focus_left",
        "focus_down",
        "layout_toggle_split",
        "focus_down",
        "focus_up",
        "toggle_floating",
        "toggle_floating",
        "layout_tabbed",
        "toggle_floating",
        "toggle_fullscreen",
        "focus_down",
        "focus_child",
        "focus_parent",
        "toggle_focus_mode",
        "layout_tabbed",
    ];

    for op in ops {
        apply_parity_replay_op(&mut layout, op, &mut next_id);
    }

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(
            workspace.floating_is_active(),
            "precondition: step 49 should still have floating active",
        );
        assert_eq!(workspace.tiling().tiles().count(), 0);
        assert_eq!(workspace.floating().tiles().count(), 6);
        assert_eq!(
            workspace.debug_command_context(),
            "floating",
            "precondition: step 49 should target a floating container path, not workspace",
        );
    }

    layout.add_window(
        TestWindow::new(TestWindowParams::new(next_id)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::default(),
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.tiling().tiles().count(),
        1,
        "open_window from floating workspace context should create tiling like sway",
    );
    assert_eq!(
        workspace.floating().tiles().count(),
        6,
        "open_window from floating workspace context should not join floating subtree",
    );
}

#[test]
fn focus_left_wraps_within_split_container_like_sway() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(1),
        Op::FocusColumnLeft,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));
    assert!(
        tree.contains("Window 3 *"),
        "expected focus to wrap to last child inside current split container:\n{tree}"
    );
}

#[test]
fn i3_101_directional_focus_on_single_window_is_noop() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ]);

    let focused_before = layout.focus().map(|win| *win.id());
    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindowDown,
            Op::FocusWindowUp,
            Op::FocusColumnLeft,
            Op::FocusColumnRight,
        ],
    );

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        focused_before,
        "directional focus should be a no-op when only one tiled window exists",
    );
}

#[test]
fn i3_121_focus_left_right_wraps_across_root_split() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);

    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));

    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(1),
        "focus right should wrap from the rightmost root leaf to the leftmost one",
    );

    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));

    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "focus left should wrap from the leftmost root leaf to the rightmost one",
    );
}

#[test]
fn i3_101_focus_window_command_targets_specific_leaf() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);

    check_ops_on_layout(&mut layout, [Op::FocusWindow(2)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "focus command should activate the requested leaf directly",
    );

    check_ops_on_layout(&mut layout, [Op::FocusWindow(1)]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));
}

#[test]
fn fullscreen_directional_focus_stays_on_active_window_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(3),
        Op::FullscreenWindow(3),
        Op::SplitVertical,
    ]);

    let focused_before = layout.focus().map(|win| *win.id());
    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        focused_before,
        "focus_left should not escape active fullscreen subtree (sway parity)"
    );

    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        focused_before,
        "focus_right should not escape active fullscreen subtree (sway parity)"
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Window 3 *"),
        "focus should remain on the fullscreen window after directional focus:\n{tree}"
    );
}

#[test]
fn fullscreen_focus_parent_is_noop_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(2),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(3),
        Op::FullscreenWindow(3),
    ]);

    check_ops_on_layout(&mut layout, [Op::FocusParent]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));
    assert_eq!(workspace.debug_handler_context(), "tiling_window");
    assert!(
        !workspace.is_tiling_workspace_context_active(),
        "focus_parent should not enter workspace context while fullscreen is active"
    );
    assert!(
        !workspace.tiling().selected_is_container(),
        "focus_parent should not select a tiling container while fullscreen is active"
    );

    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Window 3 *"),
        "focus should remain on the fullscreen window after focus_parent:\n{tree}"
    );
}

#[test]
fn fullscreen_open_window_does_not_steal_focus_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(3),
        Op::FullscreenWindow(3),
        Op::SplitVertical,
    ]);

    let focused_before = layout.focus().map(|win| *win.id());
    layout.add_window(
        TestWindow::new(TestWindowParams::new(4)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        focused_before,
        "open_window should not steal focus from active fullscreen tiling window (sway parity)"
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Window 3 *"),
        "focus should remain on fullscreen window after opening a new tiling window:\n{tree}"
    );
}

#[test]
fn fullscreen_open_then_focus_right_stays_locked_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(3),
        Op::FullscreenWindow(3),
        Op::SplitVertical,
    ]);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(4)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetLayoutTabbed,
            Op::SetLayoutSplitV,
            Op::FocusColumnRight,
        ],
    );

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "focus_right should remain locked on fullscreen tiling window after open/layout ops (sway parity)"
    );
}

#[test]
fn fullscreen_focus_down_can_move_within_fullscreen_subtree_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(3),
        Op::FullscreenWindow(3),
        Op::SplitVertical,
    ]);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(4)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetLayoutTabbed,
            Op::SetLayoutSplitV,
            Op::FocusColumnRight,
            Op::FocusWindowDown,
        ],
    );

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(4),
        "focus_down should move within fullscreen subtree after split/tabbed transitions (sway parity)"
    );

    check_ops_on_layout(&mut layout, [Op::FocusWindowDown]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(4),
        "second focus_down at bottom of fullscreen subtree should be no-op (no wrap, sway parity)"
    );

    layout.add_window(
        TestWindow::new(TestWindowParams::new(5)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(4),
        "open_window should not steal focus even when focus is on non-fullscreen leaf inside fullscreen subtree (sway parity)"
    );
}

#[test]
fn floating_explicit_split_returns_to_tiling_as_container() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::ToggleWindowFloating { id: None },
            Op::SplitHorizontal,
            Op::ToggleWindowFloating { id: None },
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&2));
    let tree_after_return = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree_after_return.contains("SplitH\n  Window 1\n  SplitH\n    Window 2"),
        "floating split should return as nested tiling container:\n{tree_after_return}"
    );

    check_ops_on_layout(
        &mut layout,
        [Op::AddWindow {
            params: TestWindowParams::new(3),
        }],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let tree_after_insert = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree_after_insert.contains("SplitH\n  Window 1\n  SplitH\n    Window 2\n    Window 3")
            || tree_after_insert
                .contains("SplitH\n  Window 1\n  SplitH\n    Window 3\n    Window 2"),
        "new tiling window should insert inside preserved split container:\n{tree_after_insert}"
    );
}

#[test]
fn floating_to_tiling_restore_uses_leaf_reference_as_sibling() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::ToggleWindowFloating { id: Some(3) },
        Op::ToggleWindowFloating { id: Some(3) },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&3));

    layout.activate_window(&1);
    let idx1 = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .focus_path();
    layout.activate_window(&3);
    let idx3 = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .focus_path();
    layout.activate_window(&2);
    let idx2 = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .focus_path();

    assert_eq!(
        idx1.len(),
        1,
        "window 1 should remain a root child: {idx1:?}"
    );
    assert_eq!(
        idx3.len(),
        1,
        "window 3 should be inserted as a root sibling: {idx3:?}"
    );
    assert_eq!(
        idx2.len(),
        1,
        "window 2 should remain a root child: {idx2:?}"
    );
    assert!(
        idx1[0] < idx3[0] && idx3[0] < idx2[0],
        "leaf reference restore should insert after window 1 and before window 2: {idx1:?} {idx3:?} {idx2:?}"
    );
}

#[test]
fn floating_to_tiling_restore_uses_container_reference_as_child() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(1),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(3),
        Op::ToggleWindowFloating { id: Some(3) },
        Op::FocusTiling,
        Op::FocusWindow(1),
        Op::FocusParent,
        Op::ToggleWindowFloating { id: Some(3) },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&3));

    layout.activate_window(&1);
    let path1 = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .focus_path();
    layout.activate_window(&4);
    let path4 = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .focus_path();
    layout.activate_window(&3);
    let path3 = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .focus_path();

    assert!(
        path1.len() == path4.len() && path4.len() == path3.len(),
        "all windows should stay in the same restored container depth: {path1:?} {path4:?} {path3:?}"
    );
    assert_eq!(
        &path1[..path1.len() - 1],
        &path4[..path4.len() - 1],
        "window 4 should remain under the same container as window 1: {path1:?} {path4:?}"
    );
    assert_eq!(
        &path1[..path1.len() - 1],
        &path3[..path3.len() - 1],
        "restored window should be inserted as a child of the selected container: {path1:?} {path3:?}"
    );
    assert!(
        path1[path1.len() - 1] < path4[path4.len() - 1]
            && path4[path4.len() - 1] < path3[path3.len() - 1],
        "container-reference restore should append after existing children (1,4,3): {path1:?} {path4:?} {path3:?}"
    );
}

#[test]
fn floating_stacked_then_split_roundtrip_preserves_container() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusChild,
        Op::FocusChild,
        Op::SetLayoutStacked,
        Op::SplitHorizontal,
        Op::FocusWindowUp,
        Op::ToggleWindowFloating { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&2));
    assert!(!workspace.is_floating(&3));
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    assert!(
        tree.contains("SplitH\n  Window 1\n  SplitH\n    Window 2\n    Window 3")
            || tree.contains("SplitH\n  Window 1\n  SplitH\n    Window 3\n    Window 2"),
        "expected nested split container after floating roundtrip:\n{tree}"
    );
}

#[test]
fn floating_toggle_after_split_marks_container_as_grouped() {
    let mut layout = Layout::default();
    check_ops_on_layout(
        &mut layout,
        [
            Op::AddOutput(1),
            Op::AddWindow {
                params: TestWindowParams::new(1),
            },
            Op::AddWindow {
                params: TestWindowParams::new(2),
            },
            Op::AddWindow {
                params: TestWindowParams::new(3),
            },
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::FocusWindowUp,
            Op::CloseWindow(4),
            Op::FocusColumnRight,
            Op::SplitVertical,
            Op::FocusWindowUp,
            Op::ToggleWindowFloating { id: None },
            Op::FocusChild,
            Op::FocusChild,
            Op::SetLayoutStacked,
            Op::SplitHorizontal,
            Op::FocusWindowUp,
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let floating_id = workspace
        .floating()
        .active_window()
        .expect("floating window should stay active")
        .id()
        .clone();
    assert!(workspace.is_floating(&floating_id));
    assert!(
        workspace.floating_container_allows_splits(&floating_id),
        "floating explicit split should be considered grouped for toggle back"
    );

    check_ops_on_layout(&mut layout, [Op::ToggleWindowFloating { id: None }]);
    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree().replace(" *", "");
    let has_single_leaf_split = tree.contains("\n  SplitH\n    Window ");
    assert!(
        has_single_leaf_split,
        "expected explicit floating split to return as single-leaf split container:\n{tree}"
    );
}

#[test]
fn floating_focus_parent_reaches_wrapper_after_root_in_nested_tree() {
    let mut params2 = TestWindowParams::new(2);
    params2.is_floating = true;
    let mut params3 = TestWindowParams::new(3);
    params3.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow { params: params2 },
        Op::FocusWindow(1),
        Op::SplitHorizontal,
        Op::AddWindow { params: params3 },
        Op::FocusWindow(1),
        Op::FocusParent,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(!workspace.floating().wrapper_selected_for_window(&1));

    let mut layout = layout;
    check_ops_on_layout(&mut layout, [Op::FocusParent]);
    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.floating().wrapper_selected_for_window(&1));
}

#[test]
fn floating_focus_child_exits_wrapper_selection() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusParent,
        Op::FocusParent,
        Op::FocusChild,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(!workspace.floating().wrapper_selected_for_window(&1));
}

#[test]
fn floating_split_with_wrapper_selected_changes_root_layout() {
    let mut params2 = TestWindowParams::new(2);
    params2.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow { params: params2 },
        Op::FocusWindow(1),
        Op::FocusParent,
        Op::FocusParent,
        Op::SplitHorizontal,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(workspace.is_floating(&2));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitH)
    );
}

#[test]
fn floating_set_layout_mode_on_wrapper_is_noop_like_sway() {
    let mut params2 = TestWindowParams::new(2);
    params2.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow { params: params2 },
        Op::FocusParent,
        Op::FocusParent,
        Op::SetLayoutTabbed,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(workspace.is_floating(&2));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV)
    );
}

#[test]
fn floating_consume_into_column_uses_floating_tree() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::ConsumeWindowIntoColumn,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV)
    );
}

#[test]
fn floating_expel_from_column_uses_floating_tree() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::ExpelWindowFromColumn,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitH)
    );
}

#[test]
fn consume_or_expel_targeting_floating_window_does_not_use_tiling_tree() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: Some(1) },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(!workspace.is_floating(&2));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV)
    );
    assert!(window_layout(&layout, 2).pos_in_tiling_layout.is_some());
}

#[test]
fn floating_toggle_column_tabbed_display_changes_floating_layout() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::ToggleColumnTabbedDisplay,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::Tabbed)
    );
}

#[test]
fn floating_tab_bar_hit_does_not_report_resize_edges() {
    let mut layout = Layout::default();
    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.toggle_window_floating(None);
    layout.split_vertical();
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::NextTo(&1),
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.toggle_column_tabbed_display();

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.is_floating(&1));
        assert!(workspace.is_floating(&2));
        assert_eq!(
            workspace.floating().root_layout_for_window(&1),
            Some(ContainerLayout::Tabbed)
        );
    }

    let rect = tile_rect(&layout, 2);
    let mut tab_pos = None;
    for dy in 1..96 {
        for frac in [0.2, 0.5, 0.8] {
            let candidate = rect.loc + Point::from((rect.size.w * frac, -(dy as f64)));
            if matches!(
                layout.window_under(&output, candidate),
                Some((
                    _,
                    HitType::Activate {
                        is_tab_indicator: true
                    }
                ))
            ) {
                tab_pos = Some(candidate);
                break;
            }
        }
        if tab_pos.is_some() {
            break;
        }
    }

    let tab_pos = tab_pos.expect("expected a tab-bar hit position above floating tile");
    assert_eq!(layout.resize_edges_under(&output, tab_pos), None);

    let mut tab_pos_top = None;
    for dy in (1..96).rev() {
        for frac in [0.2, 0.5, 0.8] {
            let candidate = rect.loc + Point::from((rect.size.w * frac, -(dy as f64)));
            if matches!(
                layout.window_under(&output, candidate),
                Some((
                    _,
                    HitType::Activate {
                        is_tab_indicator: true
                    }
                ))
            ) {
                tab_pos_top = Some(candidate);
                break;
            }
        }
        if tab_pos_top.is_some() {
            break;
        }
    }

    let tab_pos_top = tab_pos_top.expect("expected a top tab-bar hit position above floating tile");
    assert_eq!(layout.resize_edges_under(&output, tab_pos_top), None);
}

#[test]
fn floating_tab_bar_hit_does_not_fall_through_to_tiling_window() {
    let mut layout = Layout::default();
    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.toggle_window_floating(None);
    layout.split_vertical();
    layout.add_window(
        TestWindow::new(TestWindowParams::new(3)),
        AddWindowTarget::NextTo(&2),
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.toggle_column_tabbed_display();

    let rect = tile_rect(&layout, 3);
    let mut hit = None;
    for dy in 1..96 {
        for frac in [0.2, 0.5, 0.8] {
            let candidate = rect.loc + Point::from((rect.size.w * frac, -(dy as f64)));
            if let Some((
                win,
                HitType::Activate {
                    is_tab_indicator: true,
                },
            )) = layout.window_under(&output, candidate)
            {
                if *win.id() != 1 {
                    hit = Some((candidate, *win.id()));
                    break;
                }
            }
        }
        if hit.is_some() {
            break;
        }
    }

    let (candidate, id) = hit.expect("expected floating tab bar hit to capture pointer");
    assert_ne!(
        id, 1,
        "tab bar hit must not fall through to tiling window below"
    );
    assert_eq!(layout.resize_edges_under(&output, candidate), None);
}

#[test]
fn scratchpad_show_hides_focused_window() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params2 = TestWindowParams::new(2);
    let id2 = params2.id;
    layout.add_window(
        TestWindow::new(params2),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    layout.move_window_to_scratchpad(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id1));
    assert!(!workspace.has_window(&id2));

    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id2));
    assert!(workspace.is_floating(&id2));
    assert_eq!(workspace.active_window().unwrap().id(), &id2);

    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&id2));
}

#[test]
fn scratchpad_show_moves_visible_between_outputs() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output_a = make_test_output("output-a");
    let output_b = make_test_output("output-b");
    layout.add_output(output_a.clone(), None);
    layout.add_output(output_b.clone(), None);

    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    layout.move_window_to_scratchpad(None);
    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id1));
    assert!(workspace.is_floating(&id1));

    layout.focus_output(&output_b);
    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id1));
    assert!(workspace.is_floating(&id1));
}

#[test]
fn scratchpad_multiple_windows_round_robin() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add 3 windows
    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params2 = TestWindowParams::new(2);
    let id2 = params2.id;
    layout.add_window(
        TestWindow::new(params2),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params3 = TestWindowParams::new(3);
    let id3 = params3.id;
    layout.add_window(
        TestWindow::new(params3),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    // Move all 3 windows to scratchpad
    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id1));
    layout.move_window_to_scratchpad(None);

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id2));
    layout.move_window_to_scratchpad(None);

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id3));
    layout.move_window_to_scratchpad(None);

    // No windows visible in workspace
    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&id1));
    assert!(!workspace.has_window(&id2));
    assert!(!workspace.has_window(&id3));

    // Show scratchpad - first window should appear (round robin order depends on implementation)
    layout.scratchpad_show();
    let workspace = layout.active_workspace().expect("active workspace");
    // At least one window should be visible
    assert!(workspace.has_window(&id1) || workspace.has_window(&id2) || workspace.has_window(&id3));
}

#[test]
fn scratchpad_from_floating_preserves_floating() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add a window and make it floating
    let params = TestWindowParams::new(1);
    let id = params.id;
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    // Set as floating
    layout.set_window_floating(Some(&id), true);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&id));

    // Move to scratchpad
    layout.move_window_to_scratchpad(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&id));

    // Show from scratchpad - should appear as floating
    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id));
    assert!(workspace.is_floating(&id));
}

#[test]
fn scratchpad_from_tiling_becomes_floating() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add a tiling window
    let params = TestWindowParams::new(1);
    let id = params.id;
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&id));

    // Move to scratchpad
    layout.move_window_to_scratchpad(None);

    // Show from scratchpad - should appear as floating (scratchpad windows are always floating)
    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id));
    assert!(workspace.is_floating(&id));
}

#[test]
fn scratchpad_move_without_outputs_cleans_up_empty_workspace() {
    let layout = check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::MoveWindowToScratchpad { id: Some(4) },
    ]);

    let MonitorSet::NoOutputs { workspaces } = layout.monitor_set else {
        unreachable!()
    };

    assert!(workspaces.is_empty());
}

#[test]
fn move_window_to_workspace_ignores_hidden_scratchpad_window() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
        Op::MoveWindowUpOrToWorkspaceUp,
        Op::FocusWorkspacePrevious,
        Op::MoveWindowToScratchpad { id: None },
        Op::MoveWindowToWorkspace {
            window_id: Some(5),
            workspace_idx: 0,
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&5));
}

#[test]
fn scratchpad_show_keeps_empty_workspace_tail() {
    let layout = check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::MoveWindowToScratchpad { id: None },
        Op::FocusWorkspace(1),
        Op::ScratchpadShow,
    ]);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let monitor = monitors.into_iter().next().unwrap();
    assert!(!monitor.workspaces.last().unwrap().has_windows());
}

#[test]
fn scratchpad_show_after_move_to_workspace_cleans_empty_non_active_workspace() {
    let layout = check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::MoveWindowToScratchpad { id: None },
        Op::ScratchpadShow,
        Op::MoveColumnToWorkspace(1, false),
        Op::ScratchpadShow,
    ]);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let monitor = monitors.into_iter().next().unwrap();
    for (idx, ws) in monitor.workspaces.iter().enumerate().skip(1) {
        if idx != monitor.active_workspace_idx && idx != monitor.workspaces.len() - 1 {
            assert!(
                ws.has_windows_or_name(),
                "workspace {idx} should not be left empty and unnamed"
            );
        }
    }
}

#[test]
fn move_window_to_scratchpad_during_interactive_move_doesnt_panic_on_refresh() {
    let layout = check_ops([
        Op::AddScaledOutput {
            id: 1,
            scale: 1.0,
            layout_config: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::InteractiveMoveBegin {
            window: 3,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::MoveWindowToScratchpad { id: None },
        Op::Refresh { is_active: false },
    ]);

    assert!(layout.workspaces().all(|(_, _, ws)| !ws.has_window(&3)));
}

#[test]
fn move_window_to_scratchpad_during_interactive_move_update_doesnt_panic() {
    let layout = check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(4),
        Op::MoveWindowUpOrToWorkspaceUp,
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 4,
            px: 0.0,
            py: 0.0,
        },
        Op::FocusWorkspaceUp,
        Op::MoveWindowToScratchpad { id: None },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 0.0,
            output_idx: 4,
            px: 0.0,
            py: 0.0,
        },
    ]);

    assert!(layout.workspaces().all(|(_, _, ws)| !ws.has_window(&2)));
}

#[test]
fn interactive_move_begin_ignores_hidden_tabbed_window() {
    let layout = check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddScaledOutput {
            id: 4,
            scale: 1.0,
            layout_config: None,
        },
        Op::AddWindowNextTo {
            params: TestWindowParams {
                id: 3,
                is_floating: true,
                ..TestWindowParams::new(3)
            },
            next_to_id: 2,
        },
        Op::SplitHorizontal,
        Op::SetLayoutTabbed,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::InteractiveMoveBegin {
            window: 3,
            output_idx: 4,
            px: 0.0,
            py: 0.0,
        },
    ]);

    assert!(layout.has_window(&3));
}

#[test]
fn move_to_scratchpad_cleans_empty_non_active_workspace() {
    let layout = check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(1),
        Op::MoveWindowToWorkspaceDown(false),
        Op::FocusWorkspaceAutoBackAndForth(0),
        Op::MoveWindowToScratchpad { id: Some(2) },
    ]);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let monitor = monitors.into_iter().next().unwrap();
    let last_idx = monitor.workspaces.len() - 1;
    for (idx, workspace) in monitor.workspaces.iter().enumerate() {
        if idx != monitor.active_workspace_idx && idx != last_idx {
            assert!(workspace.has_windows_or_name());
        }
    }
}

#[test]
fn toggle_window_floating_after_output_attach_keeps_options_synced() {
    check_ops([
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::FocusParent,
        Op::ToggleWindowFloating { id: None },
    ]);
}

#[test]
fn move_window_to_workspace_up_after_maximize_keeps_floating_normal() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams {
                id: 3,
                is_floating: true,
                ..TestWindowParams::new(3)
            },
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 1,
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::MoveWindowToWorkspaceUp(false),
    ];

    let layout = check_ops(ops);

    let monitor = match layout.monitor_set {
        MonitorSet::Normal { monitors, .. } => monitors.into_iter().next().unwrap(),
        MonitorSet::NoOutputs { .. } => unreachable!(),
    };

    // Window 1 was maximized before the move and should stay in tiling (not floating).
    let ws0 = &monitor.workspaces[0];
    assert!(ws0.tiling().tiles().any(|tile| tile.window().id() == &1));
    assert!(!ws0.floating().tiles().any(|tile| tile.window().id() == &1));
}

#[test]
fn sticky_toggle_requires_floating() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params = TestWindowParams::new(1);
    let id = params.id;
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    layout.toggle_window_sticky(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id));
    assert!(!window_layout(&layout, id).is_sticky);
}

#[test]
fn sticky_moves_across_workspaces_on_output() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params = TestWindowParams::new(1);
    let id = params.id;
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    layout.set_window_floating(Some(&id), true);
    layout.toggle_window_sticky(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&id));
    assert!(window_layout(&layout, id).is_sticky);

    layout.switch_workspace(1);
    let active_ws_id = layout.active_workspace().expect("active workspace").id();

    assert!(window_layout(&layout, id).is_sticky);

    // Ensure sticky window reports the active workspace id.
    let mut reported_ws = None;
    layout.with_windows(|win, _output, ws_id, _layout| {
        if *win.id() == id {
            reported_ws = ws_id;
        }
    });
    assert_eq!(reported_ws, Some(active_ws_id));

    layout.toggle_window_sticky(Some(&id));
    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id));
    assert!(!window_layout(&layout, id).is_sticky);
}
#[test]
fn scratchpad_show_hides_visible_then_shows_next() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add 2 windows
    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params2 = TestWindowParams::new(2);
    let id2 = params2.id;
    layout.add_window(
        TestWindow::new(params2),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    // Move both to scratchpad
    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id1));
    layout.move_window_to_scratchpad(None);

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id2));
    layout.move_window_to_scratchpad(None);

    // Show first scratchpad window
    layout.scratchpad_show();
    let workspace = layout.active_workspace().expect("active workspace");
    let first_visible = if workspace.has_window(&id1) {
        id1.clone()
    } else {
        id2.clone()
    };
    assert!(workspace.has_window(&first_visible));

    // Call scratchpad_show again - should hide current and show the other
    layout.scratchpad_show();
    let workspace = layout.active_workspace().expect("active workspace");
    // First window should be hidden now
    assert!(!workspace.has_window(&first_visible));
}

#[test]
fn scratchpad_fullscreen_to_scratchpad() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add a window
    let params = TestWindowParams::new(1);
    let id = params.id;
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    // Make fullscreen
    layout.set_fullscreen(&id, true);

    // Move to scratchpad
    layout.move_window_to_scratchpad(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&id));

    // Show from scratchpad - should appear as floating
    layout.scratchpad_show();

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&id));
    assert!(workspace.is_floating(&id));
}

#[test]
fn marks_replace_add_toggle() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params2 = TestWindowParams::new(2);
    let id2 = params2.id;
    layout.add_window(
        TestWindow::new(params2),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id1));

    layout.mark_focused(String::from("one"), MarkMode::Replace);
    assert_eq!(marks_for(&layout, id1), vec![String::from("one")]);

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id2));

    layout.mark_focused(String::from("one"), MarkMode::Add);
    assert!(marks_for(&layout, id1).is_empty());
    assert_eq!(marks_for(&layout, id2), vec![String::from("one")]);

    layout.mark_focused(String::from("one"), MarkMode::Toggle);
    assert!(marks_for(&layout, id2).is_empty());
}

#[test]
fn marks_multiple_on_same_window() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    // Add multiple marks to the same window
    layout.mark_focused(String::from("mark_a"), MarkMode::Add);
    layout.mark_focused(String::from("mark_b"), MarkMode::Add);
    layout.mark_focused(String::from("mark_c"), MarkMode::Add);

    let marks = marks_for(&layout, id1);
    assert!(marks.contains(&String::from("mark_a")));
    assert!(marks.contains(&String::from("mark_b")));
    assert!(marks.contains(&String::from("mark_c")));
    assert_eq!(marks.len(), 3);
}

#[test]
fn marks_unique_across_windows() {
    // When using Replace mode, mark moves from old window to new window
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params2 = TestWindowParams::new(2);
    let id2 = params2.id;
    layout.add_window(
        TestWindow::new(params2),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    // Add mark to window 1
    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id1));
    layout.mark_focused(String::from("unique_mark"), MarkMode::Replace);
    assert_eq!(marks_for(&layout, id1), vec![String::from("unique_mark")]);

    // Focus window 2 and add the same mark - should move from window 1 to window 2
    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id2));
    layout.mark_focused(String::from("unique_mark"), MarkMode::Replace);

    // Mark should now be only on window 2, not on window 1
    assert!(marks_for(&layout, id1).is_empty());
    assert_eq!(marks_for(&layout, id2), vec![String::from("unique_mark")]);
}

#[test]
fn unmark_removes_specific_mark_and_clears_focused_window_marks() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    let params1 = TestWindowParams::new(1);
    let id1 = params1.id;
    layout.add_window(
        TestWindow::new(params1),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let params2 = TestWindowParams::new(2);
    let id2 = params2.id;
    layout.add_window(
        TestWindow::new(params2),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id1));
    layout.mark_focused(String::from("alpha"), MarkMode::Replace);
    layout.mark_focused(String::from("beta"), MarkMode::Add);

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id2));
    layout.mark_focused(String::from("gamma"), MarkMode::Replace);

    layout.unmark(Some("alpha"));
    assert_eq!(marks_for(&layout, id1), vec![String::from("beta")]);
    assert_eq!(marks_for(&layout, id2), vec![String::from("gamma")]);

    let workspace = layout.active_workspace_mut().expect("active workspace");
    assert!(workspace.focus_window_by_id(&id1));
    layout.unmark(None);

    assert!(marks_for(&layout, id1).is_empty());
    assert_eq!(marks_for(&layout, id2), vec![String::from("gamma")]);
}

#[test]
fn urgent_propagates_to_workspace() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetLayoutTabbed,
    ]);

    set_window_urgent(&mut layout, 1, true);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.is_urgent(),
        "workspace should reflect urgent child state"
    );

    set_window_urgent(&mut layout, 1, false);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace.is_urgent(),
        "workspace urgency should clear when urgent flag is removed",
    );
}

#[track_caller]
fn check_ops_on_layout(layout: &mut Layout<TestWindow>, ops: impl IntoIterator<Item = Op>) {
    for op in ops {
        op.apply(layout);
        layout.verify_invariants();
    }
}

#[track_caller]
fn check_ops(ops: impl IntoIterator<Item = Op>) -> Layout<TestWindow> {
    let mut layout = Layout::default();
    check_ops_on_layout(&mut layout, ops);
    layout
}

#[track_caller]
fn check_ops_with_options(
    options: Options,
    ops: impl IntoIterator<Item = Op>,
) -> Layout<TestWindow> {
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);
    check_ops_on_layout(&mut layout, ops);
    layout
}

#[test]
fn operations_dont_panic() {
    if std::env::var_os("RUN_SLOW_TESTS").is_none() {
        eprintln!("ignoring slow test");
        return;
    }

    let every_op = [
        Op::AddOutput(0),
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::RemoveOutput(0),
        Op::RemoveOutput(1),
        Op::RemoveOutput(2),
        Op::FocusOutput(0),
        Op::FocusOutput(1),
        Op::FocusOutput(2),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(1),
            layout_config: None,
        },
        Op::UnnameWorkspace { ws_name: 1 },
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(2),
            next_to_id: 1,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(3),
            ws_name: 1,
        },
        Op::CloseWindow(0),
        Op::CloseWindow(1),
        Op::CloseWindow(2),
        Op::FullscreenWindow(1),
        Op::FullscreenWindow(2),
        Op::FullscreenWindow(3),
        Op::MaximizeWindowToEdges { id: Some(1) },
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::MaximizeWindowToEdges { id: Some(3) },
        Op::FocusColumnLeft,
        Op::FocusColumnRight,
        Op::FocusColumnRightOrFirst,
        Op::FocusColumnLeftOrLast,
        Op::FocusWindowOrMonitorUp(0),
        Op::FocusWindowOrMonitorDown(1),
        Op::FocusColumnOrMonitorLeft(0),
        Op::FocusColumnOrMonitorRight(1),
        Op::FocusWindowUp,
        Op::FocusWindowUpOrColumnLeft,
        Op::FocusWindowUpOrColumnRight,
        Op::FocusWindowOrWorkspaceUp,
        Op::FocusWindowDown,
        Op::FocusWindowDownOrColumnLeft,
        Op::FocusWindowDownOrColumnRight,
        Op::FocusWindowOrWorkspaceDown,
        Op::MoveColumnLeft,
        Op::MoveColumnRight,
        Op::MoveColumnLeftOrToMonitorLeft(0),
        Op::MoveColumnRightOrToMonitorRight(1),
        Op::ConsumeWindowIntoColumn,
        Op::ExpelWindowFromColumn,
        Op::CenterColumn,
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceUp,
        Op::FocusWorkspace(1),
        Op::FocusWorkspace(2),
        Op::MoveWindowToWorkspaceDown(true),
        Op::MoveWindowToWorkspaceUp(true),
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 1,
        },
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 2,
        },
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceUp(true),
        Op::MoveColumnToWorkspace(1, true),
        Op::MoveColumnToWorkspace(2, true),
        Op::MoveWindowDown,
        Op::MoveWindowDownOrToWorkspaceDown,
        Op::MoveWindowUp,
        Op::MoveWindowUpOrToWorkspaceUp,
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ConsumeOrExpelWindowRight { id: None },
        Op::MoveWorkspaceToOutput(1),
        Op::ToggleColumnTabbedDisplay,
    ];

    for third in &every_op {
        for second in &every_op {
            for first in &every_op {
                // eprintln!("{first:?}, {second:?}, {third:?}");

                let mut layout = Layout::default();
                first.clone().apply(&mut layout);
                layout.verify_invariants();
                second.clone().apply(&mut layout);
                layout.verify_invariants();
                third.clone().apply(&mut layout);
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn operations_from_starting_state_dont_panic() {
    if std::env::var_os("RUN_SLOW_TESTS").is_none() {
        eprintln!("ignoring slow test");
        return;
    }

    // Running every op from an empty state doesn't get us to all the interesting states. So,
    // also run it from a manually-created starting state with more things going on to exercise
    // more code paths.
    let setup_ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MoveWindowToWorkspaceDown(true),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusColumnLeft,
        Op::ConsumeWindowIntoColumn,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
        Op::MoveWindowToOutput {
            window_id: None,
            output_id: 2,
            target_ws_idx: None,
        },
        Op::FocusOutput(1),
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::Communicate(4),
        Op::Communicate(5),
    ];

    let every_op = [
        Op::AddOutput(0),
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::RemoveOutput(0),
        Op::RemoveOutput(1),
        Op::RemoveOutput(2),
        Op::FocusOutput(0),
        Op::FocusOutput(1),
        Op::FocusOutput(2),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(1),
            layout_config: None,
        },
        Op::UnnameWorkspace { ws_name: 1 },
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(6),
            next_to_id: 0,
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(7),
            next_to_id: 1,
        },
        Op::AddWindowToNamedWorkspace {
            params: TestWindowParams::new(5),
            ws_name: 1,
        },
        Op::CloseWindow(0),
        Op::CloseWindow(1),
        Op::CloseWindow(2),
        Op::FullscreenWindow(1),
        Op::FullscreenWindow(2),
        Op::FullscreenWindow(3),
        Op::MaximizeWindowToEdges { id: Some(1) },
        Op::MaximizeWindowToEdges { id: Some(2) },
        Op::MaximizeWindowToEdges { id: Some(3) },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: false,
        },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: true,
        },
        Op::SetFullscreenWindow {
            window: 2,
            is_fullscreen: false,
        },
        Op::SetFullscreenWindow {
            window: 2,
            is_fullscreen: true,
        },
        Op::FocusColumnLeft,
        Op::FocusColumnRight,
        Op::FocusColumnRightOrFirst,
        Op::FocusColumnLeftOrLast,
        Op::FocusWindowOrMonitorUp(0),
        Op::FocusWindowOrMonitorDown(1),
        Op::FocusColumnOrMonitorLeft(0),
        Op::FocusColumnOrMonitorRight(1),
        Op::FocusWindowUp,
        Op::FocusWindowUpOrColumnLeft,
        Op::FocusWindowUpOrColumnRight,
        Op::FocusWindowOrWorkspaceUp,
        Op::FocusWindowDown,
        Op::FocusWindowDownOrColumnLeft,
        Op::FocusWindowDownOrColumnRight,
        Op::FocusWindowOrWorkspaceDown,
        Op::MoveColumnLeft,
        Op::MoveColumnRight,
        Op::MoveColumnLeftOrToMonitorLeft(0),
        Op::MoveColumnRightOrToMonitorRight(1),
        Op::ConsumeWindowIntoColumn,
        Op::ExpelWindowFromColumn,
        Op::CenterColumn,
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceUp,
        Op::FocusWorkspace(1),
        Op::FocusWorkspace(2),
        Op::FocusWorkspace(3),
        Op::MoveWindowToWorkspaceDown(true),
        Op::MoveWindowToWorkspaceUp(true),
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 1,
        },
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 2,
        },
        Op::MoveWindowToWorkspace {
            window_id: None,
            workspace_idx: 3,
        },
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceUp(true),
        Op::MoveColumnToWorkspace(1, true),
        Op::MoveColumnToWorkspace(2, true),
        Op::MoveColumnToWorkspace(3, true),
        Op::MoveWindowDown,
        Op::MoveWindowDownOrToWorkspaceDown,
        Op::MoveWindowUp,
        Op::MoveWindowUpOrToWorkspaceUp,
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::ConsumeOrExpelWindowRight { id: None },
        Op::ToggleColumnTabbedDisplay,
    ];

    for third in &every_op {
        for second in &every_op {
            for first in &every_op {
                // eprintln!("{first:?}, {second:?}, {third:?}");

                let mut layout = Layout::default();
                for op in &setup_ops {
                    op.clone().apply(&mut layout);
                }

                let mut layout = Layout::default();
                first.clone().apply(&mut layout);
                layout.verify_invariants();
                second.clone().apply(&mut layout);
                layout.verify_invariants();
                third.clone().apply(&mut layout);
                layout.verify_invariants();
            }
        }
    }
}

#[test]
fn primary_active_workspace_idx_not_updated_on_output_add() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(2),
        Op::FocusWorkspace(3),
        Op::AddOutput(2),
    ];

    check_ops(ops);
}

#[test]
fn window_closed_on_previous_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusWorkspaceDown,
        Op::CloseWindow(0),
    ];

    check_ops(ops);
}

#[test]
fn removing_output_must_keep_empty_focus_on_primary() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    // The workspace from the removed output was inserted at position 0, so the active workspace
    // must change to 1 to keep the focus on the empty workspace.
    assert_eq!(monitors[0].active_workspace_idx, 1);
}

#[test]
fn move_to_workspace_by_idx_does_not_leave_empty_workspaces() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(1),
        Op::MoveWindowToWorkspace {
            window_id: Some(0),
            workspace_idx: 2,
        },
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert!(monitors[0].workspaces[1].has_windows());
}

#[test]
fn empty_workspaces_dont_move_back_to_original_output() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspace(1),
        Op::CloseWindow(1),
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn named_workspaces_dont_update_original_output_on_adding_window() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        // Adding a window updates the original output for unnamed workspaces.
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        // Connecting the previous output should move the named workspace back since its
        // original output wasn't updated.
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);
    let (mon, _, ws) = layout
        .workspaces()
        .find(|(_, _, ws)| ws.name().is_some())
        .unwrap();
    assert!(ws.name().is_some()); // Sanity check.
    let mon = mon.unwrap();
    assert_eq!(mon.output_name(), "output1");
}

#[test]
fn workspaces_update_original_output_on_moving_to_same_output() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        Op::MoveWorkspaceToOutput(2),
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);
    let (mon, _, ws) = layout
        .workspaces()
        .find(|(_, _, ws)| ws.name().is_some())
        .unwrap();
    assert!(ws.name().is_some()); // Sanity check.
    let mon = mon.unwrap();
    assert_eq!(mon.output_name(), "output2");
}

#[test]
fn workspaces_update_original_output_on_moving_to_same_monitor() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 1,
            ws_name: None,
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        Op::MoveWorkspaceToMonitor {
            ws_name: Some(1),
            output_id: 2,
        },
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);
    let (mon, _, ws) = layout
        .workspaces()
        .find(|(_, _, ws)| ws.name().is_some())
        .unwrap();
    assert!(ws.name().is_some()); // Sanity check.
    let mon = mon.unwrap();
    assert_eq!(mon.output_name(), "output2");
}

#[test]
fn large_negative_height_change() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::AdjustProportion(-1e129),
        },
    ];

    let mut options = Options::default();
    options.layout.border.off = false;
    options.layout.border.width = 1.;

    check_ops_with_options(options, ops);
}

#[test]
fn large_max_size() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                min_max_size: (Size::from((0, 0)), Size::from((i32::MAX, i32::MAX))),
                ..TestWindowParams::new(1)
            },
        },
    ];

    let mut options = Options::default();
    options.layout.border.off = false;
    options.layout.border.width = 1.;

    check_ops_with_options(options, ops);
}

#[test]
fn workspace_cleanup_during_switch() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::CloseWindow(1),
    ];

    check_ops(ops);
}

#[test]
fn workspace_transfer_during_switch() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::RemoveOutput(1),
        Op::FocusWorkspaceDown,
        Op::FocusWorkspaceDown,
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn workspace_transfer_during_switch_from_last() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(2),
        Op::RemoveOutput(1),
        Op::FocusWorkspaceUp,
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn workspace_transfer_during_switch_gets_cleaned_up() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(1),
        Op::AddOutput(2),
        Op::MoveColumnToWorkspaceDown(true),
        Op::MoveColumnToWorkspaceDown(true),
        Op::AddOutput(1),
    ];

    check_ops(ops);
}

#[test]
fn move_workspace_to_output() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::MoveWorkspaceToOutput(2),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal {
        monitors,
        active_monitor_idx,
        ..
    } = layout.monitor_set
    else {
        unreachable!()
    };

    assert_eq!(active_monitor_idx, 1);
    assert_eq!(monitors[0].workspaces.len(), 1);
    assert!(!monitors[0].workspaces[0].has_windows());
    assert_eq!(monitors[1].active_workspace_idx, 0);
    assert_eq!(monitors[1].workspaces.len(), 2);
    assert!(monitors[1].workspaces[0].has_windows());
}

#[test]
fn open_right_of_on_different_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(3),
            next_to_id: 1,
        },
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let mon = monitors.into_iter().next().unwrap();
    assert_eq!(
        mon.active_workspace_idx, 1,
        "the second workspace must remain active"
    );
    assert_eq!(
        mon.workspaces[0].tiling().active_column_idx(),
        1,
        "the new window must become active"
    );
}

#[test]
// empty_workspace_above_first = true
fn open_right_of_on_different_workspace_ewaf() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(3),
            next_to_id: 1,
        },
    ];

    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(options, ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    let mon = monitors.into_iter().next().unwrap();
    assert_eq!(
        mon.active_workspace_idx, 2,
        "the second workspace must remain active"
    );
    assert_eq!(
        mon.workspaces[1].tiling().active_column_idx(),
        1,
        "the new window must become active"
    );
}

#[test]
fn removing_all_outputs_preserves_empty_named_workspaces() {
    let ops = [
        Op::AddOutput(1),
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: None,
            layout_config: None,
        },
        Op::AddNamedWorkspace {
            ws_name: 2,
            output_name: None,
            layout_config: None,
        },
        Op::RemoveOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::NoOutputs { workspaces } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(workspaces.len(), 2);
}

#[test]
fn config_change_updates_cached_sizes() {
    let mut config = Config::default();
    let border = &mut config.layout.border;
    border.off = false;
    border.width = 2.;

    let mut layout = Layout::new(Clock::default(), &config);

    Op::AddWindow {
        params: TestWindowParams {
            bbox: Rectangle::from_size(Size::from((1280, 200))),
            ..TestWindowParams::new(1)
        },
    }
    .apply(&mut layout);

    config.layout.border.width = 4.;
    layout.update_config(&config);

    layout.verify_invariants();
}

#[test]
fn preset_height_change_removes_preset() {
    let mut config = Config::default();
    config.layout.preset_window_heights = vec![PresetSize::Fixed(1), PresetSize::Fixed(2)];

    let mut layout = Layout::new(Clock::default(), &config);

    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::SwitchPresetWindowHeight { id: None },
        Op::SwitchPresetWindowHeight { id: None },
    ];
    for op in ops {
        op.apply(&mut layout);
    }

    // Leave only one.
    config.layout.preset_window_heights = vec![PresetSize::Fixed(1)];

    layout.update_config(&config);

    layout.verify_invariants();
}

#[test]
fn set_window_height_recomputes_to_auto() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        Op::FocusWindowUp,
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
    ];

    check_ops(ops);
}

#[test]
fn one_window_in_column_becomes_weight_1() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(100),
        },
        Op::Communicate(2),
        Op::FocusWindowUp,
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(200),
        },
        Op::Communicate(1),
        Op::CloseWindow(0),
        Op::CloseWindow(1),
    ];

    check_ops(ops);
}

#[test]
fn fixed_height_takes_max_non_auto_into_account() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::SetWindowHeight {
            id: Some(0),
            change: SizeChange::SetFixed(704),
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
    ];

    let options = Options {
        layout: tiri_config::Layout {
            border: tiri_config::Border {
                off: false,
                width: 4.,
                ..Default::default()
            },
            gaps: 0.,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn start_interactive_move_then_remove_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::CloseWindow(0),
    ];

    check_ops(ops);
}

#[test]
fn maximize_during_interactive_move_start_is_ignored() {
    let layout = check_ops([
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::InteractiveMoveBegin {
            window: 3,
            output_idx: 2,
            px: 0.,
            py: 0.,
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::AddWindowNextTo {
            params: TestWindowParams::new(1),
            next_to_id: 3,
        },
        Op::InteractiveMoveUpdate {
            window: 3,
            dx: 0.,
            dy: -10406.186649509411,
            output_idx: 2,
            px: 0.,
            py: 0.,
        },
    ]);

    let Some(InteractiveMoveState::Moving(move_)) = &layout.interactive_move else {
        panic!("interactive move should still be active");
    };

    assert_eq!(move_.tile.window().id(), &3);
    assert!(move_.tile.window().pending_sizing_mode().is_normal());
}

#[test]
fn interactive_move_onto_empty_output() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::AddOutput(2),
        Op::InteractiveMoveUpdate {
            window: 0,
            dx: 1000.,
            dy: 0.,
            output_idx: 2,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveEnd { window: 0 },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_onto_empty_output_ewaf() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::AddOutput(2),
        Op::InteractiveMoveUpdate {
            window: 0,
            dx: 1000.,
            dy: 0.,
            output_idx: 2,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveEnd { window: 0 },
    ];

    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn interactive_move_onto_last_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::InteractiveMoveBegin {
            window: 0,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveUpdate {
            window: 0,
            dx: 1000.,
            dy: 0.,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::FocusWorkspaceDown,
        Op::AdvanceAnimations { msec_delta: 1000 },
        Op::InteractiveMoveEnd { window: 0 },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_onto_first_empty_workspace() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::InteractiveMoveBegin {
            window: 1,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::InteractiveMoveUpdate {
            window: 1,
            dx: 1000.,
            dy: 0.,
            output_idx: 1,
            px: 0.,
            py: 0.,
        },
        Op::FocusWorkspaceUp,
        Op::AdvanceAnimations { msec_delta: 1000 },
        Op::InteractiveMoveEnd { window: 1 },
    ];
    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn output_active_workspace_is_preserved() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::RemoveOutput(1),
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[0].active_workspace_idx, 1);
}

#[test]
fn output_active_workspace_is_preserved_with_other_outputs() {
    let ops = [
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::RemoveOutput(1),
        Op::AddOutput(1),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[1].active_workspace_idx, 1);
}

#[test]
fn named_workspace_to_output() {
    let ops = [
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: None,
            layout_config: None,
        },
        Op::AddOutput(1),
        Op::MoveWorkspaceToOutput(1),
        Op::FocusWorkspaceUp,
    ];
    check_ops(ops);
}

#[test]
// empty_workspace_above_first = true
fn named_workspace_to_output_ewaf() {
    let ops = [
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(2),
            layout_config: None,
        },
        Op::AddOutput(1),
        Op::AddOutput(2),
    ];
    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn move_window_to_empty_workspace_above_first() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MoveWorkspaceUp,
        Op::MoveWorkspaceDown,
        Op::FocusWorkspaceUp,
        Op::MoveWorkspaceDown,
    ];
    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn move_window_to_different_output() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::MoveWorkspaceToOutput(2),
    ];
    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn close_window_empty_ws_above_first() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddOutput(1),
        Op::CloseWindow(1),
    ];
    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn add_and_remove_output() {
    let ops = [
        Op::AddOutput(2),
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::RemoveOutput(2),
    ];
    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn switch_ewaf_on() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ];

    let mut layout = check_ops(ops);
    layout.update_options(Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    });
    layout.verify_invariants();
}

#[test]
fn switch_ewaf_off() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ];

    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(options, ops);
    layout.update_options(Options::default());
    layout.verify_invariants();
}

#[test]
fn interactive_move_drop_on_other_output_during_animation() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::InteractiveMoveBegin {
            window: 3,
            output_idx: 3,
            px: 0.0,
            py: 0.0,
        },
        Op::FocusWorkspaceDown,
        Op::AddOutput(4),
        Op::InteractiveMoveUpdate {
            window: 3,
            dx: 0.0,
            dy: 8300.68619826683,
            output_idx: 4,
            px: 0.0,
            py: 0.0,
        },
        Op::RemoveOutput(4),
        Op::InteractiveMoveEnd { window: 3 },
    ];
    check_ops(ops);
}

#[test]
fn add_window_next_to_only_interactively_moved_without_outputs() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(1),
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 3586.692842955048,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::RemoveOutput(1),
        // We have no outputs, and the only existing window is interactively moved, meaning there
        // are no workspaces either.
        Op::AddWindowNextTo {
            params: TestWindowParams::new(3),
            next_to_id: 2,
        },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_toggle_floating_ends_dnd_gesture() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 3586.692842955048,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::Refresh { is_active: false },
        Op::ToggleWindowFloating { id: None },
        Op::InteractiveMoveEnd { window: 2 },
    ];

    check_ops(ops);
}

#[test]
fn interactive_move_floating_window_stays_out_of_active_grouped_floating_container() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(2),
        Op::ToggleWindowFloating { id: None },
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        let window2_tree = workspace
            .floating()
            .debug_tree_for_window(&2)
            .expect("window 2 floating tree");
        assert_eq!(workspace.tiling().tiles().count(), 0);
        assert_eq!(workspace.floating().tiles().count(), 4);
        assert_eq!(
            workspace.floating().root_layout_for_window(&1),
            Some(ContainerLayout::SplitV)
        );
        assert_eq!(
            window2_tree.matches("Window ").count(),
            1,
            "precondition: window 2 should start in its own floating container:\n{window2_tree}",
        );
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::InteractiveMoveBegin {
                window: 2,
                output_idx: 1,
                px: 0.,
                py: 0.,
            },
            Op::InteractiveMoveUpdate {
                window: 2,
                dx: 1.,
                dy: 0.,
                output_idx: 1,
                px: 1.,
                py: 0.,
            },
            Op::InteractiveMoveEnd { window: 2 },
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let window2_tree = workspace
        .floating()
        .debug_tree_for_window(&2)
        .expect("window 2 floating tree");
    assert_eq!(workspace.tiling().tiles().count(), 0);
    assert_eq!(workspace.floating().tiles().count(), 4);
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV)
    );
    assert_eq!(
        window2_tree.matches("Window ").count(),
        1,
        "interactive move should keep window 2 in its own floating container:\n{window2_tree}",
    );
}

#[test]
fn interactive_move_floating_window_stays_out_of_toggled_floating_subtree() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::SplitHorizontal,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusParent,
        Op::ToggleWindowFloating { id: None },
        Op::FocusWindow(1),
        Op::ToggleWindowFloating { id: None },
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        let window1_tree = workspace
            .floating()
            .debug_tree_for_window(&1)
            .expect("window 1 floating tree");
        assert_eq!(workspace.tiling().tiles().count(), 1);
        assert_eq!(workspace.floating().tiles().count(), 3);
        assert_eq!(
            window1_tree.matches("Window ").count(),
            1,
            "precondition: window 1 should start in its own floating container:\n{window1_tree}",
        );
        let window4_tree = workspace
            .floating()
            .debug_tree_for_window(&4)
            .expect("window 4 floating tree");
        assert!(
            window4_tree.matches("Window ").count() >= 2,
            "precondition: window 4 should belong to a grouped floating subtree:\n{window4_tree}",
        );
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::InteractiveMoveBegin {
                window: 1,
                output_idx: 1,
                px: 0.,
                py: 0.,
            },
            Op::InteractiveMoveUpdate {
                window: 1,
                dx: 1.,
                dy: 0.,
                output_idx: 1,
                px: 1.,
                py: 0.,
            },
            Op::InteractiveMoveEnd { window: 1 },
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    let window1_tree = workspace
        .floating()
        .debug_tree_for_window(&1)
        .expect("window 1 floating tree");
    assert_eq!(workspace.tiling().tiles().count(), 1);
    assert_eq!(workspace.floating().tiles().count(), 3);
    assert_eq!(
        window1_tree.matches("Window ").count(),
        1,
        "interactive move should keep window 1 in its own floating container:\n{window1_tree}",
    );
}

#[test]
fn interactive_move_from_workspace_with_layout_config() {
    let ops = [
        Op::AddNamedWorkspace {
            ws_name: 1,
            output_name: Some(2),
            layout_config: Some(Box::new(tiri_config::LayoutPart {
                border: Some(tiri_config::BorderRule {
                    on: true,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        },
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::InteractiveMoveBegin {
            window: 2,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 3586.692842955048,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        // Now remove and add the output. It will have the same workspace.
        Op::RemoveOutput(1),
        Op::AddOutput(1),
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 0.0,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
        // Now move onto a different workspace.
        Op::FocusWorkspaceDown,
        Op::CompleteAnimations,
        Op::InteractiveMoveUpdate {
            window: 2,
            dx: 0.0,
            dy: 0.0,
            output_idx: 1,
            px: 0.0,
            py: 0.0,
        },
    ];

    check_ops(ops);
}

#[test]
fn set_width_fixed_negative() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::SetColumnWidth(SizeChange::SetFixed(-100)),
    ];
    check_ops(ops);
}

#[test]
fn set_height_fixed_negative() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::SetWindowHeight {
            id: None,
            change: SizeChange::SetFixed(-100),
        },
    ];
    check_ops(ops);
}

#[test]
fn interactive_resize_to_negative() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::InteractiveResizeBegin {
            window: 3,
            edges: ResizeEdge::BOTTOM_RIGHT,
        },
        Op::InteractiveResizeUpdate {
            window: 3,
            dx: -10000.,
            dy: -10000.,
        },
    ];
    check_ops(ops);
}

#[test]
fn interactive_resize_nested_split_targets_parent() {
    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output0");
    layout.add_output(output.clone(), None);

    layout.add_window(
        TestWindow::new(TestWindowParams::new(1)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    layout.activate_window(&1);
    layout.split_vertical();
    layout.add_window(
        TestWindow::new(TestWindowParams::new(3)),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );
    layout.set_layout_mode(ContainerLayout::SplitH);

    let width_before_1 = requested_width(&layout, 1);
    let width_before_2 = requested_width(&layout, 2);
    let width_before_3 = requested_width(&layout, 3);

    let rect = tile_rect(&layout, 3);
    let pos = rect.loc + Point::from((rect.size.w - 1.0, rect.size.h / 2.0));
    let edges = layout
        .resize_edges_under(&output, pos)
        .expect("expected resize edge");
    assert!(edges.contains(ResizeEdge::RIGHT));

    assert!(layout.interactive_resize_begin(3, edges));
    layout.interactive_resize_update(&3, Point::from((100.0, 0.0)));
    layout.interactive_resize_end(&3);

    let width_after_1 = requested_width(&layout, 1);
    let width_after_2 = requested_width(&layout, 2);
    let width_after_3 = requested_width(&layout, 3);

    assert!(width_after_1 > width_before_1);
    assert!(width_after_3 > width_before_3);
    assert!(width_after_2 < width_before_2);
}

#[test]
fn windows_on_other_workspaces_remain_activated() {
    let ops = [
        Op::AddOutput(3),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWorkspaceDown,
        Op::Refresh { is_active: true },
    ];

    let layout = check_ops(ops);
    let (_, win) = layout.windows().next().unwrap();
    assert!(win.0.pending_activated.get());
}

#[test]
fn stacking_add_parent_brings_up_child() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(1),
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
    ];

    check_ops(ops);
}

#[test]
fn stacking_add_parent_brings_up_descendants() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(2),
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(0),
                ..TestWindowParams::new(1)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(2)
            },
        },
    ];

    check_ops(ops);
}

#[test]
fn stacking_activate_brings_up_descendants() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(0),
                ..TestWindowParams::new(1)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                parent_id: Some(1),
                ..TestWindowParams::new(2)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(3)
            },
        },
        Op::FocusWindow(0),
    ];

    check_ops(ops);
}

#[test]
fn stacking_set_parent_brings_up_child() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(0)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::SetParent {
            id: 0,
            new_parent_id: Some(1),
        },
    ];

    check_ops(ops);
}

#[test]
fn move_window_to_workspace_with_different_active_output() {
    let ops = [
        Op::AddOutput(0),
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusOutput(1),
        Op::MoveWindowToWorkspace {
            window_id: Some(0),
            workspace_idx: 2,
        },
    ];

    check_ops(ops);
}

#[test]
fn set_first_workspace_name() {
    let ops = [
        Op::AddOutput(0),
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
    ];

    check_ops(ops);
}

#[test]
fn set_first_workspace_name_ewaf() {
    let ops = [
        Op::AddOutput(0),
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
    ];

    let options = Options {
        layout: tiri_config::Layout {
            empty_workspace_above_first: true,
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn set_last_workspace_name() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusWorkspaceDown,
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
    ];

    check_ops(ops);
}

#[test]
fn ensure_workspace_by_name_creates_named_workspace() {
    let mut layout: Layout<TestWindow> = Layout::default();
    let output = make_test_output("eDP-1");
    layout.add_output(output.clone(), None);

    let (target_output, idx) = layout.ensure_workspace_by_name("3").unwrap();
    assert_eq!(
        target_output.as_ref().map(|out| out.name()),
        Some(output.name())
    );
    assert_eq!(idx, 0);

    let (found_idx, ws) = layout.find_workspace_by_name("3").unwrap();
    assert_eq!(found_idx, 0);
    assert_eq!(ws.name().map(String::as_str), Some("3"));
}

#[test]
fn find_workspace_by_ref_index_prefers_numeric_named_workspace() {
    let mut layout: Layout<TestWindow> = Layout::default();
    layout.add_output(make_test_output("eDP-1"), None);

    layout.ensure_workspace_by_name("3");
    let (_, ws) = layout.find_workspace_by_name("3").unwrap();
    let ws_id = ws.id();

    let resolved = layout
        .find_workspace_by_ref(WorkspaceReference::Index(3))
        .map(|ws| ws.id());
    assert_eq!(resolved, Some(ws_id));
}

#[test]
fn find_workspace_by_ref_index_without_numeric_named_workspace_returns_none() {
    let mut layout: Layout<TestWindow> = Layout::default();
    layout.add_output(make_test_output("eDP-1"), None);

    let resolved = layout.find_workspace_by_ref(WorkspaceReference::Index(2));
    assert!(resolved.is_none());
}

#[test]
fn set_workspace_name_by_index_does_not_use_positional_fallback() {
    let mut layout: Layout<TestWindow> = Layout::default();
    layout.add_output(make_test_output("eDP-1"), None);

    layout.set_workspace_name(
        "ws-should-not-be-created".to_owned(),
        Some(WorkspaceReference::Index(2)),
    );

    assert!(layout
        .find_workspace_by_name("ws-should-not-be-created")
        .is_none());
}

#[test]
fn internal_empty_workspace_tail_is_hidden_only_when_inactive() {
    let mut layout: Layout<TestWindow> = Layout::default();
    layout.add_output(make_test_output("eDP-1"), None);
    layout.ensure_workspace_by_name("1");

    let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
        unreachable!()
    };
    let mon = &mut monitors[0];

    // Right after creating "1", the old trailing empty workspace stays focused.
    assert!(!mon.is_internal_empty_workspace(mon.active_workspace_idx()));

    mon.activate_workspace(0);
    assert!(mon.is_internal_empty_workspace(1));
}

#[test]
fn transient_numeric_workspace_is_cleaned_when_empty_and_unfocused() {
    let mut layout: Layout<TestWindow> = Layout::default();
    layout.add_output(make_test_output("eDP-1"), None);
    layout
        .ensure_workspace_by_name_transient("93")
        .expect("must create transient workspace");

    {
        let MonitorSet::Normal { monitors, .. } = &mut layout.monitor_set else {
            unreachable!()
        };
        let mon = &mut monitors[0];
        let idx = mon
            .find_named_workspace_index("93")
            .expect("workspace 93 must exist");
        mon.activate_workspace(idx);
        mon.activate_workspace(1);
        // Simulate workspace switch animation completion for cleanup.
        mon.workspace_switch = None;
        mon.clean_up_workspaces();
    }

    assert!(layout.find_workspace_by_name("93").is_none());
}

#[test]
fn move_workspace_to_output_by_workspace_id_moves_correct_workspace() {
    let mut layout: Layout<TestWindow> = Layout::default();
    let output_a = make_test_output("eDP-1");
    let output_b = make_test_output("HDMI-A-1");
    layout.add_output(output_a.clone(), None);
    layout.add_output(output_b.clone(), None);
    layout.focus_output(&output_a);

    layout.ensure_workspace_by_name("10");
    let workspace_id = layout
        .find_workspace_by_name("10")
        .map(|(_, ws)| ws.id())
        .expect("workspace 10 must exist");

    layout.move_workspace_to_output_by_workspace_id(workspace_id, &output_b);

    let (_, ws) = layout
        .find_workspace_by_name("10")
        .expect("workspace 10 must still exist");
    assert_eq!(
        ws.current_output().map(|out| out.name()),
        Some(output_b.name())
    );
}

#[test]
fn move_workspace_to_idx_by_workspace_id_reorders_correct_workspace() {
    let mut layout: Layout<TestWindow> = Layout::default();
    layout.add_output(make_test_output("eDP-1"), None);
    layout.ensure_workspace_by_name("10");
    layout.ensure_workspace_by_name("20");
    layout.ensure_workspace_by_name("30");

    let workspace_id = layout
        .find_workspace_by_name("20")
        .map(|(_, ws)| ws.id())
        .expect("workspace 20 must exist");

    layout.move_workspace_to_idx_by_workspace_id(workspace_id, 0);

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!()
    };
    let names: Vec<_> = monitors[0]
        .workspaces
        .iter()
        .filter_map(|ws| ws.name().cloned())
        .collect();
    assert_eq!(
        names,
        vec!["20".to_owned(), "10".to_owned(), "30".to_owned()]
    );
}

#[test]
fn move_workspace_to_same_monitor_doesnt_reorder() {
    let ops = [
        Op::AddOutput(0),
        Op::SetWorkspaceName {
            new_ws_name: 0,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::FocusWorkspaceDown,
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::MoveWorkspaceToMonitor {
            ws_name: Some(0),
            output_id: 0,
        },
    ];

    let layout = check_ops(ops);
    let counts: Vec<_> = layout
        .workspaces()
        .map(|(_, _, ws)| ws.windows().count())
        .collect();
    assert_eq!(counts, &[1, 2, 0]);
}

#[test]
fn removing_window_above_preserves_focused_window() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));

    // Focus middle window and remove the window above it.
    assert!(harness.tree.focus_window_by_id(&2));
    let before = harness.tree.debug_tree();
    assert!(before.contains("Window 2 *"));

    let _ = harness.tree.remove_window(&1);

    let after = harness.tree.debug_tree();
    assert!(after.contains("Window 2 *"));
}

#[test]
fn preset_column_width_fixed_correct_with_border() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::SwitchPresetColumnWidth,
    ];

    let options = Options {
        layout: tiri_config::Layout {
            preset_column_widths: vec![PresetSize::Fixed(500)],
            ..Default::default()
        },
        ..Default::default()
    };
    let mut layout = check_ops_with_options(options, ops);

    let win = layout.windows().next().unwrap().1;
    let base_width = win.requested_size().unwrap().w;

    // Add border.
    let options = Options {
        layout: tiri_config::Layout {
            preset_column_widths: vec![PresetSize::Fixed(500)],
            border: tiri_config::Border {
                off: false,
                width: 5.,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    layout.update_options(options);

    // With border, the window gets less size.
    let win = layout.windows().next().unwrap().1;
    let bordered_width = win.requested_size().unwrap().w;
    assert!(bordered_width <= base_width);

    // Preset widths are ignored in i3-style tiling, so toggling doesn't change size.
    layout.toggle_width(true);
    let win = layout.windows().next().unwrap().1;
    assert_eq!(win.requested_size().unwrap().w, bordered_width);
}

#[test]
fn preset_column_width_reset_after_set_width() {
    let ops = [
        Op::AddOutput(0),
        Op::AddWindow {
            params: TestWindowParams::new(0),
        },
        Op::SwitchPresetColumnWidth,
        Op::SetWindowWidth {
            id: None,
            change: SizeChange::AdjustFixed(-10),
        },
        Op::SwitchPresetColumnWidth,
    ];

    let options = Options {
        layout: tiri_config::Layout {
            preset_column_widths: vec![PresetSize::Fixed(500), PresetSize::Fixed(1000)],
            ..Default::default()
        },
        ..Default::default()
    };
    let layout = check_ops_with_options(options, ops);
    let win = layout.windows().next().unwrap().1;
    let width_after_resize = win.requested_size().unwrap().w;
    assert!(width_after_resize > 0);
}

#[test]
fn move_column_to_workspace_unfocused_with_multiple_monitors() {
    let ops = [
        Op::AddOutput(1),
        Op::SetWorkspaceName {
            new_ws_name: 101,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::SetWorkspaceName {
            new_ws_name: 102,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::SetWorkspaceName {
            new_ws_name: 201,
            ws_name: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::MoveColumnToOutput {
            output_id: 1,
            target_ws_idx: Some(0),
            activate: false,
        },
        Op::FocusOutput(1),
    ];

    let layout = check_ops(ops);

    assert_eq!(layout.active_workspace().unwrap().name().unwrap(), "ws102");

    for (mon, win) in layout.windows() {
        let mon = mon.unwrap();
        let ws = mon
            .workspaces
            .iter()
            .find(|w| w.has_window(win.id()))
            .unwrap();

        assert_eq!(
            ws.name().unwrap(),
            match win.id() {
                1 | 4 => "ws101",
                2 => "ws102",
                3 => "ws201",
                _ => unreachable!(),
            }
        );
    }
}

#[test]
fn move_column_to_workspace_down_focus_false_on_floating_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::MoveColumnToWorkspaceDown(false),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[0].active_workspace_idx, 0);
}

#[test]
fn move_column_to_workspace_focus_false_on_floating_window() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::MoveColumnToWorkspace(1, false),
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = layout.monitor_set else {
        unreachable!()
    };

    assert_eq!(monitors[0].active_workspace_idx, 0);
}

#[test]
fn restore_to_floating_persists_across_fullscreen_maximize() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        // Maximize then fullscreen.
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        // Unfullscreen.
        Op::FullscreenWindow(1),
    ];

    let mut layout = check_ops(ops);

    // Unfullscreening should return the window to the maximized state.
    let tiling = layout.active_workspace().unwrap().tiling();
    assert!(tiling.tiles().next().is_some());

    let ops = [
        // Unmaximize.
        Op::MaximizeWindowToEdges { id: None },
    ];
    check_ops_on_layout(&mut layout, ops);

    // The window was originally floating, so unmaximize restores it to floating.
    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&1));
}

#[test]
fn floating_fullscreen_roundtrip_restores_floating() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        Op::FullscreenWindow(1),
    ];

    let layout = check_ops(ops);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&1));
}

#[test]
fn floating_quick_fullscreen_roundtrip_restores_floating() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FullscreenWindow(1),
        // No communicate here: quickly toggle fullscreen off.
        Op::FullscreenWindow(1),
    ];

    let layout = check_ops(ops);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&1));
}

#[test]
fn floating_fullscreen_roundtrip_restores_floating_with_other_tiling_windows() {
    let mut floating_params = TestWindowParams::new(2);
    floating_params.is_floating = true;

    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: floating_params,
        },
        Op::FullscreenWindow(2),
        Op::Communicate(2),
        Op::FullscreenWindow(2),
    ];

    let layout = check_ops(ops);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&2));
    assert!(!workspace.is_floating(&1));
}

#[test]
fn floating_windowed_fullscreen_replaces_existing_floating_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(5)
            },
        },
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(4)
            },
        },
        Op::FullscreenWindow(5),
        Op::ToggleWindowedFullscreen(4),
    ];

    let layout = check_ops(ops);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&5));
    assert!(workspace.is_floating(&4));

    let (_mon, win4) = layout
        .windows()
        .find(|(_, win)| *win.id() == 4)
        .expect("window 4 should exist");
    let (_mon, win5) = layout
        .windows()
        .find(|(_, win)| *win.id() == 5)
        .expect("window 5 should exist");

    assert!(win4.pending_sizing_mode().is_fullscreen());
    assert!(!win5.pending_sizing_mode().is_fullscreen());
}

#[test]
fn tiling_maximized_window_floated_clears_maximized_state() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::MaximizeWindowToEdges { id: Some(3) },
        Op::AddOutput(1),
        Op::FocusParent,
        Op::ToggleWindowFloating { id: None },
    ];

    let layout = check_ops(ops);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&3));

    let (_mon, win3) = layout
        .windows()
        .find(|(_, win)| *win.id() == 3)
        .expect("window 3 should exist");
    assert!(win3.pending_sizing_mode().is_normal());
}

#[test]
fn floating_interactive_resize_then_unfloat_clears_resize_state() {
    let ops = [
        Op::AddWindow {
            params: TestWindowParams {
                id: 5,
                is_floating: true,
                ..TestWindowParams::new(5)
            },
        },
        Op::AddOutput(1),
        Op::InteractiveResizeBegin {
            window: 5,
            edges: ResizeEdge::RIGHT,
        },
        Op::ToggleWindowFloating { id: None },
    ];

    let layout = check_ops(ops);
    let workspace = layout.active_workspace().unwrap();

    assert!(!workspace.is_floating(&5));
}

#[test]
fn floating_set_fullscreen_roundtrip_restores_floating() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: true,
        },
        Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: false,
        },
    ];

    let layout = check_ops(ops);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&1));

    let (_mon, win) = layout
        .windows()
        .find(|(_, win)| *win.id() == 1)
        .expect("window 1 should exist");
    assert!(
        !win.is_pending_windowed_fullscreen(),
        "windowed fullscreen should be cleared after roundtrip"
    );
}

#[test]
fn floating_fullscreen_roundtrip_restores_size_and_position() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::Communicate(1),
        Op::MoveFloatingWindow {
            id: Some(1),
            x: PositionChange::SetFixed(137.),
            y: PositionChange::SetFixed(91.),
            animate: false,
        },
        Op::SetWindowWidth {
            id: Some(1),
            change: SizeChange::SetFixed(777),
        },
        Op::SetWindowHeight {
            id: Some(1),
            change: SizeChange::SetFixed(444),
        },
        Op::Communicate(1),
        Op::CompleteAnimations,
    ]);

    let before = tile_rect(&layout, 1);

    check_ops_on_layout(
        &mut layout,
        [Op::SetFullscreenWindow {
            window: 1,
            is_fullscreen: true,
        }],
    );

    {
        let workspace = layout.active_workspace().unwrap();
        assert!(
            workspace.is_floating(&1),
            "window should remain floating while fullscreen is active"
        );
        assert!(
            workspace.floating().is_fullscreen(&1),
            "window should be marked as fullscreen in floating"
        );

        let (_mon, win) = layout
            .windows()
            .find(|(_, win)| *win.id() == 1)
            .expect("window 1 should exist");
        assert!(
            win.pending_sizing_mode().is_fullscreen(),
            "floating fullscreen should request real fullscreen state"
        );
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::Communicate(1),
            Op::SetFullscreenWindow {
                window: 1,
                is_fullscreen: false,
            },
        ],
    );

    {
        let workspace = layout.active_workspace().unwrap();
        assert!(
            workspace.is_floating(&1),
            "window should remain floating after unfullscreen"
        );
        assert!(
            !workspace.floating().is_fullscreen(&1),
            "fullscreen flag should be cleared"
        );

        let (_mon, win) = layout
            .windows()
            .find(|(_, win)| *win.id() == 1)
            .expect("window 1 should exist");
        assert!(
            win.pending_sizing_mode().is_normal(),
            "unfullscreen should clear the pending fullscreen state"
        );
    }

    check_ops_on_layout(&mut layout, [Op::Communicate(1), Op::CompleteAnimations]);

    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&1));

    let after = tile_rect(&layout, 1);
    let close = |a: f64, b: f64| (a - b).abs() <= 1.0;

    assert!(
        close(before.loc.x, after.loc.x),
        "x mismatch: before={} after={}",
        before.loc.x,
        after.loc.x
    );
    assert!(
        close(before.loc.y, after.loc.y),
        "y mismatch: before={} after={}",
        before.loc.y,
        after.loc.y
    );
    assert!(
        close(before.size.w, after.size.w),
        "w mismatch: before={} after={}",
        before.size.w,
        after.size.w
    );
    assert!(
        close(before.size.h, after.size.h),
        "h mismatch: before={} after={}",
        before.size.h,
        after.size.h
    );
}

#[test]
fn floating_fullscreen_move_window_preserves_restored_position() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::Communicate(1),
        Op::MoveFloatingWindow {
            id: Some(1),
            x: PositionChange::SetFixed(137.),
            y: PositionChange::SetFixed(91.),
            animate: false,
        },
        Op::Communicate(1),
        Op::CompleteAnimations,
    ]);

    let before = tile_rect(&layout, 1);

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetFullscreenWindow {
                window: 1,
                is_fullscreen: true,
            },
            Op::Communicate(1),
            Op::MoveFloatingWindow {
                id: Some(1),
                x: PositionChange::AdjustFixed(200.),
                y: PositionChange::AdjustFixed(150.),
                animate: false,
            },
            Op::SetFullscreenWindow {
                window: 1,
                is_fullscreen: false,
            },
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    );

    let after = tile_rect(&layout, 1);
    let close = |a: f64, b: f64| (a - b).abs() <= 1.0;

    assert!(
        close(before.loc.x, after.loc.x),
        "fullscreen move should not change restored x position: before={} after={}",
        before.loc.x,
        after.loc.x
    );
    assert!(
        close(before.loc.y, after.loc.y),
        "fullscreen move should not change restored y position: before={} after={}",
        before.loc.y,
        after.loc.y
    );
}

#[test]
fn floating_fullscreen_center_window_preserves_restored_position() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                is_floating: true,
                ..TestWindowParams::new(1)
            },
        },
        Op::Communicate(1),
        Op::MoveFloatingWindow {
            id: Some(1),
            x: PositionChange::SetFixed(137.),
            y: PositionChange::SetFixed(91.),
            animate: false,
        },
        Op::Communicate(1),
        Op::CompleteAnimations,
    ]);

    let before = tile_rect(&layout, 1);

    check_ops_on_layout(
        &mut layout,
        [
            Op::SetFullscreenWindow {
                window: 1,
                is_fullscreen: true,
            },
            Op::Communicate(1),
            Op::CenterWindow { id: Some(1) },
            Op::SetFullscreenWindow {
                window: 1,
                is_fullscreen: false,
            },
            Op::Communicate(1),
            Op::CompleteAnimations,
        ],
    );

    let after = tile_rect(&layout, 1);
    let close = |a: f64, b: f64| (a - b).abs() <= 1.0;

    assert!(
        close(before.loc.x, after.loc.x),
        "fullscreen center should not change restored x position: before={} after={}",
        before.loc.x,
        after.loc.x
    );
    assert!(
        close(before.loc.y, after.loc.y),
        "fullscreen center should not change restored y position: before={} after={}",
        before.loc.y,
        after.loc.y
    );
}

#[test]
fn floating_fullscreen_roundtrip_restores_position_in_container_order() {
    let mut p1 = TestWindowParams::new(1);
    p1.is_floating = true;
    let mut p2 = TestWindowParams::new(2);
    p2.is_floating = true;
    let mut p3 = TestWindowParams::new(3);
    p3.is_floating = true;

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow { params: p1 },
        Op::SplitHorizontal,
        Op::AddWindow { params: p2 },
        Op::AddWindow { params: p3 },
        Op::Communicate(1),
        Op::Communicate(2),
        Op::Communicate(3),
        Op::CompleteAnimations,
    ]);

    let ws = layout.active_workspace().unwrap();
    assert!(ws.is_floating(&1));
    assert!(ws.is_floating(&2));
    assert!(ws.is_floating(&3));

    let before1 = tile_rect(&layout, 1);
    let before2 = tile_rect(&layout, 2);
    let before3 = tile_rect(&layout, 3);

    let close = |a: f64, b: f64| (a - b).abs() <= 1.0;

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWindow(2),
            Op::SetFullscreenWindow {
                window: 2,
                is_fullscreen: true,
            },
            Op::Communicate(2),
            Op::SetFullscreenWindow {
                window: 2,
                is_fullscreen: false,
            },
            Op::Communicate(2),
            Op::CompleteAnimations,
        ],
    );

    let after1 = tile_rect(&layout, 1);
    let after2 = tile_rect(&layout, 2);
    let after3 = tile_rect(&layout, 3);

    assert!(close(before1.loc.x, after1.loc.x));
    assert!(close(before2.loc.x, after2.loc.x));
    assert!(close(before3.loc.x, after3.loc.x));
}

#[test]
fn unmaximize_during_fullscreen_does_not_float() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        // Maximize then fullscreen.
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        // Unmaximize.
        Op::MaximizeWindowToEdges { id: None },
    ];

    let mut layout = check_ops(ops);

    // Unmaximize shouldn't have changed the window state since it's fullscreen.
    let tiling = layout.active_workspace().unwrap().tiling();
    assert!(tiling.tiles().next().is_some());

    let ops = [
        // Unfullscreen.
        Op::FullscreenWindow(1),
    ];
    check_ops_on_layout(&mut layout, ops);

    // The window was originally floating, so unfullscreen restores it to floating.
    let workspace = layout.active_workspace().unwrap();
    assert!(workspace.is_floating(&1));
}

#[test]
fn move_column_to_workspace_maximize_and_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        Op::MoveColumnToWorkspaceDown(true),
        Op::FullscreenWindow(1),
    ];

    let layout = check_ops(ops);
    let (_, win) = layout.windows().next().unwrap();

    // Unfullscreening should return to maximized because the window was maximized before.
    assert_eq!(win.pending_sizing_mode(), SizingMode::Maximized);
}

#[test]
fn move_window_to_workspace_maximize_and_fullscreen() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::MaximizeWindowToEdges { id: None },
        Op::FullscreenWindow(1),
        Op::MoveWindowToWorkspaceDown(true),
        Op::FullscreenWindow(1),
    ];

    let layout = check_ops(ops);
    let (_, win) = layout.windows().next().unwrap();

    // Unfullscreening should return to maximized because the window was maximized before.
    assert_eq!(win.pending_sizing_mode(), SizingMode::Maximized);
}

#[test]
fn tabs_with_different_border() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams {
                rules: Some(ResolvedWindowRules {
                    border: tiri_config::BorderRule {
                        on: true,
                        ..Default::default()
                    },
                    ..ResolvedWindowRules::default()
                }),
                ..TestWindowParams::new(2)
            },
        },
        Op::SwitchPresetWindowHeight { id: None },
        Op::ToggleColumnTabbedDisplay,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ConsumeOrExpelWindowLeft { id: None },
    ];

    let options = Options {
        layout: tiri_config::Layout {
            struts: Struts {
                left: FloatOrInt(0.),
                right: FloatOrInt(0.),
                top: FloatOrInt(20000.),
                bottom: FloatOrInt(0.),
            },
            ..Default::default()
        },
        ..Default::default()
    };
    check_ops_with_options(options, ops);
}

#[test]
fn expel_pending_left_from_fullscreen_tabbed_column() {
    let ops = [
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FullscreenWindow(1),
        Op::Communicate(1),
        // 1 is now fullscreen, view_offset_to_restore is set.
        Op::ToggleColumnTabbedDisplay,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ConsumeOrExpelWindowLeft { id: Some(2) },
        // 2 is consumed into a fullscreen column, fullscreen is requested but not applied.
        //
        // Now, get it back out while keeping it focused.
        //
        // Importantly, we expel it *left*, which results in adding a new column with the exact
        // same active_column_idx.
        Op::FocusWindow(2),
        Op::ConsumeOrExpelWindowLeft { id: None },
    ];

    check_ops(ops);
}

#[test]
fn workspace_render_geo_at_fractional_scale() {
    let ops = [
        Op::AddScaledOutput {
            id: 1,
            scale: 1.1,
            layout_config: None,
        },
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusWorkspaceDown,
        Op::CompleteAnimations,
    ];

    let layout = check_ops(ops);

    let MonitorSet::Normal { monitors, .. } = &layout.monitor_set else {
        unreachable!()
    };

    let mon = &monitors[0];
    let mut iter = mon.workspaces_with_render_geo();
    let (_ws, geo) = iter.next().unwrap();
    assert!(
        iter.next().is_none(),
        "animations are completed, only one workspace should be visible"
    );
    assert_eq!(
        geo.loc.y, 0.,
        "active workspace must be at y = 0 exactly, \
         otherwise a pointer against the screen edge at y = 0 won't hit it"
    );
}

fn parent_id_causes_loop(layout: &Layout<TestWindow>, id: usize, mut parent_id: usize) -> bool {
    if parent_id == id {
        return true;
    }

    'outer: loop {
        for (_, win) in layout.windows() {
            if win.0.id == parent_id {
                match win.0.parent_id.get() {
                    Some(new_parent_id) => {
                        if new_parent_id == id {
                            // Found a loop.
                            return true;
                        }

                        parent_id = new_parent_id;
                        continue 'outer;
                    }
                    // Reached window with no parent.
                    None => return false,
                }
            }
        }

        // Parent is not in the layout.
        return false;
    }
}

fn arbitrary_spacing() -> impl Strategy<Value = f64> {
    // Give equal weight to:
    // - 0: the element is disabled
    // - 4: some reasonable value
    // - random value, likely unreasonably big
    prop_oneof![Just(0.), Just(4.), ((1.)..=65535.)]
}

fn arbitrary_spacing_neg() -> impl Strategy<Value = f64> {
    // Give equal weight to:
    // - 0: the element is disabled
    // - 4: some reasonable value
    // - -4: some reasonable negative value
    // - random value, likely unreasonably big
    prop_oneof![Just(0.), Just(4.), Just(-4.), ((1.)..=65535.)]
}

fn arbitrary_struts() -> impl Strategy<Value = Struts> {
    (
        arbitrary_spacing_neg(),
        arbitrary_spacing_neg(),
        arbitrary_spacing_neg(),
        arbitrary_spacing_neg(),
    )
        .prop_map(|(left, right, top, bottom)| Struts {
            left: FloatOrInt(left),
            right: FloatOrInt(right),
            top: FloatOrInt(top),
            bottom: FloatOrInt(bottom),
        })
}

fn arbitrary_tab_indicator_position() -> impl Strategy<Value = TabIndicatorPosition> {
    prop_oneof![
        Just(TabIndicatorPosition::Left),
        Just(TabIndicatorPosition::Right),
        Just(TabIndicatorPosition::Top),
        Just(TabIndicatorPosition::Bottom),
    ]
}

prop_compose! {
    fn arbitrary_focus_ring()(
        off in any::<bool>(),
        width in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
    ) -> tiri_config::BorderRule {
        tiri_config::BorderRule {
            off,
            on: !off,
            width,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_border()(
        off in any::<bool>(),
        width in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
    ) -> tiri_config::BorderRule {
        tiri_config::BorderRule {
            off,
            on: !off,
            width,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_shadow()(
        off in any::<bool>(),
        softness in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
    ) -> tiri_config::ShadowRule {
        tiri_config::ShadowRule {
            off,
            on: !off,
            softness,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_tab_indicator()(
        off in any::<bool>(),
        hide_when_single_tab in prop::option::of(any::<bool>().prop_map(Flag)),
        place_within_column in prop::option::of(any::<bool>().prop_map(Flag)),
        width in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
        gap in prop::option::of(arbitrary_spacing_neg().prop_map(FloatOrInt)),
        length in prop::option::of((0f64..2f64)
            .prop_map(|x| TabIndicatorLength { total_proportion: Some(x) })),
        position in prop::option::of(arbitrary_tab_indicator_position()),
    ) -> tiri_config::TabIndicatorPart {
        tiri_config::TabIndicatorPart {
            off,
            on: !off,
            hide_when_single_tab,
            place_within_column,
            width,
            gap,
            length,
            position,
            ..Default::default()
        }
    }
}

prop_compose! {
    fn arbitrary_layout_part()(
        gaps in prop::option::of(arbitrary_spacing().prop_map(FloatOrInt)),
        struts in prop::option::of(arbitrary_struts()),
        focus_ring in prop::option::of(arbitrary_focus_ring()),
        border in prop::option::of(arbitrary_border()),
        shadow in prop::option::of(arbitrary_shadow()),
        tab_indicator in prop::option::of(arbitrary_tab_indicator()),
        empty_workspace_above_first in prop::option::of(any::<bool>().prop_map(Flag)),
    ) -> tiri_config::LayoutPart {
        tiri_config::LayoutPart {
            gaps,
            struts,
            empty_workspace_above_first,
            focus_ring,
            border,
            shadow,
            tab_indicator,
            ..Default::default()
        }
    }
}

struct TreeHarness {
    tree: ContainerTree<TestWindow>,
    options: Rc<Options>,
    clock: Clock,
    view_size: Size<f64, Logical>,
    scale: f64,
}

impl TreeHarness {
    fn new() -> Self {
        let options = Rc::new(Options::from_config(&Config::default()));
        let clock = Clock::with_time(Duration::ZERO);
        let view_size = Size::from((800.0, 600.0));
        let working_area = Rectangle::from_size(view_size);
        let scale = 1.0;
        let tree = ContainerTree::new(view_size, working_area, scale, options.clone());
        Self {
            tree,
            options,
            clock,
            view_size,
            scale,
        }
    }

    fn add_window(&mut self, id: usize) {
        self.add_window_with_params(TestWindowParams::new(id));
    }

    fn add_window_with_params(&mut self, params: TestWindowParams) {
        let window = TestWindow::new(params);
        let tile = Tile::new(
            window,
            self.view_size,
            self.scale,
            self.clock.clone(),
            self.options.clone(),
        );
        self.tree.insert_window(tile);
    }

    fn append_window(&mut self, id: usize) {
        self.append_window_with_params(TestWindowParams::new(id));
    }

    fn append_window_with_params(&mut self, params: TestWindowParams) {
        let window = TestWindow::new(params);
        let tile = Tile::new(
            window,
            self.view_size,
            self.scale,
            self.clock.clone(),
            self.options.clone(),
        );
        self.tree.append_leaf(tile, true);
    }
}

#[derive(Debug, Clone, Copy)]
enum TreeRandomOp {
    AddWindow,
    RemoveFocused,
    SplitH,
    SplitV,
    SetTabbed,
    SetStacked,
    ToggleSplit,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    FocusParent,
    FocusChild,
}

fn parse_debug_tree_windows(tree: &str) -> (Vec<usize>, usize, Option<usize>) {
    let mut ids = Vec::new();
    let mut focused_count = 0usize;
    let mut focused_id = None;

    for line in tree.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("Window ") else {
            continue;
        };

        let is_focused = rest.ends_with('*');
        let id_text = rest.trim_end_matches('*').trim();
        let id = id_text
            .parse::<usize>()
            .expect("window line in debug tree should contain a numeric id");

        ids.push(id);
        if is_focused {
            focused_count += 1;
            focused_id = Some(id);
        }
    }

    (ids, focused_count, focused_id)
}

fn count_root_children_in_debug_tree(tree: &str) -> usize {
    tree.lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("    "))
        .count()
}

fn apply_tree_random_op(harness: &mut TreeHarness, op: TreeRandomOp, next_window_id: &mut usize) {
    use super::container::Direction;

    match op {
        TreeRandomOp::AddWindow => {
            harness.add_window(*next_window_id);
            *next_window_id += 1;
        }
        TreeRandomOp::RemoveFocused => {
            let tree = harness.tree.debug_tree();
            let (_, _, focused_id) = parse_debug_tree_windows(&tree);
            if let Some(id) = focused_id {
                let _ = harness.tree.remove_window(&id);
            }
        }
        TreeRandomOp::SplitH => {
            harness.tree.split_focused(ContainerLayout::SplitH);
        }
        TreeRandomOp::SplitV => {
            harness.tree.split_focused(ContainerLayout::SplitV);
        }
        TreeRandomOp::SetTabbed => {
            harness.tree.set_focused_layout(ContainerLayout::Tabbed);
        }
        TreeRandomOp::SetStacked => {
            harness.tree.set_focused_layout(ContainerLayout::Stacked);
        }
        TreeRandomOp::ToggleSplit => {
            harness.tree.toggle_split_layout();
        }
        TreeRandomOp::FocusLeft => {
            harness.tree.focus_in_direction(Direction::Left);
        }
        TreeRandomOp::FocusRight => {
            harness.tree.focus_in_direction(Direction::Right);
        }
        TreeRandomOp::FocusUp => {
            harness.tree.focus_in_direction(Direction::Up);
        }
        TreeRandomOp::FocusDown => {
            harness.tree.focus_in_direction(Direction::Down);
        }
        TreeRandomOp::MoveLeft => {
            harness.tree.move_in_direction(Direction::Left);
        }
        TreeRandomOp::MoveRight => {
            harness.tree.move_in_direction(Direction::Right);
        }
        TreeRandomOp::MoveUp => {
            harness.tree.move_in_direction(Direction::Up);
        }
        TreeRandomOp::MoveDown => {
            harness.tree.move_in_direction(Direction::Down);
        }
        TreeRandomOp::FocusParent => {
            harness.tree.focus_parent();
        }
        TreeRandomOp::FocusChild => {
            harness.tree.focus_child();
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_container_tree_ops_keep_unique_ids_and_valid_focus(
        ops in prop::collection::vec(
            prop_oneof![
                Just(TreeRandomOp::AddWindow),
                Just(TreeRandomOp::RemoveFocused),
                Just(TreeRandomOp::SplitH),
                Just(TreeRandomOp::SplitV),
                Just(TreeRandomOp::SetTabbed),
                Just(TreeRandomOp::SetStacked),
                Just(TreeRandomOp::ToggleSplit),
                Just(TreeRandomOp::FocusLeft),
                Just(TreeRandomOp::FocusRight),
                Just(TreeRandomOp::FocusUp),
                Just(TreeRandomOp::FocusDown),
                Just(TreeRandomOp::MoveLeft),
                Just(TreeRandomOp::MoveRight),
                Just(TreeRandomOp::MoveUp),
                Just(TreeRandomOp::MoveDown),
                Just(TreeRandomOp::FocusParent),
                Just(TreeRandomOp::FocusChild),
            ],
            1..100
        ),
    ) {
        let mut harness = TreeHarness::new();
        let mut next_window_id = 1usize;

        harness.add_window(next_window_id);
        next_window_id += 1;

        for op in ops {
            apply_tree_random_op(&mut harness, op, &mut next_window_id);

            let tree = harness.tree.debug_tree();
            let (ids, focused_count, _focused_id) = parse_debug_tree_windows(&tree);
            let unique = ids.iter().copied().collect::<std::collections::HashSet<_>>();

            prop_assert_eq!(
                ids.len(),
                unique.len(),
                "duplicate window ids after {:?}:\n{}",
                op,
                tree,
            );

            if ids.is_empty() {
                prop_assert_eq!(
                    focused_count,
                    0,
                    "empty tree should not have focused windows after {:?}:\n{}",
                    op,
                    tree,
                );
            } else {
                prop_assert_eq!(
                    focused_count,
                    1,
                    "non-empty tree should have exactly one focused window after {:?}:\n{}",
                    op,
                    tree,
                );
            }
        }
    }
}

#[test]
fn move_right_enters_container_with_different_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 2
        Window 1 *
        Window 3
    "
    );
}

#[test]
fn move_right_escapes_to_grandparent_on_layout_mismatch() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
      Window 3 *
      Window 2
    "
    );
}

#[test]
fn focus_descends_into_last_focused_child() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&3));
    assert!(harness.tree.focus_in_direction(Direction::Right));
    assert!(harness.tree.focus_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
        Window 3 *
      Window 2
    "
    );
}

#[test]
fn preserve_explicit_same_layout_container_on_cleanup() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    harness.add_window(4);
    assert!(harness.tree.focus_in_direction(Direction::Right));
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    let _ = harness.tree.remove_window(&3);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      SplitV
        Window 1
        Window 4
      Window 2 *
    "
    );
}

#[test]
fn cleanup_reuses_last_root_layout_after_tree_becomes_empty() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    let _ = harness.tree.remove_window(&1);

    harness.add_window(2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      Window 2 *
    "
    );
}

#[test]
fn cleanup_preserves_single_explicit_split_for_future_inserts() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    let _ = harness.tree.remove_window(&3);

    harness.add_window(4);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
        Window 4 *
      Window 2
    "
    );
}

#[test]
fn keep_tabbed_container_on_cleanup_with_split_parent() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&2));
    harness.tree.split_focused(ContainerLayout::Tabbed);
    harness.add_window(3);
    harness.add_window(4);
    let _ = harness.tree.remove_window(&4);

    let tree = harness.tree.debug_tree();
    assert!(
        tree.contains("Tabbed"),
        "tabbed container should be preserved on cleanup:\n{tree}"
    );
}

#[test]
fn keep_stacked_container_on_cleanup_with_split_parent() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    assert!(harness.tree.focus_window_by_id(&2));
    harness.tree.split_focused(ContainerLayout::Stacked);
    harness.add_window(3);
    harness.add_window(4);
    let _ = harness.tree.remove_window(&4);

    let tree = harness.tree.debug_tree();
    assert!(
        tree.contains("Stacked"),
        "stacked container should be preserved on cleanup:\n{tree}"
    );
}

#[test]
fn move_left_enters_single_child_container() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    let _ = harness.tree.remove_window(&3);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.move_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
        Window 2 *
    "
    );
}

#[test]
fn move_right_swaps_with_sibling_in_same_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Window 3
      Window 2 *
    "
    );
}

#[test]
fn move_down_swaps_in_splitv() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    assert!(harness.tree.focus_in_direction(Direction::Up));
    assert!(harness.tree.move_in_direction(Direction::Down));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1
      Window 3
      Window 2 *
    "
    );
}

#[test]
fn move_down_enters_container_with_different_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    harness.tree.split_focused(ContainerLayout::SplitH);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Up));
    assert!(harness.tree.move_in_direction(Direction::Down));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      SplitH
        Window 2
        Window 1 *
        Window 3
    "
    );
}

#[test]
fn move_left_enters_container_with_different_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Right));
    assert!(harness.tree.move_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
        Window 3
        Window 2 *
    "
    );
}

#[test]
fn move_up_enters_container_with_different_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    assert!(harness.tree.focus_in_direction(Direction::Up));
    harness.tree.split_focused(ContainerLayout::SplitH);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Down));
    assert!(harness.tree.move_in_direction(Direction::Up));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      SplitH
        Window 1
        Window 3
        Window 2 *
    "
    );
}

#[test]
fn move_up_escapes_to_grandparent_on_layout_mismatch() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    harness.tree.split_focused(ContainerLayout::SplitH);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    assert!(harness.tree.move_in_direction(Direction::Up));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1
      Window 2 *
      SplitH
        Window 3
    "
    );
}

#[test]
fn preserve_single_child_container_with_different_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    let _ = harness.tree.remove_window(&3);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1 *
      Window 2
    "
    );
}

#[test]
fn replace_single_child_container_with_same_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitH));
    let _ = harness.tree.remove_window(&3);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitH
        Window 1 *
      Window 2
    "
    );
}

#[test]
fn move_right_enters_tabbed_container() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.tree.split_focused(ContainerLayout::Tabbed);
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Tabbed
        Window 2
        Window 3
        Window 1 *
    "
    );
}

#[test]
fn move_left_swaps_in_tabbed_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.move_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      Window 1
      Window 3 *
      Window 2
    "
    );
}

#[test]
fn split_inside_tabbed_creates_nested_split() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.split_focused(ContainerLayout::SplitH));
    harness.add_window(3);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      SplitH
        Window 1
        Window 3 *
      Window 2
    "
    );
}

#[test]
fn direct_tabbed_tiles_use_content_rect_without_tile_tab_offset() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    harness.tree.layout();

    let tiles = harness.tree.all_tiles();
    for id in [1usize, 2] {
        let tile = tiles
            .iter()
            .find(|tile| tile.window().id() == &id)
            .expect("tile should exist");
        assert!(
            tile.in_tabbed_context(),
            "window {id} should be in tabbed context"
        );
        assert_eq!(
            tile.tab_bar_offset(),
            0.0,
            "window {id} should not embed tab bar offset in tile geometry"
        );
    }
}

#[test]
fn tabbed_container_marks_urgent_tab() {
    let mut harness = TreeHarness::new();
    let mut urgent = TestWindowParams::new(1);
    urgent.is_urgent = true;
    harness.add_window_with_params(urgent);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    harness.tree.layout();

    let tab_bar = harness
        .tree
        .tab_bar_layouts()
        .into_iter()
        .next()
        .expect("tabbed tree should expose one tab bar");

    let urgent_tabs = tab_bar
        .tabs
        .iter()
        .filter(|tab| tab.is_urgent)
        .map(|tab| tab.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(urgent_tabs, vec!["Window 1"]);
}

#[test]
fn tabbed_context_propagates_to_nested_split_tiles() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);
    harness.tree.layout();

    let tiles = harness.tree.all_tiles();
    for id in [1usize, 2, 3] {
        let in_tabbed_context = tiles
            .iter()
            .find(|tile| tile.window().id() == &id)
            .map(|tile| tile.in_tabbed_context());
        assert_eq!(
            in_tabbed_context,
            Some(true),
            "window {id} should inherit tabbed border context"
        );
    }
}

#[test]
fn split_only_tiles_do_not_use_tabbed_context() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    harness.tree.layout();

    let tiles = harness.tree.all_tiles();
    for id in [1usize, 2, 3] {
        let in_tabbed_context = tiles
            .iter()
            .find(|tile| tile.window().id() == &id)
            .map(|tile| tile.in_tabbed_context());
        assert_eq!(
            in_tabbed_context,
            Some(false),
            "window {id} should not use tabbed border context in split layout"
        );
    }
}

#[test]
fn toggle_split_layout_switches_orientation() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.toggle_split_layout());

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1
      Window 2 *
    "
    );
}

#[test]
fn toggle_layout_all_cycles_through_all_layouts() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);

    assert!(harness.tree.toggle_layout_all());
    assert!(harness.tree.toggle_layout_all());
    assert!(harness.tree.toggle_layout_all());
    assert!(harness.tree.toggle_layout_all());

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Window 2 *
    "
    );
}

#[test]
fn i3_192_nested_container_layout_transitions() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);

    assert!(harness.tree.set_focused_layout(ContainerLayout::Stacked));
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Stacked
        Window 2
        Window 3 *
    "
    );

    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Tabbed
        Window 2
        Window 3 *
    "
    );

    assert!(harness.tree.toggle_split_layout());
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      SplitV
        Window 2
        Window 3 *
    "
    );
}

#[test]
fn i3_192_toggle_layout_all_cycles_nested_container_layouts() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);

    assert!(harness.tree.toggle_layout_all());
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Stacked
        Window 2
        Window 3 *
    "
    );

    assert!(harness.tree.toggle_layout_all());
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Tabbed
        Window 2
        Window 3 *
    "
    );

    assert!(harness.tree.toggle_layout_all());
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      SplitH
        Window 2
        Window 3 *
    "
    );
}

#[test]
fn i3_192_nested_container_layout_sequence_matches_i3() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);

    assert!(harness.tree.set_focused_layout(ContainerLayout::Stacked));
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.toggle_split_layout());
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      SplitV
        Window 2
        Window 3 *
    "
    );

    assert!(harness.tree.toggle_split_layout());
    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      SplitH
        Window 2
        Window 3 *
    "
    );
}

#[test]
fn move_down_swaps_in_stacked_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Stacked));
    assert!(harness.tree.focus_in_direction(Direction::Up));
    assert!(harness.tree.move_in_direction(Direction::Down));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Stacked
      Window 1
      Window 3
      Window 2 *
    "
    );
}

#[test]
fn move_up_escapes_tabbed_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    harness.tree.split_focused(ContainerLayout::Tabbed);
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.move_in_direction(Direction::Up));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1
      Window 2 *
      Tabbed
        Window 3
    "
    );
}

#[test]
fn move_left_escapes_stacked_layout() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.tree.split_focused(ContainerLayout::Stacked);
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.move_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Window 2 *
      Stacked
        Window 3
    "
    );
}

#[test]
fn move_left_at_edge_is_noop() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    assert!(!harness.tree.move_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1 *
      Window 2
    "
    );
}

#[test]
fn move_up_at_edge_is_noop() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    assert!(harness.tree.focus_in_direction(Direction::Up));
    assert!(!harness.tree.move_in_direction(Direction::Up));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1 *
      Window 2
    "
    );
}

#[test]
fn i3_122_repeated_split_on_single_window_does_not_nest_wrappers() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    let before = harness.tree.debug_tree().replace(" *", "");

    let _ = harness.tree.split_focused(ContainerLayout::SplitV);
    let after = harness.tree.debug_tree().replace(" *", "");

    assert_eq!(
        after, before,
        "repeating split on a single focused window should not keep nesting redundant wrappers",
    );
}

#[test]
fn i3_122_split_inside_stacked_creates_nested_split() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Stacked));
    assert!(harness.tree.split_focused(ContainerLayout::SplitH));
    harness.add_window(2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Stacked
      SplitH
        Window 1
        Window 2 *
    "
    );
}

#[test]
fn i3_122_toggle_split_switches_nested_container_orientation() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);

    assert!(harness.tree.toggle_split_layout());

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      SplitH
        Window 2
        Window 3 *
    "
    );
}

#[test]
fn i3_122_split_workspace_with_multiple_children_wraps_focused_branch() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);

    assert!(harness.tree.focus_root_child(0));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
        Window 3 *
      Window 2
    "
    );
}

#[test]
fn i3_122_repeated_split_without_new_window_keeps_tree_shape() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);

    assert!(harness.tree.focus_root_child(0));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);
    let before = harness.tree.debug_tree().replace(" *", "");

    let _ = harness.tree.split_focused(ContainerLayout::SplitV);
    let after = harness.tree.debug_tree().replace(" *", "");

    assert_eq!(
        after, before,
        "repeating split without opening a new window should not create extra container structure",
    );
}

#[test]
fn i3_122_split_on_empty_workspace_applies_to_next_window() {
    let mut harness = TreeHarness::new();
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(1);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1 *
    "
    );
}

#[test]
fn i3_122_split_on_single_window_persists_after_close() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    let _ = harness.tree.remove_window(&1);
    harness.add_window(2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 2 *
    "
    );
}

#[test]
fn split_on_empty_workspace_applies_to_next_window() {
    let mut harness = TreeHarness::new();
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(1);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1 *
    "
    );
}

#[test]
fn split_on_empty_workspace_applies_to_next_window_via_append() {
    let mut harness = TreeHarness::new();
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.append_window(1);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1 *
    "
    );
}

#[test]
fn layout_persists_after_last_window_closed() {
    let mut harness = TreeHarness::new();
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(1);
    let _ = harness.tree.remove_window(&1);
    harness.add_window(2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 2 *
    "
    );
}

#[test]
fn layout_persists_after_last_window_closed_via_append() {
    let mut harness = TreeHarness::new();
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.append_window(1);
    let _ = harness.tree.remove_window(&1);
    harness.append_window(2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 2 *
    "
    );
}

#[test]
fn split_on_single_window_persists_after_close() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    let _ = harness.tree.remove_window(&1);
    harness.add_window(2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 2 *
    "
    );
}

#[test]
fn split_parallel_with_siblings_wraps_focused_leaf_horizontal() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitH));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      SplitH
        Window 2 *
    "
    );
}

#[test]
fn split_parallel_with_siblings_wraps_focused_leaf_vertical() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitV));
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1
      SplitV
        Window 2 *
    "
    );
}

#[test]
fn removing_last_sibling_flattens_non_preserved_root_container() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);

    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.split_focused(ContainerLayout::Stacked));
    assert!(harness.tree.focus_window_by_id(&2));
    assert!(harness.tree.split_focused(ContainerLayout::SplitH));

    let _ = harness.tree.remove_window(&2);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Stacked
      Window 1 *
    "
    );
}

#[test]
fn wrap_root_for_sibling_insert_uses_pending_layout_hint() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);

    harness.tree.set_pending_layout(ContainerLayout::Tabbed);
    assert!(harness.tree.wrap_root_for_sibling_insert());

    let tree = harness.tree.debug_tree().replace(" *", "");
    assert!(
        tree.contains("Tabbed\n  SplitH\n    Window 1\n    Window 2"),
        "wrapping root for sibling insert should honor pending layout hint:\n{tree}"
    );
}

#[test]
fn move_right_from_single_child_container_is_atomic() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);

    assert!(harness.tree.focus_root_child(0));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(4);
    let _ = harness.tree.remove_window(&4);

    assert!(harness.tree.focus_root_child(0));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 2
      Window 1 *
      Window 3
    "
    );
}

#[test]
fn move_left_swaps_single_child_container_immediately() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);

    assert!(harness.tree.focus_root_child(1));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(4);
    let _ = harness.tree.remove_window(&4);
    assert!(harness.tree.focus_window_by_id(&2));

    assert!(harness.tree.move_in_direction(Direction::Left));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 2 *
      Window 1
      Window 3
    "
    );
}

#[test]
fn move_out_of_explicit_parallel_split_preserves_container_for_reentry() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);
    harness.add_window(4);
    assert!(harness.tree.set_focused_layout(ContainerLayout::SplitH));
    assert!(harness.tree.focus_window_by_id(&4));

    assert!(harness.tree.move_in_direction(Direction::Right));
    let after_move_out = harness.tree.debug_tree();
    assert_snapshot!(
        after_move_out.as_str(),
        @"
    SplitH
      SplitH
        Window 1
        Window 3
      Window 4 *
      Window 2
    "
    );

    assert!(harness.tree.move_in_direction(Direction::Left));
    let after_move_back = harness.tree.debug_tree();
    assert_snapshot!(
        after_move_back.as_str(),
        @"
    SplitH
      SplitH
        Window 1
        Window 3
        Window 4 *
      Window 2
    "
    );
}

#[test]
fn i3_124_move_single_window_is_noop() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    let before = harness.tree.debug_tree();
    assert!(!harness.tree.move_in_direction(Direction::Left));
    assert!(!harness.tree.move_in_direction(Direction::Right));
    assert!(!harness.tree.move_in_direction(Direction::Up));
    assert!(!harness.tree.move_in_direction(Direction::Down));
    let after = harness.tree.debug_tree();

    assert_eq!(
        after, before,
        "moving a single container in any direction should be a no-op",
    );
}

#[test]
fn i3_124_move_window_into_adjacent_split_container() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 2
        Window 1 *
        Window 3
    "
    );
}

#[test]
fn i3_124_move_window_out_of_split_on_layout_mismatch() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      SplitV
        Window 1
      Window 3 *
      Window 2
    "
    );
}

#[test]
fn i3_145_move_up_then_right_flattens_back_to_root_siblings() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);

    assert!(harness.tree.move_in_direction(Direction::Up));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Window 2
      Window 3 *
    "
    );
}

#[test]
fn i3_145_ticket_1053_sequence_flattens_after_second_move() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    harness.add_window(3);
    harness.add_window(4);

    assert!(harness.tree.focus_in_direction(Direction::Right));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    assert!(harness.tree.focus_in_direction(Direction::Right));
    assert!(harness.tree.move_in_direction(Direction::Left));
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.focus_parent());
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));

    let before = harness.tree.debug_tree();
    assert_eq!(
        count_root_children_in_debug_tree(&before),
        3,
        "precondition: first phase of i3 145 ticket #1053 should still have 3 root children:\n{before}",
    );

    assert!(harness.tree.focus_in_direction(Direction::Right));
    assert!(harness.tree.move_in_direction(Direction::Left));

    let after = harness.tree.debug_tree();
    assert_eq!(
        count_root_children_in_debug_tree(&after),
        2,
        "i3 145 ticket #1053 should flatten redundant wrappers after the second move:\n{after}",
    );
}

// Focus parent/child navigation tests
#[test]
fn focus_parent_at_root_is_noop() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    // Single window at root - focus_parent should return false
    assert!(!harness.tree.focus_parent());
}

#[test]
fn focus_parent_child_roundtrip_in_nested_splitv() {
    // Based on focus_descends_into_last_focused_child pattern
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&3));

    let tree_before = harness.tree.debug_tree();

    // Go up to parent (SplitV container)
    assert!(harness.tree.focus_parent());

    // Go back down to child (should return to window 3)
    assert!(harness.tree.focus_child());

    let tree_after = harness.tree.debug_tree();

    // Tree should be the same (window 3 still focused)
    assert_eq!(tree_before.as_str(), tree_after.as_str());
}

#[test]
fn focus_parent_traverses_hierarchy() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_in_direction(Direction::Left));
    harness.tree.split_focused(ContainerLayout::SplitV);
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&3));

    // Count how many times we can go up
    let mut levels = 0;
    while harness.tree.focus_parent() {
        levels += 1;
        // Safeguard against infinite loop
        if levels > 10 {
            break;
        }
    }

    // We should be able to go up at least once (from window to container)
    assert!(levels >= 1);
}

#[test]
fn i3_104_focus_stack_restores_tiling_focus_after_floating_close() {
    // Mirrors i3_test_cases/t/104-focus-stack.t:
    // opening a floating window must not lose the previous tiling focus after close.
    let mut floating = TestWindowParams::new(3);
    floating.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow { params: floating },
        Op::CloseWindow(3),
    ]);

    let focused = layout
        .focus()
        .map(|win| *win.id())
        .expect("focused window should exist");
    assert_eq!(
        focused, 2,
        "focus should restore to previously-focused tiled window"
    );
}

#[test]
fn i3_117_workspace_previous_switches_between_mru_workspaces() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::MoveWindowToWorkspaceDown(true),
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(
            workspace.has_window(&1),
            "moving with focus=true should leave us on the destination workspace",
        );
    }

    check_ops_on_layout(&mut layout, [Op::FocusWorkspacePrevious]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(
            workspace.has_window(&2),
            "workspace previous should restore the previously-focused workspace",
        );
        assert!(!workspace.has_window(&1));
    }

    check_ops_on_layout(&mut layout, [Op::FocusWorkspacePrevious]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.has_window(&1),
        "workspace previous should toggle back to the MRU workspace",
    );
}

#[test]
fn i3_118_open_then_kill_single_window_leaves_workspace_empty() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
    ]);

    assert!(layout.has_window(&1));
    layout.remove_window(&1, Transaction::new());

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.windows().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 0);
    assert_eq!(workspace.floating().tiles().count(), 0);
}

#[test]
fn i3_118_kill_unfocused_window_by_id_removes_correct_leaf() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));
    layout.remove_window(&1, Transaction::new());

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.has_window(&1));
    assert!(workspace.has_window(&2));
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));
}

#[test]
fn i3_129_focus_after_close_prefers_focus_stack_leaf() {
    // Mirrors i3_test_cases/t/129-focus-after-close.t (second scenario):
    // when closing an active leaf, focus is restored to the most-recent leaf from the stack.
    let layout = check_ops([
        Op::AddOutput(1),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindowUp,
        Op::SplitHorizontal,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindowDown,
        Op::CloseWindow(2),
    ]);

    let focused = layout
        .focus()
        .map(|win| *win.id())
        .expect("focused window should exist");
    assert_eq!(
        focused, 4,
        "after closing the bottom leaf, focus should return to top-right MRU leaf",
    );
}

#[test]
fn i3_129_kill_workspace_closes_tiling_and_floating_windows() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusParent,
        Op::FocusParent,
    ]);

    let selected_ids = layout.close_window_ids_for_active_selection();
    assert_eq!(
        selected_ids.len(),
        2,
        "workspace-level kill should target both tiling and floating windows",
    );

    for id in selected_ids {
        layout.remove_window(&id, Transaction::new());
    }

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.windows().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 0);
    assert_eq!(workspace.floating().tiles().count(), 0);
}

#[test]
fn kill_selected_floating_container_does_not_close_other_windows() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(3),
        Op::ToggleWindowFloating { id: None },
        Op::FocusWindow(2),
        Op::FocusParent,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.floating_is_active());
    assert_eq!(workspace.tiling().tiles().count(), 1);
    assert_eq!(workspace.floating().tiles().count(), 3);
    assert!(
        workspace.debug_handler_context() == "floating_container",
        "precondition: expected floating container selection",
    );

    let mut selected_ids = layout.close_window_ids_for_active_selection();
    selected_ids.sort_unstable();
    assert_eq!(
        selected_ids,
        vec![2, 4],
        "killing a selected floating container should not close other floating or tiling windows",
    );
}

#[test]
fn killing_workspace_selection_does_not_leave_new_windows_stuck_in_workspace_context() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusParent,
        Op::FocusParent,
    ]);

    let selected_ids = layout.close_window_ids_for_active_selection();
    assert_eq!(selected_ids, vec![1, 2, 3]);
    for id in selected_ids {
        layout.remove_window(&id, Transaction::new());
    }

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert_eq!(workspace.windows().count(), 0);
        assert_eq!(workspace.debug_handler_context(), "workspace");
        assert!(!workspace.is_tiling_workspace_context_active());
        assert!(!workspace.tiling().selected_is_container());
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::AddWindow {
                params: TestWindowParams::new(4),
            },
            Op::AddWindow {
                params: TestWindowParams::new(5),
            },
            Op::AddWindow {
                params: TestWindowParams::new(6),
            },
        ],
    );

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert_eq!(workspace.debug_handler_context(), "tiling_window");
        assert!(!workspace.is_tiling_workspace_context_active());
        assert_eq!(layout.focus().map(|win| *win.id()), Some(6));
    }

    layout.focus_left();
    assert_eq!(layout.focus().map(|win| *win.id()), Some(5));

    let selected_ids = layout.close_window_ids_for_active_selection();
    assert_eq!(
        selected_ids,
        vec![5],
        "kill after reopening windows should target only the focused leaf, not the whole workspace",
    );
}

#[test]
fn focusing_floating_leaf_clears_container_selection_and_restores_leaf_navigation() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusParent,
        Op::ToggleWindowFloating { id: None },
        Op::FocusWindow(2),
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert_eq!(workspace.debug_handler_context(), "floating_window");
        assert!(!workspace.debug_floating_workspace_context());
        assert!(!workspace.debug_active_floating_wrapper_selected());
        assert!(
            !workspace.floating().selected_is_container(Some(&2)),
            "explicitly focusing a floating leaf should clear floating container selection",
        );
    }

    layout.focus_right();
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "after focusing a floating leaf, directional focus should move between sibling floating windows again",
    );

    layout.focus_parent();
    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert_eq!(workspace.debug_handler_context(), "floating_container");
    }

    layout.toggle_window_floating(None);
    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.floating_is_active());
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 3);
}

#[test]
fn i3_130_closing_last_children_removes_empty_split_wrapper() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);

    let _ = harness.tree.remove_window(&3);
    let _ = harness.tree.remove_window(&1);

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Window 2 *
    "
    );
}

#[test]
fn i3_130_moving_last_children_away_removes_empty_split_wrapper() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWindow(1),
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::MoveWindowToWorkspaceDown(false),
        Op::FocusWindow(1),
        Op::MoveWindowToWorkspaceDown(false),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.has_window(&2));
    assert!(!workspace.has_window(&1));
    assert!(!workspace.has_window(&3));

    let tree = workspace.tiling().debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Window 2 *
    "
    );
}

#[test]
fn i3_124_move_left_then_right_swaps_root_siblings_without_extra_changes() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);

    assert!(harness.tree.move_in_direction(Direction::Left));
    let after_left = harness.tree.debug_tree();
    assert_eq!(
        parse_debug_tree_windows(&after_left),
        (vec![2, 1], 1, Some(2)),
        "moving the second root sibling left should swap it before the first:\n{after_left}",
    );

    assert!(!harness.tree.move_in_direction(Direction::Left));
    let after_second_left = harness.tree.debug_tree();
    assert_eq!(
        parse_debug_tree_windows(&after_second_left),
        (vec![2, 1], 1, Some(2)),
        "moving left again at the edge should be a no-op:\n{after_second_left}",
    );

    assert!(harness.tree.move_in_direction(Direction::Right));
    let after_right = harness.tree.debug_tree();
    assert_eq!(
        parse_debug_tree_windows(&after_right),
        (vec![1, 2], 1, Some(2)),
        "moving right should swap the root siblings back:\n{after_right}",
    );

    assert!(!harness.tree.move_in_direction(Direction::Right));
    let after_second_right = harness.tree.debug_tree();
    assert_eq!(
        parse_debug_tree_windows(&after_second_right),
        (vec![1, 2], 1, Some(2)),
        "moving right again at the edge should be a no-op:\n{after_second_right}",
    );
}

#[test]
fn i3_124_moving_all_children_out_of_split_removes_source_container() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);
    harness.add_window(2);
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(3);
    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(4);

    assert!(harness.tree.focus_window_by_id(&4));
    assert!(harness.tree.move_in_direction(Direction::Right));
    assert!(harness.tree.focus_window_by_id(&1));
    assert!(harness.tree.move_in_direction(Direction::Right));

    let tree = harness.tree.debug_tree();
    let mut ids = parse_debug_tree_windows(&tree).0;
    ids.sort_unstable();

    assert_eq!(
        count_root_children_in_debug_tree(&tree),
        1,
        "after moving the last two children out of the left split, the source container should be removed:\n{tree}",
    );
    assert_eq!(
        ids,
        vec![1, 2, 3, 4],
        "all windows should still be present:\n{tree}"
    );
}

#[test]
fn i3_127_killing_parent_chain_then_disabling_floating_reinserts_cleanly() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusWindow(2),
        Op::CloseWindow(2),
        Op::FocusWindow(1),
        Op::CloseWindow(1),
        Op::FocusWindow(3),
        Op::ToggleWindowFloating { id: None },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 1);
    assert_snapshot!(
        workspace.tiling().debug_tree().as_str(),
        @"
    Window 3 *
    "
    );
}

#[test]
fn i3_135_floating_toggle_roundtrip_preserves_focus() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
        Op::ToggleWindowFloating { id: None },
    ]);

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "toggling the focused window to floating and back should preserve focus",
    );
}

#[test]
fn i3_135_killing_unfocused_floating_window_keeps_current_floating_focus() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::FocusWindow(2),
        Op::ToggleWindowFloating { id: None },
        Op::CloseWindow(3),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));
    assert!(workspace.is_floating(&2));
    assert!(!workspace.has_window(&3));
}

#[test]
fn i3_135_killing_focused_floating_window_falls_back_to_next_floating_then_tiling() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(2),
        Op::ToggleWindowFloating { id: None },
        Op::FocusWindow(3),
        Op::ToggleWindowFloating { id: None },
        Op::FocusWindow(2),
    ]);

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "after closing the focused floating window, focus should fall back to the next floating window",
    );

    check_ops_on_layout(&mut layout, [Op::CloseWindow(3)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(1),
        "after closing the last floating window, focus should fall back to the last tiled window",
    );
}

#[test]
fn i3_135_focus_tiling_focus_floating_and_mode_toggle_switch_domains() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: None },
    ]);

    check_ops_on_layout(&mut layout, [Op::FocusTiling]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));

    check_ops_on_layout(&mut layout, [Op::FocusFloating]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));

    check_ops_on_layout(&mut layout, [Op::FocusFloating]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "focus floating on an already-focused floating window should be a no-op",
    );

    check_ops_on_layout(&mut layout, [Op::SwitchFocusFloatingTiling]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));

    check_ops_on_layout(&mut layout, [Op::SwitchFocusFloatingTiling]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));
}

#[test]
fn i3_135_directional_focus_cycles_across_floating_windows() {
    let mut one = TestWindowParams::new(1);
    one.is_floating = true;
    let mut two = TestWindowParams::new(2);
    two.is_floating = true;
    let mut three = TestWindowParams::new(3);
    three.is_floating = true;

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow { params: one },
        Op::AddWindow { params: two },
        Op::AddWindow { params: three },
    ]);

    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));

    check_ops_on_layout(&mut layout, [Op::FocusColumnLeft]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(3));

    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));

    check_ops_on_layout(&mut layout, [Op::FocusColumnRight]);
    assert_eq!(layout.focus().map(|win| *win.id()), Some(2));
}

#[test]
fn i3_135_focusing_floating_window_raises_it_to_front() {
    let mut one = TestWindowParams::new(1);
    one.is_floating = true;
    let mut two = TestWindowParams::new(2);
    two.is_floating = true;

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow { params: one },
        Op::AddWindow { params: two },
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        let order: Vec<_> = workspace
            .floating()
            .tiles()
            .map(|tile| *tile.window().id())
            .collect();
        assert_eq!(
            order.first().copied(),
            Some(2),
            "precondition: newest floating window should start on top",
        );
    }

    check_ops_on_layout(&mut layout, [Op::FocusWindow(1)]);

    let workspace = layout.active_workspace().expect("active workspace");
    let order: Vec<_> = workspace
        .floating()
        .tiles()
        .map(|tile| *tile.window().id())
        .collect();
    assert_eq!(layout.focus().map(|win| *win.id()), Some(1));
    assert_eq!(
        order.first().copied(),
        Some(1),
        "focusing a floating window should raise its container to the top of the floating stack",
    );
}

#[test]
fn i3_135_toggle_floating_on_focused_window_from_other_workspace_preserves_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWorkspace(1),
        Op::ToggleWindowFloating { id: Some(2) },
        Op::FocusWorkspace(0),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&2));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "toggling floating for the focused window from another workspace should preserve its focus",
    );

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(2) },
            Op::FocusWorkspace(0),
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&2));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "toggling the same window back to tiling from another workspace should still preserve focus",
    );
}

#[test]
fn i3_135_toggle_floating_on_unfocused_window_from_other_workspace_does_not_steal_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusWorkspace(1),
        Op::ToggleWindowFloating { id: Some(1) },
        Op::FocusWorkspace(0),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "toggling floating for an unfocused window from another workspace must not steal focus",
    );

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(1) },
            Op::FocusWorkspace(0),
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&1));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "toggling the unfocused window back to tiling from another workspace must still not steal focus",
    );
}

#[test]
fn i3_135_toggle_floating_on_other_workspace_keeps_focused_floating_window() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::ToggleWindowFloating { id: Some(2) },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWorkspace(1),
        Op::ToggleWindowFloating { id: Some(3) },
        Op::FocusWorkspace(0),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&2));
    assert!(workspace.is_floating(&3));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "if the toggled window was focused on its workspace, it should remain focused after returning",
    );

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(3) },
            Op::FocusWorkspace(0),
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&2));
    assert!(!workspace.is_floating(&3));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "toggling that same focused window back to tiling on another workspace should keep focus on it",
    );
}

#[test]
fn i3_135_toggle_unfocused_window_on_other_workspace_keeps_current_floating_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::FocusWorkspace(1),
        Op::ToggleWindowFloating { id: Some(2) },
        Op::FocusWorkspace(0),
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&2));
    assert!(workspace.is_floating(&3));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "toggling another window from a different workspace must not steal focus from the current floating window",
    );

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(2) },
            Op::FocusWorkspace(0),
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&2));
    assert!(workspace.is_floating(&3));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "toggling the unfocused window back to tiling on another workspace must keep floating focus unchanged",
    );
}

#[test]
fn i3_135_toggle_floating_for_nested_window_from_other_workspace_preserves_focus() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWorkspace(1),
        Op::ToggleWindowFloating { id: Some(3) },
        Op::FocusWorkspace(0),
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.is_floating(&3));
        assert_eq!(
            layout.focus().map(|win| *win.id()),
            Some(3),
            "toggling a focused nested window to floating from another workspace should preserve its focus",
        );
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(3) },
            Op::FocusWorkspace(0),
        ],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(!workspace.is_floating(&3));
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "toggling that nested window back to tiling from another workspace should still preserve focus",
    );
    let tree = workspace.tiling().debug_tree();
    assert!(
        tree.contains("Window 3 *"),
        "after the roundtrip, the nested window should still be the focused tiling leaf:\n{tree}",
    );
}

#[test]
fn i3_135_deep_floating_roundtrip_from_other_workspace_preserves_focus_chain() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::SplitHorizontal,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
    ]);

    let tree_before = layout
        .active_workspace()
        .expect("active workspace")
        .tiling()
        .debug_tree()
        .replace(" *", "");
    assert!(
        tree_before.contains("SplitV\n        Window 4\n        Window 5"),
        "precondition: deep nested layout should place window 4 before 5 in the innermost split:\n{tree_before}",
    );
    assert_eq!(layout.focus().map(|win| *win.id()), Some(5));

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(4) },
            Op::FocusWorkspace(0),
        ],
    );

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.is_floating(&4));
        assert_eq!(
            layout.focus().map(|win| *win.id()),
            Some(5),
            "after floating the deep nested window from another workspace, focus should stay on D-like sibling",
        );
        let tree = workspace.tiling().debug_tree().replace(" *", "");
        assert!(
            tree.contains("Window 5"),
            "the tiling tree should keep the sibling that replaced the floated window's slot:\n{tree}",
        );
    }

    check_ops_on_layout(
        &mut layout,
        [
            Op::FocusWorkspace(1),
            Op::ToggleWindowFloating { id: Some(4) },
            Op::FocusWorkspace(0),
        ],
    );

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(!workspace.is_floating(&4));
        assert_eq!(
            layout.focus().map(|win| *win.id()),
            Some(5),
            "after restoring the deep nested window to tiling from another workspace, focus should stay on the previously-focused sibling",
        );
        let tree = workspace.tiling().debug_tree();
        assert!(
            tree.contains("Window 4") && tree.contains("Window 5 *"),
            "after the roundtrip both deep siblings should exist and window 5 should still be focused:\n{tree}",
        );
    }

    check_ops_on_layout(&mut layout, [Op::CloseWindow(5)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(4),
        "after killing the focused deep sibling, focus should fall back to the restored floating-roundtrip window",
    );

    check_ops_on_layout(&mut layout, [Op::CloseWindow(4)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(3),
        "after killing the restored deep window, focus should move up the focus stack to the next ancestor leaf",
    );

    check_ops_on_layout(&mut layout, [Op::CloseWindow(3)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "after killing the next leaf, focus should continue restoring toward the previous sibling branch",
    );

    check_ops_on_layout(&mut layout, [Op::CloseWindow(2)]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(1),
        "after killing that branch, the root-left leaf should receive focus",
    );
}

#[test]
fn i3_135_focus_parent_then_focus_child_roundtrips_from_floating_window() {
    let mut floating = TestWindowParams::new(2);
    floating.is_floating = true;

    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow { params: floating },
    ]);

    check_ops_on_layout(&mut layout, [Op::FocusParent]);
    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(
            workspace.debug_floating_workspace_context(),
            "focus parent from a floating window should move to workspace context",
        );
    }

    check_ops_on_layout(&mut layout, [Op::FocusChild]);
    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(2),
        "focus child from workspace context should return to the floating window",
    );
}

#[test]
fn i3_146_floating_toggle_reinserts_into_previous_split_container() {
    // Mirrors i3_test_cases/t/146-floating-reinsert.t:
    // toggling a floating window back to tiling should reinsert it in the focused split.
    let mut floating = TestWindowParams::new(4);
    floating.is_floating = true;

    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::AddWindow { params: floating },
        Op::ToggleWindowFloating { id: None },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 4);
    assert_snapshot!(
        workspace.tiling().debug_tree().as_str(),
        @"
    SplitH
      Window 1
      SplitV
        Window 2
        Window 3
        Window 4 *
    "
    );
}

#[test]
fn i3_152_focus_parent_then_toggle_floating_workspace_context_behaves_like_sway() {
    // Mirrors i3_test_cases/t/152-regress-level-up.t and extends it with a
    // sway-equivalent no-op check when toggling from workspace context with empty tiling.
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::FocusParent,
        Op::FocusParent,
        Op::ToggleWindowFloating { id: None },
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert_eq!(workspace.tiling().tiles().count(), 0);
        assert_eq!(workspace.floating().tiles().count(), 1);
    }

    check_ops_on_layout(
        &mut layout,
        [Op::FocusParent, Op::ToggleWindowFloating { id: None }],
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.floating_is_active(),
        "workspace-context toggle_floating with empty tiling must be a no-op",
    );
    assert_eq!(workspace.tiling().tiles().count(), 0);
    assert_eq!(workspace.floating().tiles().count(), 1);
}

#[test]
fn floating_workspace_context_toggle_floating_uses_selected_floating_container_like_sway() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::FocusParent,
        Op::FocusParent,
    ]);

    {
        let workspace = layout.active_workspace().expect("active workspace");
        assert!(workspace.floating_is_active());
        assert!(workspace.debug_floating_workspace_context());
        assert_eq!(workspace.debug_command_context(), "workspace");
        assert_eq!(
            workspace.debug_active_floating_command_container_path(),
            Some(Vec::new()),
            "precondition: workspace context should still retain floating wrapper selection",
        );
    }

    layout.toggle_window_floating(None);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace.floating_is_active(),
        "workspace-context toggle_floating should restore the selected floating container to tiling",
    );
    assert_eq!(workspace.floating().tiles().count(), 0);
    assert_eq!(workspace.tiling().tiles().count(), 1);
}

#[test]
fn focus_stack_head_is_workspace_in_floating_workspace_context() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusParent,
    ]);

    let snapshot = layout.seat_focus.snapshot();
    assert!(
        matches!(snapshot.first(), Some(SeatFocusNode::Workspace { .. })),
        "workspace-context focus must record Workspace at seat-focus head",
    );
    assert!(
        snapshot.iter().any(
            |node| matches!(node, SeatFocusNode::Floating { window_id, .. } if *window_id == 1)
        ),
        "floating node should remain in inactive MRU history",
    );

    layout.switch_focus_floating_tiling();
    let snapshot = layout.seat_focus.snapshot();
    assert!(
        matches!(snapshot.first(), Some(SeatFocusNode::Floating { window_id, .. }) if *window_id == 1),
        "switching back to floating target should restore Floating at seat-focus head",
    );
}

#[test]
fn handler_context_routing_matrix_for_core_command_families() {
    struct ExpectedRoute {
        handler: &'static str,
        command: &'static str,
        focus: &'static str,
        split: &'static str,
        layout: &'static str,
        move_directional: &'static str,
        move_container: &'static str,
    }

    let cases: [(&str, Vec<Op>, ExpectedRoute); 5] = [
        (
            "tiling_window",
            vec![
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
            ],
            ExpectedRoute {
                handler: "tiling_window",
                command: "tiling",
                focus: "tiling",
                split: "tiling",
                layout: "tiling",
                move_directional: "tiling",
                move_container: "tiling",
            },
        ),
        (
            "tiling_container",
            vec![
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::SplitVertical,
                Op::FocusParent,
            ],
            ExpectedRoute {
                handler: "tiling_container",
                command: "tiling",
                focus: "tiling",
                split: "tiling",
                layout: "tiling",
                move_directional: "tiling",
                move_container: "tiling",
            },
        ),
        (
            "floating_window",
            vec![
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::ToggleWindowFloating { id: None },
            ],
            ExpectedRoute {
                handler: "floating_window",
                command: "floating",
                focus: "floating",
                split: "floating",
                layout: "floating",
                move_directional: "floating",
                move_container: "floating",
            },
        ),
        (
            "floating_container",
            vec![
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::ToggleWindowFloating { id: None },
                Op::SplitVertical,
                Op::FocusParent,
            ],
            ExpectedRoute {
                handler: "floating_container",
                command: "floating",
                focus: "floating",
                split: "floating",
                layout: "floating",
                move_directional: "floating",
                move_container: "floating",
            },
        ),
        (
            "floating_workspace_context",
            vec![
                Op::AddOutput(1),
                Op::AddWindow {
                    params: TestWindowParams::new(1),
                },
                Op::ToggleWindowFloating { id: None },
                Op::SplitVertical,
                Op::FocusParent,
                Op::FocusParent,
            ],
            ExpectedRoute {
                handler: "workspace",
                command: "workspace",
                focus: "workspace",
                split: "floating",
                layout: "floating",
                move_directional: "workspace",
                move_container: "workspace",
            },
        ),
    ];

    for (name, ops, expected) in cases {
        let layout = check_ops(ops);
        let workspace = layout.active_workspace().expect("active workspace");

        assert_eq!(
            workspace.debug_handler_context(),
            expected.handler,
            "case={name}: unexpected handler_context",
        );
        assert_eq!(
            workspace.debug_command_context(),
            expected.command,
            "case={name}: unexpected command_context",
        );
        assert_eq!(
            workspace.debug_route_domain_for_focus(),
            expected.focus,
            "case={name}: unexpected focus routing domain",
        );
        assert_eq!(
            workspace.debug_route_domain_for_split(),
            expected.split,
            "case={name}: unexpected split routing domain",
        );
        assert_eq!(
            workspace.debug_route_domain_for_layout(),
            expected.layout,
            "case={name}: unexpected layout routing domain",
        );
        assert_eq!(
            workspace.debug_route_domain_for_move_directional(),
            expected.move_directional,
            "case={name}: unexpected move-directional routing domain",
        );
        assert_eq!(
            workspace.debug_route_domain_for_move_container(),
            expected.move_container,
            "case={name}: unexpected move-container routing domain",
        );
    }
}

#[test]
fn i3_218_floating_container_cannot_be_split_or_relayouted() {
    // Mirrors i3_test_cases/t/218-regress-floating-split.t:
    // layout on a floating leaf is a no-op; split creates one explicit split wrapper.
    let mut params = TestWindowParams::new(1);
    params.is_floating = true;

    let mut layout = check_ops([Op::AddOutput(1), Op::AddWindow { params }]);

    let before_layout = layout
        .active_workspace()
        .expect("active workspace")
        .floating()
        .root_layout_for_window(&1);

    check_ops_on_layout(&mut layout, [Op::SetLayoutStacked]);

    let after_layout = layout
        .active_workspace()
        .expect("active workspace")
        .floating()
        .root_layout_for_window(&1);
    assert_eq!(
        after_layout, before_layout,
        "layout command should be a no-op on floating leaf",
    );

    check_ops_on_layout(&mut layout, [Op::SplitVertical]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(workspace.floating().tiles().count(), 1);
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV),
        "split on floating leaf should create an explicit SplitV wrapper (sway parity)",
    );
}

#[test]
fn i3_192_toggle_layout_all_cycles_floating_container_layouts() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV),
    );

    check_ops_on_layout(&mut layout, [Op::ToggleLayoutAll]);
    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::Stacked),
        "toggle_layout_all should cycle floating container layout from SplitV to Stacked",
    );

    check_ops_on_layout(&mut layout, [Op::ToggleLayoutAll]);
    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::Tabbed),
        "toggle_layout_all should cycle floating container layout from Stacked to Tabbed",
    );

    check_ops_on_layout(&mut layout, [Op::ToggleLayoutAll]);
    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitH),
        "toggle_layout_all should cycle floating container layout from Tabbed to SplitH",
    );
}

#[test]
fn i3_218_toggle_layout_all_on_floating_leaf_is_noop() {
    let mut params = TestWindowParams::new(1);
    params.is_floating = true;

    let mut layout = check_ops([Op::AddOutput(1), Op::AddWindow { params }]);

    let before = layout
        .active_workspace()
        .expect("active workspace")
        .floating()
        .root_layout_for_window(&1);

    check_ops_on_layout(&mut layout, [Op::ToggleLayoutAll]);

    let after = layout
        .active_workspace()
        .expect("active workspace")
        .floating()
        .root_layout_for_window(&1);
    assert_eq!(
        after, before,
        "toggle_layout_all should be a no-op on a floating leaf without an explicit wrapper",
    );
}

#[test]
fn i3_192_set_layout_on_floating_container_with_children_retargets_wrapper() {
    let mut layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::SplitV),
    );

    check_ops_on_layout(&mut layout, [Op::SetLayoutTabbed]);
    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::Tabbed),
        "set_layout should retarget the active floating container wrapper",
    );

    check_ops_on_layout(&mut layout, [Op::SetLayoutStacked]);
    let workspace = layout.active_workspace().expect("active workspace");
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::Stacked),
        "set_layout should continue to mutate the same floating container wrapper",
    );
}

#[test]
fn i3_510_cross_output_focus_uses_target_workspace_mru_leaf() {
    // Mirrors i3_test_cases/t/510-focus-across-outputs.t (#1160 section):
    // crossing outputs should focus the MRU leaf in target workspace, not the geometric first leaf.
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::FocusColumnLeft,
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusColumnOrMonitorRight(2),
    ]);

    let focused = layout
        .focus()
        .map(|win| *win.id())
        .expect("focused window should exist");
    assert_eq!(
        focused, 1,
        "cross-output focus should land on target workspace MRU leaf",
    );
}

#[test]
fn i3_510_cross_output_focus_prefers_tiling_over_destination_floating() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::ToggleWindowFloating { id: Some(3) },
        Op::FocusWindow(2),
        Op::FocusFloating,
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusColumnOrMonitorLeft(1),
    ]);

    let focused = layout
        .focus()
        .map(|win| *win.id())
        .expect("focused window should exist");
    assert_eq!(
        focused, 2,
        "cross-output focus should not land on destination floating when a tiling candidate exists",
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace.is_floating(&2),
        "cross-output focus should land on the destination tiling leaf, not the floating window",
    );
}

#[test]
fn i3_510_cross_output_focus_uses_focused_descendant_in_tabbed_target() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetLayoutTabbed,
        Op::FocusWindow(1),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusColumnOrMonitorRight(2),
    ]);

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(1),
        "cross-output focus should land on the focused tab inside the destination tabbed container",
    );
}

#[test]
fn i3_510_cross_output_focus_uses_focused_descendant_in_stacked_target() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetLayoutStacked,
        Op::FocusWindow(1),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusColumnOrMonitorRight(2),
    ]);

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(1),
        "cross-output focus should land on the focused child inside the destination stacked container",
    );
}

#[test]
fn i3_510_cross_output_focus_uses_nested_focused_descendant() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::FocusOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SplitVertical,
        Op::AddWindow {
            params: TestWindowParams::new(3),
        },
        Op::FocusWindow(3),
        Op::SplitHorizontal,
        Op::AddWindow {
            params: TestWindowParams::new(4),
        },
        Op::FocusWindow(4),
        Op::FocusOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(5),
        },
        Op::FocusColumnOrMonitorRight(2),
    ]);

    assert_eq!(
        layout.focus().map(|win| *win.id()),
        Some(4),
        "cross-output focus should descend into the focused nested leaf of the destination workspace",
    );
}

#[test]
fn i3_520_cross_output_focus_falls_back_to_existing_floating_window() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddOutput(2),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
        Op::FocusOutput(2),
        Op::FocusColumnOrMonitorLeft(1),
    ]);

    let focused = layout
        .focus()
        .map(|win| *win.id())
        .expect("focused window should exist");
    assert_eq!(
        focused, 1,
        "cross-output focus should target the floating window when the destination output has no tiling candidate",
    );

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace.is_floating(&1),
        "cross-output directional focus should land on the floating container itself",
    );
}

#[test]
fn layout_matching_workspace_on_top_level_leaf_keeps_workspace_root_implicit() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetLayoutSplitH,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        workspace
            .tiling()
            .debug_root_is_synthetic_workspace_container(),
        "layout matching workspace layout on a top-level leaf must stay in workspace context",
    );

    let tree = workspace.tiling().debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitH
      Window 1
      Window 2 *
    "
    );
}

#[test]
fn layout_on_top_level_leaf_materializes_explicit_root_wrapper_when_workspace_changes() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::AddWindow {
            params: TestWindowParams::new(2),
        },
        Op::SetLayoutTabbed,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(
        !workspace
            .tiling()
            .debug_root_is_synthetic_workspace_container(),
        "changing workspace-target layout from a top-level leaf must explicitize the root wrapper",
    );

    let tree = workspace.tiling().debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      Window 1
      Window 2 *
    "
    );
}

#[test]
fn i3_550_repeated_split_toggles_on_single_leaf_keep_one_wrapper() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    assert!(harness.tree.split_focused(ContainerLayout::SplitH));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    SplitV
      Window 1 *
    "
    );
}

#[test]
fn i3_550_tabbed_then_stacked_on_single_leaf_keeps_single_wrapper() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.set_focused_layout(ContainerLayout::Stacked));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Stacked
      Window 1 *
    "
    );
}

#[test]
fn i3_550_split_inside_tabbed_keeps_single_nested_split_wrapper() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      SplitV
        Window 1 *
    "
    );
}

#[test]
fn i3_550_toggle_split_inside_tabbed_does_not_create_redundant_wrappers() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      SplitV
        Window 1 *
    "
    );
}

#[test]
fn i3_550_tabbed_with_two_nodes_inside_other_tabbed_stays_two_level() {
    let mut harness = TreeHarness::new();
    harness.add_window(1);

    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));
    assert!(harness.tree.split_focused(ContainerLayout::SplitV));
    harness.add_window(2);
    assert!(harness.tree.set_focused_layout(ContainerLayout::Tabbed));

    let tree = harness.tree.debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      Tabbed
        Window 1
        Window 2 *
    "
    );
}

#[test]
fn i3_550_repeat_tabbed_layout_does_not_create_redundant_wrappers() {
    // Mirrors i3_test_cases/t/550-split-redundant-containers.t ("repeat tabbed layout").
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetLayoutTabbed,
        Op::SetLayoutTabbed,
        Op::SetLayoutTabbed,
        Op::SetLayoutTabbed,
        Op::SetLayoutTabbed,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      Window 1 *
    "
    );
}

#[test]
fn i3_550_split_inside_tabbed_then_back_to_tabbed_flattens_split_wrapper() {
    // Mirrors i3_test_cases/t/550-split-redundant-containers.t
    // ("split v inside tabbed and back to just tabbed").
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::SetLayoutTabbed,
        Op::SplitVertical,
        Op::SetLayoutTabbed,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    let tree = workspace.tiling().debug_tree();
    assert_snapshot!(
        tree.as_str(),
        @"
    Tabbed
      Window 1 *
    "
    );
}

// Insert Position Tests
// These test the logic for determining where windows should be placed during drag-and-drop

#[test]
fn insert_position_empty_workspace_returns_new_column() {
    use super::monitor::InsertPosition;

    let options = Options::from_config(&Config::default());
    let mut layout: Layout<TestWindow> =
        Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Get the workspace without any windows
    let workspace = layout.active_workspace().expect("active workspace");

    // For an empty workspace, insert position should be NewColumn(0)
    let pos = Point::from((100.0, 100.0));
    let insert_pos = workspace.tiling_insert_position(pos);

    assert!(matches!(insert_pos, InsertPosition::NewColumn(0)));
}

#[test]
fn insert_position_with_window_on_top_edge() {
    use super::container::Direction;
    use super::monitor::InsertPosition;

    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add a window
    let params = TestWindowParams::new(1);
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace = layout.active_workspace().expect("active workspace");

    // Position at top edge should indicate SplitRoot with Up direction
    let pos = Point::from((100.0, 0.0));
    let insert_pos = workspace.tiling_insert_position(pos);

    // Should be SplitRoot { direction: Up, ... }
    match insert_pos {
        InsertPosition::SplitRoot { direction, .. } => {
            assert_eq!(direction, Direction::Up);
        }
        other => panic!("Expected SplitRoot with Up, got {:?}", other),
    }
}

#[test]
fn insert_position_with_window_on_bottom_edge() {
    use super::container::Direction;
    use super::monitor::InsertPosition;

    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add a window
    let params = TestWindowParams::new(1);
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace = layout.active_workspace().expect("active workspace");

    // Position at bottom edge should indicate SplitRoot with Down direction
    // Use a very large Y to be at the bottom
    let pos = Point::from((100.0, 10000.0));
    let insert_pos = workspace.tiling_insert_position(pos);

    // Should be SplitRoot { direction: Down, ... }
    match insert_pos {
        InsertPosition::SplitRoot { direction, .. } => {
            assert_eq!(direction, Direction::Down);
        }
        other => panic!("Expected SplitRoot with Down, got {:?}", other),
    }
}

#[test]
fn insert_position_center_of_window() {
    use super::monitor::InsertPosition;

    let options = Options::from_config(&Config::default());
    let mut layout = Layout::with_options(Clock::with_time(Duration::ZERO), options);

    let output = make_test_output("output-test");
    layout.add_output(output.clone(), None);

    // Add a window
    let params = TestWindowParams::new(1);
    layout.add_window(
        TestWindow::new(params),
        AddWindowTarget::Auto,
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let workspace = layout.active_workspace().expect("active workspace");

    // Position in the center of the window area should result in Swap or Split
    // (depending on exact position relative to the window)
    let pos = Point::from((640.0, 360.0)); // center of 1280x720
    let insert_pos = workspace.tiling_insert_position(pos);

    // Should be either Swap or Split (both are valid for center area)
    assert!(
        matches!(
            insert_pos,
            InsertPosition::Swap { .. } | InsertPosition::Split { .. }
        ),
        "Expected Swap or Split at window center, got {:?}",
        insert_pos
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: if std::env::var_os("RUN_SLOW_TESTS").is_none() {
            eprintln!("ignoring slow test");
            0
        } else {
            ProptestConfig::default().cases
        },
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_operations_dont_panic(
        ops: Vec<Op>,
        layout_config in arbitrary_layout_part(),
    ) {
        // eprintln!("{ops:?}");
        let options = Options {
            layout: tiri_config::Layout::from_part(&layout_config),
            ..Default::default()
        };

        check_ops_with_options(options, ops);
    }
}
