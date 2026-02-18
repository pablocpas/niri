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
        false
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

    assert_eq!(rects.len(), ids.len(), "expected {} visible tiled rects", ids.len());
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
        .pos_in_scrolling_layout
        .expect("window 1 should be tiled");
    let pos2 = window_layout(&layout, id2)
        .pos_in_scrolling_layout
        .expect("window 2 should be tiled");
    let pos3 = window_layout(&layout, id3)
        .pos_in_scrolling_layout
        .expect("window 3 should be tiled");

    // Existing windows should stay in distinct columns after the split operation.
    assert_ne!(pos1.0, pos2.0);
    // Auto-inserted window should not replace existing placements.
    assert_ne!(pos3.0, pos1.0);
    assert_ne!(pos3.0, pos2.0);
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
    assert!(window_layout(&layout, 2).pos_in_scrolling_layout.is_some());
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
    assert!(window_layout(&layout, 2).pos_in_scrolling_layout.is_some());
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
fn tiling_focus_parent_then_split_applies_to_parent_container() {
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
    ]);

    let r1 = tile_rect(&layout, 1);
    let r2 = tile_rect(&layout, 2);

    // The split must apply to the selected parent container, not to the focused leaf.
    assert!((r1.loc.y - r2.loc.y).abs() <= 1.0);
    assert!((r1.loc.x - r2.loc.x).abs() > 1.0);
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
fn floating_focus_parent_selects_wrapper_container() {
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
    assert!(workspace.floating().wrapper_selected_for_window(&1));
    assert!(workspace.floating().selected_is_container(Some(&1)));
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
fn floating_set_layout_mode_uses_wrapper_selection() {
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
        Some(ContainerLayout::Tabbed)
    );
}

#[test]
fn floating_toggle_split_layout_uses_wrapper_selection() {
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
        Op::ToggleSplitLayout,
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
fn floating_toggle_layout_all_uses_wrapper_selection() {
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
        Op::ToggleLayoutAll,
    ]);

    let workspace = layout.active_workspace().expect("active workspace");
    assert!(workspace.is_floating(&1));
    assert!(workspace.is_floating(&2));
    assert_eq!(
        workspace.floating().root_layout_for_window(&1),
        Some(ContainerLayout::Stacked)
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
    assert!(window_layout(&layout, 2).pos_in_scrolling_layout.is_some());
}

#[test]
fn floating_toggle_column_tabbed_display_changes_floating_layout() {
    let layout = check_ops([
        Op::AddOutput(1),
        Op::AddWindow {
            params: TestWindowParams::new(1),
        },
        Op::ToggleWindowFloating { id: None },
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
    layout.toggle_column_tabbed_display();
    layout.add_window(
        TestWindow::new(TestWindowParams::new(2)),
        AddWindowTarget::NextTo(&1),
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

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
                Some((_, HitType::Activate { is_tab_indicator: true }))
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
                Some((_, HitType::Activate { is_tab_indicator: true }))
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
    layout.toggle_column_tabbed_display();
    layout.add_window(
        TestWindow::new(TestWindowParams::new(3)),
        AddWindowTarget::NextTo(&2),
        None,
        None,
        false,
        false,
        ActivateWindow::Yes,
    );

    let rect = tile_rect(&layout, 3);
    let mut hit = None;
    for dy in 1..96 {
        for frac in [0.2, 0.5, 0.8] {
            let candidate = rect.loc + Point::from((rect.size.w * frac, -(dy as f64)));
            if let Some((win, HitType::Activate { is_tab_indicator: true })) =
                layout.window_under(&output, candidate)
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
    assert_ne!(id, 1, "tab bar hit must not fall through to tiling window below");
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
    assert!(ws0.scrolling().tiles().any(|tile| tile.window().id() == &1));
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
        mon.workspaces[0].scrolling().active_column_idx(),
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
        mon.workspaces[1].scrolling().active_column_idx(),
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
    assert_eq!(target_output.as_ref().map(|out| out.name()), Some(output.name()));
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

    assert!(layout.find_workspace_by_name("ws-should-not-be-created").is_none());
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
    assert_eq!(ws.current_output().map(|out| out.name()), Some(output_b.name()));
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
    assert_eq!(names, vec!["20".to_owned(), "10".to_owned(), "30".to_owned()]);
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
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_some());

    let ops = [
        // Unmaximize.
        Op::MaximizeWindowToEdges { id: None },
    ];
    check_ops_on_layout(&mut layout, ops);

    // In tiri, this path now remains in tiling after unmaximize.
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_some());
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
        let scrolling = layout.active_workspace().unwrap().scrolling();
        let tile = scrolling
            .tiles()
            .find(|tile| *tile.window().id() == 1)
            .expect("window 1 should be in scrolling after fullscreen");
        assert_eq!(
            tile.floating_window_size.map(|size| size.w),
            Some(before.size.w as i32),
            "floating width should be preserved while fullscreen"
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
            "window should move to floating immediately on unfullscreen"
        );

        let floating = workspace.floating();
        let tile = floating
            .tiles()
            .find(|tile| *tile.window().id() == 1)
            .expect("window 1 should be in floating");
        assert_eq!(
            tile.floating_window_size.map(|size| size.w),
            Some(before.size.w as i32),
            "stored floating width should still be present when returning to floating"
        );

        let (_mon, win) = layout
            .windows()
            .find(|(_, win)| *win.id() == 1)
            .expect("window 1 should exist");
        assert_eq!(
            win.requested_size().map(|size| size.w),
            Some(before.size.w as i32),
            "unfullscreen-to-floating should request previous floating width"
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
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_some());

    let ops = [
        // Unfullscreen.
        Op::FullscreenWindow(1),
    ];
    check_ops_on_layout(&mut layout, ops);

    // In tiri, this path now remains in tiling after unfullscreen.
    let scrolling = layout.active_workspace().unwrap().scrolling();
    assert!(scrolling.tiles().next().is_some());
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
        let window = TestWindow::new(TestWindowParams::new(id));
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
        let window = TestWindow::new(TestWindowParams::new(id));
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
fn flatten_same_layout_container_on_cleanup() {
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
      Window 1
      Window 4
      Window 2 *
    "
    );
}

#[test]
fn squash_parallel_tabbed_container_on_cleanup() {
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
        !tree.contains("Tabbed"),
        "parallel tabbed container should be squashed:\n{tree}"
    );
}

#[test]
fn squash_parallel_stacked_container_on_cleanup() {
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
        !tree.contains("Stacked"),
        "parallel stacked container should be squashed:\n{tree}"
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
    let insert_pos = workspace.scrolling_insert_position(pos);

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
    let insert_pos = workspace.scrolling_insert_position(pos);

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
    let insert_pos = workspace.scrolling_insert_position(pos);

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
    let insert_pos = workspace.scrolling_insert_position(pos);

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
