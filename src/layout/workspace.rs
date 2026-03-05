use std::cmp::max;
use std::rc::Rc;
use std::time::Duration;

use tiri_config::utils::MergeWith as _;
use tiri_config::{
    CornerRadius, OutputName, PresetSize, Workspace as WorkspaceConfig,
};
use tiri_ipc::{ColumnDisplay, LayoutTreeNode, PositionChange, SizeChange, WindowLayout};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::{layer_map_for_output, Window};
use smithay::input::pointer::CursorIcon;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Serial, Size, Transform};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::SurfaceCachedState;

use super::container::{
    DetachedNode, Direction, InactiveTilingReference, InsertParentInfo, Layout,
};
use super::floating::{
    compute_toplevel_bounds, FloatingResizeResult, FloatingSpace, FloatingSpaceRenderElement,
};
use super::shadow::Shadow;
use super::tile::{Tile, TileRenderSnapshot};
use super::tiling::{Column, ColumnWidth, ScrollDirection, TilingSpace, TilingSpaceRenderElement};
use super::{
    ActivateWindow, HitType, InsertPosition, InteractiveResizeData, LayoutElement, Options,
    RemovedTile, ResizeHit, SizeFrac,
};
use crate::animation::Clock;
use crate::niri_render_elements;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::RenderTarget;
use crate::utils::id::IdCounter;
use crate::utils::transaction::{Transaction, TransactionBlocker};
use crate::utils::{
    center_preferring_top_left_in_area, ensure_min_max_size, ensure_min_max_size_maybe_zero,
    output_size, send_scale_transform, ResizeEdge,
};
use crate::window::ResolvedWindowRules;

#[derive(Debug)]
pub struct Workspace<W: LayoutElement> {
    /// The scrollable-tiling layout.
    scrolling: TilingSpace<W>,

    /// The floating layout.
    floating: FloatingSpace<W>,

    /// Whether the floating layout is active instead of the scrolling layout.
    floating_is_active: FloatingActive,

    /// Match sway command-context where focus can be on workspace while the seat is
    /// still on floating mode (no active floating container target).
    floating_workspace_context: bool,

    /// seat->focus_stack equivalent for tiling restore targets (MRU at index 0).
    inactive_tiling_focus_stack: Vec<InactiveTilingReference>,

    /// The original output of this workspace.
    ///
    /// Most of the time this will be the workspace's current output, however, after an output
    /// disconnection, it may remain pointing to the disconnected output.
    pub(super) original_output: OutputId,

    /// Current output of this workspace.
    output: Option<Output>,

    /// Latest known output scale for this workspace.
    ///
    /// This should be set from the current workspace output, or, if all outputs have been
    /// disconnected, preserved until a new output is connected.
    scale: smithay::output::Scale,

    /// Latest known output transform for this workspace.
    ///
    /// This should be set from the current workspace output, or, if all outputs have been
    /// disconnected, preserved until a new output is connected.
    transform: Transform,

    /// Latest known view size for this workspace.
    ///
    /// This should be computed from the current workspace output size, or, if all outputs have
    /// been disconnected, preserved until a new output is connected.
    view_size: Size<f64, Logical>,

    /// Latest known working area for this workspace.
    ///
    /// Not rounded to physical pixels.
    ///
    /// This is similar to view size, but takes into account things like layer shell exclusive
    /// zones.
    working_area: Rectangle<f64, Logical>,

    /// This workspace's shadow in the overview.
    shadow: Shadow,

    /// This workspace's background.
    background_buffer: SolidColorBuffer,

    /// Clock for driving animations.
    pub(super) clock: Clock,

    /// Configurable properties of the layout as received from the parent monitor.
    pub(super) base_options: Rc<Options>,

    /// Configurable properties of the layout with logical sizes adjusted for the current `scale`.
    pub(super) options: Rc<Options>,

    /// Optional name of this workspace.
    pub(super) name: Option<String>,
    /// Whether the workspace name was auto-assigned for transient numeric access.
    name_is_transient: bool,

    /// Layout config overrides for this workspace.
    layout_config: Option<tiri_config::LayoutPart>,

    /// Unique ID of this workspace.
    id: WorkspaceId,
}

#[derive(Debug, Clone)]
pub struct OutputId(String);

impl OutputId {
    pub fn matches(&self, output: &Output) -> bool {
        let output_name = output.user_data().get::<OutputName>().unwrap();
        output_name.matches(&self.0)
    }
}

static WORKSPACE_ID_COUNTER: IdCounter = IdCounter::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkspaceId(u64);

impl WorkspaceId {
    fn next() -> WorkspaceId {
        WorkspaceId(WORKSPACE_ID_COUNTER.next())
    }

    pub fn get(self) -> u64 {
        self.0
    }

    pub fn specific(id: u64) -> Self {
        Self(id)
    }
}

niri_render_elements! {
    WorkspaceRenderElement<R> => {
        Scrolling = TilingSpaceRenderElement<R>,
        Floating = FloatingSpaceRenderElement<R>,
    }
}

#[derive(Debug)]
pub(super) struct InteractiveResize<W: LayoutElement> {
    pub window: W::Id,
    pub original_window_size: Size<f64, Logical>,
    pub original_window_pos: Option<Point<f64, Logical>>,
    pub original_container_size: Size<f64, Logical>,
    pub resize_container_edges: ResizeEdge,
    pub data: InteractiveResizeData,
}

/// Resolved width or height in logical pixels.
#[derive(Debug, Clone, Copy)]
pub enum ResolvedSize {
    /// Size of the tile including borders.
    Tile(f64),
    /// Size of the window excluding borders.
    Window(f64),
}

/// Whether the floating space is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatingActive {
    /// The scrolling space is active.
    No,
    /// The scrolling space is active, but the floating space should render on top, even if the
    /// active scrolling window is fullscreen.
    ///
    /// This is necessary for focus-follows-mouse that activates but doesn't raise the window to
    /// avoid being annoying.
    NoButRaised,
    /// The floating space is active.
    Yes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandContext {
    Tiling,
    Floating,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandlerContext {
    TilingWindow,
    TilingContainer,
    FloatingWindow,
    FloatingContainer,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InactiveTilingRestoreSource {
    Stack,
    Current,
}

/// Where to put a newly added window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAddWindowTarget<'a, W: LayoutElement> {
    /// No particular preference.
    #[default]
    Auto,
    /// As a new column at this index.
    NewColumnAt(usize),
    /// Next to this existing window.
    NextTo(&'a W::Id),
}

impl OutputId {
    pub fn new(output: &Output) -> Self {
        let output_name = output.user_data().get::<OutputName>().unwrap();
        Self(output_name.format_make_model_serial_or_connector())
    }
}

impl FloatingActive {
    fn get(self) -> bool {
        self == Self::Yes
    }
}

impl HandlerContext {
    fn command_context(self) -> CommandContext {
        match self {
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => CommandContext::Tiling,
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                CommandContext::Floating
            }
            HandlerContext::Workspace => CommandContext::Workspace,
        }
    }

    fn targets_container(self) -> bool {
        matches!(
            self,
            HandlerContext::TilingContainer | HandlerContext::FloatingContainer
        )
    }

    fn has_window_target(self) -> bool {
        matches!(
            self,
            HandlerContext::TilingWindow | HandlerContext::FloatingWindow
        )
    }
}

fn external_resize_cursor_icon(edges: ResizeEdge) -> CursorIcon {
    if edges.contains(ResizeEdge::TOP) && edges.contains(ResizeEdge::LEFT) {
        return CursorIcon::NwResize;
    }
    if edges.contains(ResizeEdge::TOP) && edges.contains(ResizeEdge::RIGHT) {
        return CursorIcon::NeResize;
    }
    if edges.contains(ResizeEdge::BOTTOM) && edges.contains(ResizeEdge::RIGHT) {
        return CursorIcon::SeResize;
    }
    if edges.contains(ResizeEdge::BOTTOM) && edges.contains(ResizeEdge::LEFT) {
        return CursorIcon::SwResize;
    }
    if edges.contains(ResizeEdge::LEFT) {
        return CursorIcon::WResize;
    }
    if edges.contains(ResizeEdge::RIGHT) {
        return CursorIcon::EResize;
    }
    if edges.contains(ResizeEdge::TOP) {
        return CursorIcon::NResize;
    }
    if edges.contains(ResizeEdge::BOTTOM) {
        return CursorIcon::SResize;
    }

    CursorIcon::Default
}

impl<W: LayoutElement> Workspace<W> {
    pub fn new(output: Output, clock: Clock, options: Rc<Options>) -> Self {
        Self::new_with_config(output, None, clock, options)
    }

    pub fn new_with_config(
        output: Output,
        mut config: Option<WorkspaceConfig>,
        clock: Clock,
        base_options: Rc<Options>,
    ) -> Self {
        let original_output = config
            .as_ref()
            .and_then(|c| c.open_on_output.clone())
            .map(OutputId)
            .unwrap_or(OutputId::new(&output));

        let layout_config = config.as_mut().and_then(|c| c.layout.take().map(|x| x.0));

        let scale = output.current_scale();
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(layout_config.as_ref())
                .adjusted_for_scale(scale.fractional_scale()),
        );

        let view_size = output_size(&output);
        let working_area = compute_working_area(&output);

        let scrolling = TilingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let floating = FloatingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, view_size);

        Self {
            scrolling,
            floating,
            floating_is_active: FloatingActive::No,
            floating_workspace_context: false,
            inactive_tiling_focus_stack: Vec::new(),
            original_output,
            scale,
            transform: output.current_transform(),
            view_size,
            working_area,
            shadow: Shadow::new(shadow_config),
            background_buffer: SolidColorBuffer::new(view_size, options.layout.background_color),
            output: Some(output),
            clock,
            base_options,
            options,
            name: config.map(|c| c.name.0),
            name_is_transient: false,
            layout_config,
            id: WorkspaceId::next(),
        }
    }

    pub fn new_with_config_no_outputs(
        mut config: Option<WorkspaceConfig>,
        clock: Clock,
        base_options: Rc<Options>,
    ) -> Self {
        let original_output = OutputId(
            config
                .as_ref()
                .and_then(|c| c.open_on_output.clone())
                .unwrap_or_default(),
        );

        let layout_config = config.as_mut().and_then(|c| c.layout.take().map(|x| x.0));

        let scale = smithay::output::Scale::Integer(1);
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(layout_config.as_ref())
                .adjusted_for_scale(scale.fractional_scale()),
        );

        let view_size = Size::from((1280., 720.));
        let working_area = Rectangle::from_size(Size::from((1280., 720.)));

        let scrolling = TilingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let floating = FloatingSpace::new(
            view_size,
            working_area,
            scale.fractional_scale(),
            clock.clone(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, view_size);

        Self {
            scrolling,
            floating,
            floating_is_active: FloatingActive::No,
            floating_workspace_context: false,
            inactive_tiling_focus_stack: Vec::new(),
            output: None,
            scale,
            transform: Transform::Normal,
            original_output,
            view_size,
            working_area,
            shadow: Shadow::new(shadow_config),
            background_buffer: SolidColorBuffer::new(view_size, options.layout.background_color),
            clock,
            base_options,
            options,
            name: config.map(|c| c.name.0),
            name_is_transient: false,
            layout_config,
            id: WorkspaceId::next(),
        }
    }

    pub fn new_no_outputs(clock: Clock, options: Rc<Options>) -> Self {
        Self::new_with_config_no_outputs(None, clock, options)
    }

    fn assign_default_floating_size_if_missing(
        &self,
        tile: &mut Tile<W>,
        animate: bool,
    ) -> Option<Size<i32, Logical>> {
        if tile.floating_window_size.is_some() {
            return None;
        }

        // Match sway-style default: 50% width x 75% height of the working area.
        let working_size = self.floating.working_area().size;
        let mut size = Size::from((working_size.w * 0.5, working_size.h * 0.75)).to_i32_floor();

        // Respect min/max size constraints from the window.
        let min_size = tile.window().min_size();
        let max_size = tile.window().max_size();
        size.w = ensure_min_max_size(size.w, min_size.w, max_size.w);
        size.h = ensure_min_max_size(size.h, min_size.h, max_size.h);

        tile.floating_window_size = Some(size);
        tile.window_mut().request_size_once(size, animate);
        Some(size)
    }

    pub fn id(&self) -> WorkspaceId {
        self.id
    }

    pub fn name(&self) -> Option<&String> {
        self.name.as_ref()
    }

    pub fn set_name(&mut self, name: String, is_transient: bool) {
        self.name = Some(name);
        self.name_is_transient = is_transient;
    }

    pub fn unname(&mut self) {
        self.name = None;
        self.name_is_transient = false;
    }

    pub fn has_windows_or_name(&self) -> bool {
        self.has_windows() || (self.name.is_some() && !self.name_is_transient)
    }

    pub fn scale(&self) -> smithay::output::Scale {
        self.scale
    }

    pub fn advance_animations(&mut self) {
        self.scrolling.advance_animations();
        self.floating.advance_animations();
    }

    pub fn are_animations_ongoing(&self) -> bool {
        self.scrolling.are_animations_ongoing() || self.floating.are_animations_ongoing()
    }

    pub fn are_transitions_ongoing(&self) -> bool {
        self.scrolling.are_transitions_ongoing() || self.floating.are_transitions_ongoing()
    }

    pub fn update_render_elements(&mut self, is_active: bool) {
        self.scrolling
            .update_render_elements(is_active && !self.floating_is_active.get());

        let view_rect = Rectangle::from_size(self.view_size);
        self.floating
            .update_render_elements(is_active && self.floating_is_active.get(), view_rect);

        self.shadow.update_render_elements(
            self.view_size,
            true,
            CornerRadius::default(),
            self.scale.fractional_scale(),
            1.,
        );
    }

    pub fn update_config(&mut self, base_options: Rc<Options>) {
        let scale = self.scale.fractional_scale();
        let options = Rc::new(
            Options::clone(&base_options)
                .with_merged_layout(self.layout_config.as_ref())
                .adjusted_for_scale(scale),
        );

        self.scrolling.update_config(
            self.view_size,
            self.working_area,
            self.scale.fractional_scale(),
            options.clone(),
        );

        self.floating.update_config(
            self.view_size,
            self.working_area,
            self.scale.fractional_scale(),
            options.clone(),
        );

        let shadow_config =
            compute_workspace_shadow_config(options.overview.workspace_shadow, self.view_size);
        self.shadow.update_config(shadow_config);

        self.background_buffer
            .set_color(options.layout.background_color);

        self.base_options = base_options;
        self.options = options;
    }

    pub fn update_layout_config(&mut self, layout_config: Option<tiri_config::LayoutPart>) {
        if self.layout_config == layout_config {
            return;
        }

        self.layout_config = layout_config;
        self.update_config(self.base_options.clone());
    }

    pub fn update_shaders(&mut self) {
        self.scrolling.update_shaders();
        self.floating.update_shaders();
        self.shadow.update_shaders();
    }

    pub fn windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.tiles().map(Tile::window)
    }

    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut W> + '_ {
        self.tiles_mut().map(Tile::window_mut)
    }

    pub fn tiles(&self) -> impl Iterator<Item = &Tile<W>> + '_ {
        let scrolling = self.scrolling.tiles();
        let floating = self.floating.tiles();
        scrolling.chain(floating)
    }

    pub fn tiles_mut(&mut self) -> impl Iterator<Item = &mut Tile<W>> + '_ {
        let scrolling = self.scrolling.tiles_mut();
        let floating = self.floating.tiles_mut();
        scrolling.chain(floating)
    }

    pub fn is_floating(&self, id: &W::Id) -> bool {
        self.floating.has_window(id)
    }

    fn is_floating_target(&self, window: Option<&W::Id>) -> bool {
        window.map_or(self.floating_is_active.get(), |id| {
            self.floating.has_window(id)
        })
    }

    fn handler_context(&self) -> HandlerContext {
        // Match sway semantics: no floating command context exists when there are
        // no floating containers in the workspace.
        if self.floating.is_empty() || !self.floating_is_active.get() {
            return if self.scrolling.selected_is_container() {
                HandlerContext::TilingContainer
            } else {
                HandlerContext::TilingWindow
            };
        }

        // Match sway handler-context semantics: command routing may remain on a
        // selected tiling container even while focus mode is floating.
        if self.scrolling.selected_is_container() {
            return HandlerContext::TilingContainer;
        }

        if self.floating_workspace_context {
            return HandlerContext::Workspace;
        }

        if self.floating.active_wrapper_selected() || self.floating.selected_is_container(None) {
            HandlerContext::FloatingContainer
        } else {
            HandlerContext::FloatingWindow
        }
    }

    fn command_context(&self) -> CommandContext {
        self.handler_context().command_context()
    }

    pub fn current_output(&self) -> Option<&Output> {
        self.output.as_ref()
    }

    pub fn active_window(&self) -> Option<&W> {
        if self.floating_is_active.get() {
            self.floating.active_window()
        } else {
            self.scrolling.active_window()
        }
    }

    pub fn active_window_mut(&mut self) -> Option<&mut W> {
        if self.floating_is_active.get() {
            self.floating.active_window_mut()
        } else {
            self.scrolling.active_window_mut()
        }
    }

    pub fn active_selection_is_container(&self) -> bool {
        self.handler_context().targets_container()
    }

    pub fn active_command_has_window_target(&self) -> bool {
        self.handler_context().has_window_target()
    }

    pub fn close_window_ids_for_active_selection(&self) -> Vec<W::Id> {
        match self.handler_context() {
            HandlerContext::Workspace => {
                return self.windows().map(|window| window.id().clone()).collect();
            }
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                // Sway kill semantics: when floating wrapper selection is active,
                // command targets the workspace container and closes every window in it.
                if self.floating.active_wrapper_selected() {
                    return self.windows().map(|window| window.id().clone()).collect();
                }

                let ids = self.floating.close_window_ids_for_active_selection();
                if !ids.is_empty() {
                    return ids;
                }
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                let ids = self.scrolling.close_window_ids_for_active_selection();
                if !ids.is_empty() {
                    return ids;
                }
            }
        }

        self.active_window()
            .map(|window| vec![window.id().clone()])
            .unwrap_or_default()
    }

    pub fn is_active_pending_fullscreen(&self) -> bool {
        self.scrolling.is_active_pending_fullscreen()
    }

    pub fn set_output(&mut self, output: Option<Output>) {
        if self.output == output {
            return;
        }

        if let Some(output) = self.output.take() {
            for win in self.windows() {
                win.output_leave(&output);
            }
        }

        self.output = output;

        if let Some(output) = &self.output {
            // Normalize original output: possibly replace connector with make/model/serial.
            if self.original_output.matches(output) {
                self.original_output = OutputId::new(output);
            }

            self.update_output_size();

            for win in self.windows() {
                self.enter_output_for_window(win);
            }
        }
    }

    fn enter_output_for_window(&self, window: &W) {
        if let Some(output) = &self.output {
            window.set_preferred_scale_transform(self.scale, self.transform);
            window.output_enter(output);
        }
    }

    pub fn update_output_size(&mut self) {
        let output = self.output.as_ref().unwrap();
        let scale = output.current_scale();
        let transform = output.current_transform();
        let view_size = output_size(output);
        let working_area = compute_working_area(output);
        self.set_view_size(scale, transform, view_size, working_area);
    }

    fn set_view_size(
        &mut self,
        scale: smithay::output::Scale,
        transform: Transform,
        size: Size<f64, Logical>,
        working_area: Rectangle<f64, Logical>,
    ) {
        let scale_transform_changed = self.transform != transform
            || self.scale.integer_scale() != scale.integer_scale()
            || self.scale.fractional_scale() != scale.fractional_scale();
        if !scale_transform_changed && self.view_size == size && self.working_area == working_area {
            return;
        }

        let fractional_scale_changed = self.scale.fractional_scale() != scale.fractional_scale();

        self.scale = scale;
        self.transform = transform;
        self.view_size = size;
        self.working_area = working_area;

        if fractional_scale_changed {
            // Options need to be recomputed for the new scale.
            self.update_config(self.base_options.clone());
        } else {
            // Pass our existing options as is.
            self.scrolling.update_config(
                size,
                working_area,
                scale.fractional_scale(),
                self.options.clone(),
            );
            self.floating.update_config(
                size,
                working_area,
                scale.fractional_scale(),
                self.options.clone(),
            );

            let shadow_config =
                compute_workspace_shadow_config(self.options.overview.workspace_shadow, size);
            self.shadow.update_config(shadow_config);
        }

        self.background_buffer.resize(size);

        if scale_transform_changed {
            for window in self.windows() {
                window.set_preferred_scale_transform(self.scale, self.transform);
            }
        }
    }

    pub fn view_size(&self) -> Size<f64, Logical> {
        self.view_size
    }

    pub fn make_tile(&self, window: W) -> Tile<W> {
        Tile::new(
            window,
            self.view_size,
            self.scale.fractional_scale(),
            self.clock.clone(),
            self.options.clone(),
        )
    }

    pub fn add_tile(
        &mut self,
        mut tile: Tile<W>,
        target: WorkspaceAddWindowTarget<W>,
        activate: ActivateWindow,
        width: ColumnWidth,
        is_full_width: bool,
        is_floating: bool,
    ) {
        self.enter_output_for_window(tile.window());
        let floating_active = self.floating_is_active.get();
        let workspace_command_context = self.command_context() == CommandContext::Workspace;

        match target {
            WorkspaceAddWindowTarget::Auto => {
                // Match sway: if the active floating container was explicitly split/grouped,
                // the next normal window should join that floating container.
                let grouped_floating = floating_active
                    && !workspace_command_context
                    && !self.scrolling.is_empty()
                    && self.floating.active_container_allows_splits();
                let wants_floating = is_floating || grouped_floating;
                let has_tiling_fullscreen = self.scrolling.has_fullscreen_window();
                if !wants_floating {
                    tile.set_scratchpad(false);
                }
                tile.restore_to_floating = wants_floating;

                let keep_floating_focus = floating_active
                    && !wants_floating
                    && (!self.scrolling.is_empty() || workspace_command_context);
                // Match sway: when a floating container is selected (focus-parent context),
                // opening a new floating window inserts into that container without stealing
                // selection/focus from the container command target.
                let keep_floating_container_selection = floating_active
                    && wants_floating
                    && self.floating.selected_is_container(None);
                let activate = if keep_floating_focus {
                    false
                } else if keep_floating_container_selection {
                    false
                } else if !wants_floating && has_tiling_fullscreen {
                    // Match sway: while a tiling window is fullscreen, newly opened tiling windows
                    // should not steal focus.
                    false
                } else {
                    // Don't steal focus from an active fullscreen window.
                    activate.map_smart(|| !self.is_active_pending_fullscreen())
                };

                // If the tile is pending maximized or fullscreen, open it in the scrolling layout
                // where it can do that.
                if wants_floating
                    && tile.window().pending_sizing_mode().is_normal()
                    && !tile.pending_maximized
                {
                    if floating_active && self.floating.active_container_allows_splits() {
                        self.floating.add_tile_to_active_container(tile, activate);
                    } else {
                        self.floating.add_tile(tile, activate);
                    }

                    if activate || self.scrolling.is_empty() {
                        self.floating_is_active = FloatingActive::Yes;
                    }
                } else {
                    let scrolling_was_empty = self.scrolling.is_empty();
                    self.scrolling
                        .add_tile(None, tile, activate, width, is_full_width, None);


                    if activate
                        || (floating_active
                            && scrolling_was_empty
                            && !wants_floating
                            && !workspace_command_context)
                    {
                        self.floating_is_active = FloatingActive::No;
                    }
                }
            }
            WorkspaceAddWindowTarget::NewColumnAt(col_idx) => {
                if !is_floating {
                    tile.set_scratchpad(false);
                }
                tile.restore_to_floating = is_floating;
                let activate = activate.map_smart(|| false);
                self.scrolling
                    .add_tile(Some(col_idx), tile, activate, width, is_full_width, None);

                if activate {
                    self.floating_is_active = FloatingActive::No;
                }
            }
            WorkspaceAddWindowTarget::NextTo(next_to) => {
                let floating_has_window = self.floating.has_window(next_to);
                let grouped_floating_target =
                    floating_has_window && self.floating.container_allows_splits(next_to);
                let wants_floating = is_floating || grouped_floating_target;
                if !wants_floating {
                    tile.set_scratchpad(false);
                }
                tile.restore_to_floating = wants_floating;

                let activate = activate.map_smart(|| {
                    self.active_window().is_some_and(|win| win.id() == next_to)
                });

                if wants_floating
                    && tile.window().pending_sizing_mode().is_normal()
                    && !tile.pending_maximized
                {
                    if floating_has_window {
                        if grouped_floating_target {
                            self.floating
                                .add_tile_to_container_of(next_to, tile, activate);
                        } else {
                            self.floating.add_tile_above(next_to, tile, activate);
                        }
                    } else {
                        if let Some((next_to_tile, render_pos, _visible)) = self
                            .scrolling
                            .tiles_with_render_positions()
                            .find(|(tile, _, _)| tile.window().id() == next_to)
                        {
                            // Position the new tile in the center above the next_to tile. Think
                            // a dialog opening on top of a window.
                            //
                            // FIXME: use static pos
                            let tile_size = tile.tile_size();
                            let pos = render_pos
                                + (next_to_tile.tile_size().to_point() - tile_size.to_point())
                                    .downscale(2.);
                            let pos = self.floating.clamp_within_working_area(pos, tile_size);
                            let pos = self.floating.logical_to_size_frac(pos);
                            tile.floating_pos = Some(pos);
                        } else {
                            error!(
                                "next_to target disappeared while placing a new floating window"
                            );
                        }
                        self.floating.add_tile(tile, activate);
                    }

                    if activate || self.scrolling.is_empty() {
                        self.floating_is_active = FloatingActive::Yes;
                    }
                } else if floating_has_window {
                    self.scrolling
                        .add_tile(None, tile, activate, width, is_full_width, None);

                    if activate {
                        self.floating_is_active = FloatingActive::No;
                    }
                } else {
                    if self.scrolling.tiles().any(|tile| tile.window().id() == next_to) {
                        self.scrolling
                            .add_tile_right_of(next_to, tile, activate, width, is_full_width);
                    } else {
                        error!("next_to target disappeared while placing a new tiled window");
                        self.scrolling
                            .add_tile(None, tile, activate, width, is_full_width, None);
                    }

                    if activate {
                        self.floating_is_active = FloatingActive::No;
                        self.sync_tiling_focus_context_from_scrolling();
                    }
                }
            }
        }
    }

    pub fn add_tile_to_column(
        &mut self,
        col_idx: usize,
        tile_idx: Option<usize>,
        mut tile: Tile<W>,
        activate: bool,
    ) {
        tile.set_scratchpad(false);
        self.enter_output_for_window(tile.window());
        self.scrolling
            .add_tile_to_column(col_idx, tile_idx, tile, activate);

        if activate {
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub(super) fn scrolling_insert_parent_info(
        &self,
        window: &W::Id,
    ) -> Option<InsertParentInfo> {
        self.scrolling.insert_parent_info_for_window(window)
    }

    fn remember_inactive_tiling_reference(&mut self, reference: InactiveTilingReference) {
        let key = reference.node_key();
        self.inactive_tiling_focus_stack
            .retain(|existing| existing.node_key() != key);
        self.inactive_tiling_focus_stack.insert(0, reference);
        if self.inactive_tiling_focus_stack.len() > 64 {
            self.inactive_tiling_focus_stack.truncate(64);
        }
    }

    fn inactive_tiling_restore_target(
        &mut self,
    ) -> Option<(InsertParentInfo, InactiveTilingRestoreSource)> {
        let debug_restore = std::env::var_os("TIRI_PARITY_DEBUG_RESTORE").is_some();

        // Match sway seat_get_focus_inactive_tiling():
        // if workspace has no tiling nodes, there is no inactive tiling target.
        if self.scrolling.windows().next().is_none() {
            if debug_restore {
                eprintln!("restore_target: no tiling windows");
            }
            return None;
        }

        if debug_restore {
            eprintln!(
                "restore_target: stack={:?}",
                self.inactive_tiling_focus_stack,
            );
        }

        // Match sway: restore target for floating->tiling comes from the seat
        // inactive focus stack first (seat_get_focus_inactive_tiling()).
        let idx = 0;
        while idx < self.inactive_tiling_focus_stack.len() {
            let reference = &self.inactive_tiling_focus_stack[idx];
            if let Some(info) = self
                .scrolling
                .insert_parent_info_from_inactive_tiling_reference_strict(reference)
            {
                if debug_restore {
                    eprintln!("restore_target: from_stack={reference:?} info={info:?}");
                }
                return Some((info, InactiveTilingRestoreSource::Stack));
            }
            if debug_restore {
                eprintln!("restore_target: drop_stale={reference:?}");
            }
            self.inactive_tiling_focus_stack.remove(idx);
        }

        // Fallback only when the inactive stack has no valid tiling references.
        if let Some(reference) = self
            .scrolling
            .inactive_tiling_reference_for_selected_or_focused()
        {
            let info = self
                .scrolling
                .insert_parent_info_from_inactive_tiling_reference(&reference);
            if debug_restore {
                eprintln!("restore_target: from_current={reference:?} info={info:?}");
            }
            return info.map(|info| (info, InactiveTilingRestoreSource::Current));
        }

        if debug_restore {
            eprintln!("restore_target: none");
        }
        None
    }

    fn remember_current_tiling_reference(&mut self) {
        let chain = self
            .scrolling
            .inactive_tiling_reference_chain_for_focused_reference();
        for reference in chain.into_iter().rev() {
            self.remember_inactive_tiling_reference(reference);
        }
    }

    fn remember_current_tiling_focused_leaf_reference(&mut self) {
        let chain = self
            .scrolling
            .inactive_tiling_reference_chain_for_focused_leaf();
        for reference in chain.into_iter().rev() {
            self.remember_inactive_tiling_reference(reference);
        }
    }

    fn sync_tiling_focus_context_from_scrolling(&mut self) {
        self.remember_current_tiling_reference();
    }

    pub(super) fn seat_focus_tiling_chain(
        &self,
    ) -> Vec<super::container::InactiveTilingReference> {
        self.scrolling
            .inactive_tiling_reference_chain_for_focused_reference()
    }

    pub(super) fn has_tiling_reference(
        &self,
        reference: &super::container::InactiveTilingReference,
        strict: bool,
    ) -> bool {
        self.scrolling.has_inactive_tiling_reference(reference, strict)
    }

    pub(super) fn focus_tiling_reference(
        &mut self,
        reference: &super::container::InactiveTilingReference,
        strict: bool,
    ) -> bool {
        let focused = self
            .scrolling
            .focus_inactive_tiling_reference(reference, strict);
        if focused {
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
        }
        focused
    }

    pub(super) fn focus_floating_window(&mut self, id: &W::Id, raise: bool) -> bool {
        let focused = if raise {
            self.floating.activate_window(id)
        } else {
            self.floating.activate_window_without_raising(id)
        };
        if focused {
            self.floating_is_active = FloatingActive::Yes;
        }
        focused
    }

    pub(super) fn scrolling_replace_tile_at_path(
        &mut self,
        path: &[usize],
        tile: Tile<W>,
    ) -> Option<Tile<W>> {
        self.scrolling.replace_tile_at_path(path, tile)
    }

    pub(super) fn scrolling_is_leaf_at_path(&self, path: &[usize]) -> bool {
        self.scrolling.is_leaf_at_path(path)
    }

    pub(super) fn scrolling_insert_tile_with_parent_info(
        &mut self,
        info: &InsertParentInfo,
        tile: Tile<W>,
        activate: bool,
    ) -> bool {
        self.scrolling
            .insert_tile_with_parent_info(info, tile, activate)
    }

    pub fn add_tile_split(
        &mut self,
        target_path: &[usize],
        direction: Direction,
        mut tile: Tile<W>,
        activate: bool,
    ) -> bool {
        tile.set_scratchpad(false);
        self.enter_output_for_window(tile.window());
        tile.restore_to_floating = false;

        let inserted = self
            .scrolling
            .insert_tile_split(target_path, direction, tile, activate);

        if inserted && activate {
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
        }

        inserted
    }

    pub fn add_tile_split_root(
        &mut self,
        direction: Direction,
        mut tile: Tile<W>,
        activate: bool,
    ) -> bool {
        tile.set_scratchpad(false);
        self.enter_output_for_window(tile.window());
        tile.restore_to_floating = false;

        let inserted = self
            .scrolling
            .insert_tile_split_root(direction, tile, activate);

        if inserted && activate {
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
        }

        inserted
    }

    pub fn add_column(&mut self, column: Column<W>, activate: bool) {
        for tile in column.tiles() {
            self.enter_output_for_window(tile.window());
        }

        self.scrolling.add_column(None, column, activate, None);

        if activate {
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    fn update_focus_floating_tiling_after_removing(&mut self, removed_from_floating: bool) {
        if removed_from_floating {
            if self.floating.is_empty() {
                self.floating_is_active = FloatingActive::No;
                self.sync_tiling_focus_context_from_scrolling();
            }
        } else {
            // Scrolling should remain focused if both are empty.
            if self.scrolling.is_empty() && !self.floating.is_empty() {
                self.floating_is_active = FloatingActive::Yes;
            }
        }
    }

    pub fn remove_tile(&mut self, id: &W::Id, transaction: Transaction) -> RemovedTile<W> {
        let mut from_floating = false;
        let removed = if self.floating.has_window(id) {
            from_floating = true;
            self.floating.remove_tile(id)
        } else {
            self.scrolling.remove_tile(id, transaction)
        };

        if let Some(output) = &self.output {
            removed.tile.window().output_leave(output);
        }

        self.update_focus_floating_tiling_after_removing(from_floating);

        removed
    }

    pub fn remove_active_tile(&mut self, transaction: Transaction) -> Option<RemovedTile<W>> {
        let from_floating = self.floating_is_active.get();
        let removed = if from_floating {
            self.floating.remove_active_tile()?
        } else {
            self.scrolling.remove_active_tile(transaction)?
        };

        if let Some(output) = &self.output {
            removed.tile.window().output_leave(output);
        }

        self.update_focus_floating_tiling_after_removing(from_floating);

        Some(removed)
    }

    pub fn remove_active_column(&mut self) -> Option<Column<W>> {
        let from_floating = self.floating_is_active.get();
        if from_floating {
            return None;
        }

        let column = self.scrolling.remove_active_column()?;

        if let Some(output) = &self.output {
            for tile in column.tiles() {
                tile.window().output_leave(output);
            }
        }

        self.update_focus_floating_tiling_after_removing(from_floating);

        Some(column)
    }

    pub fn resolve_default_width(
        &self,
        default_width: Option<Option<PresetSize>>,
        is_floating: bool,
    ) -> Option<PresetSize> {
        match default_width {
            Some(Some(width)) => Some(width),
            Some(None) => None,
            None if is_floating => None,
            None => self.options.layout.default_column_width,
        }
    }

    pub fn resolve_default_height(
        &self,
        default_height: Option<Option<PresetSize>>,
        is_floating: bool,
    ) -> Option<PresetSize> {
        match default_height {
            Some(Some(height)) => Some(height),
            Some(None) => None,
            None if is_floating => None,
            // We don't have a global default at the moment.
            None => None,
        }
    }

    pub fn new_window_size(
        &self,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_floating: bool,
        rules: &ResolvedWindowRules,
        (min_size, max_size): (Size<i32, Logical>, Size<i32, Logical>),
    ) -> Size<i32, Logical> {
        let mut size = if is_floating {
            self.floating.new_window_size(width, height, rules)
        } else {
            self.scrolling.new_window_size(width, height, rules)
        };

        // If the window has a fixed size, or we're picking some fixed size, apply min and max
        // size. This is to ensure that a fixed-size window rule works on open, while still
        // allowing the window freedom to pick its default size otherwise.
        let (min_size, max_size) = rules.apply_min_max_size(min_size, max_size);
        size.w = ensure_min_max_size_maybe_zero(size.w, min_size.w, max_size.w);
        // For scrolling (where height is > 0) only ensure fixed height, since at runtime scrolling
        // will only honor fixed height currently.
        if min_size.h == max_size.h {
            size.h = ensure_min_max_size(size.h, min_size.h, max_size.h);
        } else if size.h > 0 {
            // Also always honor min height, scrolling always does.
            size.h = max(size.h, min_size.h);
        }

        size
    }

    pub fn configure_new_window(
        &self,
        window: &Window,
        width: Option<PresetSize>,
        height: Option<PresetSize>,
        is_floating: bool,
        rules: &ResolvedWindowRules,
    ) {
        window.with_surfaces(|surface, data| {
            send_scale_transform(surface, data, self.scale, self.transform);
        });

        let toplevel = window.toplevel().expect("no x11 support");
        let (min_size, max_size) = with_states(toplevel.wl_surface(), |state| {
            let mut guard = state.cached_state.get::<SurfaceCachedState>();
            let current = guard.current();
            (current.min_size, current.max_size)
        });
        toplevel.with_pending_state(|state| {
            if state.states.contains(xdg_toplevel::State::Fullscreen) {
                state.size = Some(self.view_size.to_i32_round());
            } else if state.states.contains(xdg_toplevel::State::Maximized) {
                state.size = Some(self.working_area.size.to_i32_round());
            } else {
                let size =
                    self.new_window_size(width, height, is_floating, rules, (min_size, max_size));
                state.size = Some(size);
            }

            if is_floating {
                state.bounds = Some(self.floating.new_window_toplevel_bounds(rules));
            } else {
                state.bounds = Some(self.scrolling.new_window_toplevel_bounds(rules));
            }
        });
    }

    pub(super) fn resolve_scrolling_width(
        &self,
        window: &W,
        width: Option<PresetSize>,
    ) -> ColumnWidth {
        let width = width.unwrap_or_else(|| PresetSize::Fixed(window.size().w));
        match width {
            PresetSize::Fixed(fixed) => {
                let mut fixed = f64::from(fixed);

                // Add border width since ColumnWidth includes borders.
                let rules = window.rules();
                let border = self.options.layout.border.merged_with(&rules.border);
                if !border.off {
                    fixed += border.width * 2.;
                }

                ColumnWidth::Fixed(fixed as i32)
            }
            PresetSize::Proportion(prop) => ColumnWidth::Proportion(prop),
        }
    }

    pub fn focus_left(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_left()
        } else {
            let moved = self.scrolling.focus_left();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_left_no_wrap(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_left_no_wrap()
        } else {
            let moved = self.scrolling.focus_left_no_wrap();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_right(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_right()
        } else {
            let moved = self.scrolling.focus_right();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_right_no_wrap(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_right_no_wrap()
        } else {
            let moved = self.scrolling.focus_right_no_wrap();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_column_first(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_leftmost();
        } else {
            self.scrolling.focus_column_first();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_column_last(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_rightmost();
        } else {
            self.scrolling.focus_column_last();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_column_right_or_first(&mut self) {
        if !self.focus_right() {
            self.focus_column_first();
        }
    }

    pub fn focus_column_left_or_last(&mut self) {
        if !self.focus_left() {
            self.focus_column_last();
        }
    }

    pub fn focus_column(&mut self, index: usize) {
        if self.floating_is_active.get() {
            self.focus_tiling();
        }
        self.scrolling.focus_column(index);
        self.sync_tiling_focus_context_from_scrolling();
    }

    pub fn focus_window_in_column(&mut self, index: u8) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.focus_window_in_column(index);
        self.sync_tiling_focus_context_from_scrolling();
    }

    pub fn focus_down(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_down()
        } else {
            let moved = self.scrolling.focus_down();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_up(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_up()
        } else {
            let moved = self.scrolling.focus_up();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_down_or_left(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_down();
        } else {
            self.scrolling.focus_down_or_left();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_down_or_right(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_down();
        } else {
            self.scrolling.focus_down_or_right();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_up_or_left(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_up();
        } else {
            self.scrolling.focus_up_or_left();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_up_or_right(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_up();
        } else {
            self.scrolling.focus_up_or_right();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_window_top(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_topmost();
        } else {
            self.scrolling.focus_top();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_window_bottom(&mut self) {
        if self.floating_is_active.get() {
            self.floating.focus_bottommost();
        } else {
            self.scrolling.focus_bottom();
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn focus_window_down_or_top(&mut self) {
        if !self.focus_down() {
            self.focus_window_top();
        }
    }

    pub fn focus_window_up_or_bottom(&mut self) {
        if !self.focus_up() {
            self.focus_window_bottom();
        }
    }

    pub fn focus_up_no_wrap(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_up_no_wrap()
        } else {
            let moved = self.scrolling.focus_up_no_wrap();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub fn focus_down_no_wrap(&mut self) -> bool {
        if self.floating_is_active.get() {
            self.floating.focus_down_no_wrap()
        } else {
            let moved = self.scrolling.focus_down_no_wrap();
            self.sync_tiling_focus_context_from_scrolling();
            moved
        }
    }

    pub(super) fn focus_entry_from_output_direction(&mut self, direction: Direction) -> bool {
        if self.scrolling.has_fullscreen_window() {
            // Match sway get_node_in_output_direction(): fullscreen workspace target resolves to
            // the inactive focus under the fullscreen subtree. Keep tiling active as-is.
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
            return true;
        }

        let Some((root_layout, child_count)) = self.scrolling.root_layout_and_child_count() else {
            return false;
        };
        if child_count == 0 {
            return false;
        }

        let use_edge = match direction {
            Direction::Left | Direction::Right => {
                matches!(root_layout, Layout::SplitH | Layout::Tabbed)
            }
            Direction::Up | Direction::Down => {
                matches!(root_layout, Layout::SplitV | Layout::Stacked)
            }
        };
        if !use_edge {
            // Match sway get_node_in_output_direction():
            // for non-parallel workspace layout, caller should use seat-level inactive tiling.
            return false;
        }

        match direction {
            Direction::Left | Direction::Up => self.scrolling.focus_column_last(),
            Direction::Right | Direction::Down => self.scrolling.focus_column_first(),
        }
        self.floating_is_active = FloatingActive::No;
        self.sync_tiling_focus_context_from_scrolling();
        true
    }

    pub(super) fn has_tiling_windows(&self) -> bool {
        !self.scrolling.is_empty()
    }

    pub(super) fn focus_workspace_node_like_sway(&mut self) {
        self.scrolling.clear_selection_context();
        self.floating.clear_selection_context();
        if self.floating.is_empty() {
            self.floating_is_active = FloatingActive::No;
            self.floating_workspace_context = false;
            return;
        }

        // Match sway return &ws->node in get_node_in_output_direction():
        // workspace becomes command context while floating mode stays active.
        self.floating_is_active = FloatingActive::Yes;
        self.floating_workspace_context = true;
    }

    pub fn focus_window_by_id(&mut self, id: &W::Id) -> bool {
        if self.floating.has_window(id) {
            if self.floating.focus_window_by_id(id) {
                self.floating_is_active = FloatingActive::Yes;
                return true;
            }
        }

        if self.scrolling.activate_window(id) {
            self.floating_is_active = FloatingActive::No;
            self.sync_tiling_focus_context_from_scrolling();
            return true;
        }

        false
    }

    pub fn move_left(&mut self) -> bool {
        match self.handler_context() {
            HandlerContext::Workspace => false,
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating.move_left();
                true
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => self.scrolling.move_left(),
        }
    }

    pub fn move_right(&mut self) -> bool {
        match self.handler_context() {
            HandlerContext::Workspace => false,
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating.move_right();
                true
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.move_right()
            }
        }
    }

    pub fn move_container_left(&mut self) -> bool {
        if self.floating_is_active.get() {
            return false;
        }
        self.scrolling.move_column_left()
    }

    pub fn move_column_left(&mut self) -> bool {
        self.move_container_left()
    }

    pub fn move_container_right(&mut self) -> bool {
        if self.floating_is_active.get() {
            return false;
        }
        self.scrolling.move_column_right()
    }

    pub fn move_column_right(&mut self) -> bool {
        self.move_container_right()
    }

    pub fn move_container_to_first(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.move_column_to_first();
    }

    pub fn move_column_to_first(&mut self) {
        self.move_container_to_first();
    }

    pub fn move_container_to_last(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.move_column_to_last();
    }

    pub fn move_column_to_last(&mut self) {
        self.move_container_to_last();
    }

    pub fn move_container_to_index(&mut self, index: usize) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.move_column_to_index(index);
    }

    pub fn move_column_to_index(&mut self, index: usize) {
        self.move_container_to_index(index);
    }

    pub fn move_down(&mut self) -> bool {
        match self.handler_context() {
            HandlerContext::Workspace => false,
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating.move_down();
                true
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => self.scrolling.move_down(),
        }
    }

    pub fn move_up(&mut self) -> bool {
        match self.handler_context() {
            HandlerContext::Workspace => false,
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating.move_up();
                true
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => self.scrolling.move_up(),
        }
    }

    pub fn consume_or_expel_window_left(&mut self, window: Option<&W::Id>) {
        if self.is_floating_target(window) {
            self.floating.consume_or_expel_window_left(window);
        } else {
            self.scrolling.consume_or_expel_window_left(window);
        }
    }

    pub fn consume_or_expel_window_right(&mut self, window: Option<&W::Id>) {
        if self.is_floating_target(window) {
            self.floating.consume_or_expel_window_right(window);
        } else {
            self.scrolling.consume_or_expel_window_right(window);
        }
    }

    pub fn consume_into_container(&mut self) {
        if self.floating_is_active.get() {
            self.floating.consume_into_column();
        } else {
            self.scrolling.consume_into_column();
        }
    }

    pub fn consume_into_column(&mut self) {
        self.consume_into_container();
    }

    pub fn expel_from_container(&mut self) {
        if self.floating_is_active.get() {
            self.floating.expel_from_column();
        } else {
            self.scrolling.expel_from_column();
        }
    }

    pub fn expel_from_column(&mut self) {
        self.expel_from_container();
    }

    pub fn swap_window_in_direction(&mut self, direction: ScrollDirection) {
        match self.handler_context() {
            HandlerContext::Workspace => {}
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating.swap_window_in_direction(direction);
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.swap_window_in_direction(direction);
            }
        }
    }

    pub fn toggle_column_tabbed_display(&mut self) {
        if self.floating_is_active.get() {
            self.floating.toggle_column_tabbed_display();
        } else {
            self.scrolling.toggle_column_tabbed_display();
        }
    }

    pub fn set_column_display(&mut self, display: ColumnDisplay) {
        if self.floating_is_active.get() {
            self.floating.set_column_display(display);
        } else {
            self.scrolling.set_column_display(display);
        }
    }

    pub fn center_column(&mut self) {
        if self.floating_is_active.get() {
            self.floating.center_window(None);
        } else {
            self.scrolling.center_column();
        }
    }

    pub fn center_window(&mut self, id: Option<&W::Id>) {
        if self.is_floating_target(id) {
            self.floating.center_window(id);
        } else {
            self.scrolling.center_window(id);
        }
    }

    pub fn center_visible_columns(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.center_visible_columns();
    }

    pub fn toggle_width(&mut self, forwards: bool) {
        if self.floating_is_active.get() {
            self.floating.toggle_window_width(None, forwards);
        } else {
            self.scrolling.toggle_width(forwards);
        }
    }

    pub fn toggle_full_width(&mut self) {
        if self.floating_is_active.get() {
            // Leave this unimplemented for now. For good UX, this probably needs moving the tile
            // to be against the left edge of the working area while it is full-width.
            return;
        }
        self.scrolling.toggle_full_width();
    }

    pub fn set_column_width(&mut self, change: SizeChange) {
        if self.floating_is_active.get() {
            self.floating.set_window_width(None, change, true);
        } else {
            self.scrolling.set_column_width(change);
        }
    }

    pub fn set_window_width(&mut self, window: Option<&W::Id>, change: SizeChange) {
        if self.is_floating_target(window) {
            self.floating.set_window_width(window, change, true);
        } else {
            self.scrolling.set_window_width(window, change);
        }
    }

    pub fn set_window_height(&mut self, window: Option<&W::Id>, change: SizeChange) {
        if self.is_floating_target(window) {
            self.floating.set_window_height(window, change, true);
        } else {
            self.scrolling.set_window_height(window, change);
        }
    }

    pub fn reset_window_height(&mut self, window: Option<&W::Id>) {
        if self.is_floating_target(window) {
            return;
        }
        self.scrolling.reset_window_height(window);
    }

    pub fn toggle_window_width(&mut self, window: Option<&W::Id>, forwards: bool) {
        if self.is_floating_target(window) {
            self.floating.toggle_window_width(window, forwards);
        } else {
            self.scrolling.toggle_window_width(window, forwards);
        }
    }

    pub fn toggle_window_height(&mut self, window: Option<&W::Id>, forwards: bool) {
        if self.is_floating_target(window) {
            self.floating.toggle_window_height(window, forwards);
        } else {
            self.scrolling.toggle_window_height(window, forwards);
        }
    }

    pub fn expand_column_to_available_width(&mut self) {
        if self.floating_is_active.get() {
            return;
        }
        self.scrolling.expand_column_to_available_width();
    }

    pub fn focus_parent(&mut self) {
        match self.handler_context() {
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                // Match sway: when floating focus reaches above the floating container,
                // command context moves to workspace while floating mode remains active.
                self.floating_workspace_context = !self.floating.focus_parent();
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.focus_parent();
                self.sync_tiling_focus_context_from_scrolling();
            }
            HandlerContext::Workspace => {}
        }
    }

    pub fn focus_child(&mut self) {
        match self.handler_context() {
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating.focus_child();
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.focus_child();
                self.sync_tiling_focus_context_from_scrolling();
            }
            // Sway focus_child from workspace context may no-op when no active
            // tiling child can be resolved.
            HandlerContext::Workspace => {}
        }
    }

    pub fn split_horizontal(&mut self) {
        match self.handler_context() {
            // Sway: cmd_split works in both floating and tiling.
            HandlerContext::Workspace => self.scrolling.split_workspace_horizontal(),
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.split_horizontal()
            }
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating_workspace_context = false;
                self.floating.split_horizontal();
            }
        }
    }

    pub fn split_vertical(&mut self) {
        match self.handler_context() {
            // Sway: cmd_split works in both floating and tiling.
            HandlerContext::Workspace => self.scrolling.split_workspace_vertical(),
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.split_vertical()
            }
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating_workspace_context = false;
                self.floating.split_vertical();
            }
        }
    }

    pub fn set_layout_mode(&mut self, layout: Layout) {
        match self.handler_context() {
            HandlerContext::Workspace => {
                if self.scrolling.is_empty() {
                    self.scrolling.set_workspace_layout_hint(layout);
                }
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.set_layout_mode(layout)
            }
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating_workspace_context = false;
                self.floating.set_layout_mode(layout);
            }
        }
    }

    pub fn toggle_split_layout(&mut self) {
        match self.handler_context() {
            HandlerContext::Workspace => {
                if self.scrolling.is_empty() {
                    self.scrolling.toggle_workspace_split_layout();
                }
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.toggle_split_layout()
            }
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating_workspace_context = false;
                self.floating.toggle_split_layout();
            }
        }
    }

    pub fn toggle_layout_all(&mut self) {
        match self.handler_context() {
            HandlerContext::Workspace => {
                if self.scrolling.is_empty() {
                    self.scrolling.toggle_workspace_layout_all();
                }
            }
            HandlerContext::TilingWindow | HandlerContext::TilingContainer => {
                self.scrolling.toggle_layout_all()
            }
            HandlerContext::FloatingWindow | HandlerContext::FloatingContainer => {
                self.floating_workspace_context = false;
                self.floating.toggle_layout_all();
            }
        }
    }

    pub fn set_fullscreen(&mut self, window: &W::Id, is_fullscreen: bool) {
        let restore_to_floating = false;
        if self.floating.has_window(window) {
            if let Some(tile) = self
                .floating
                .tiles_mut()
                .find(|tile| tile.window().id() == window)
            {
                // Match sway semantics: toggling fullscreen on a floating window keeps it in
                // floating mode and toggles the windowed-fullscreen state.
                tile.window_mut().request_windowed_fullscreen(is_fullscreen);
            }
            return;
        } else if !is_fullscreen {
            // The window is in the scrolling layout and we're requesting an unfullscreen. If it is
            // indeed fullscreen (i.e. this isn't a duplicate unfullscreen request), then we may
            // need to unfullscreen into floating.
            let tile = self
                .scrolling
                .tiles()
                .find(|tile| tile.window().id() == window)
                .unwrap();

            // When going from fullscreen to maximized, don't consider restore_to_floating yet.
            // pending_sizing_mode() is asynchronous, so also check scrolling.is_fullscreen() to
            // handle requests while the client is catching up.
            let is_fullscreen_now =
                self.scrolling.is_fullscreen(tile.window())
                    || tile.window().pending_sizing_mode().is_fullscreen();
            if is_fullscreen_now && !tile.pending_maximized {
                if tile.restore_to_floating {
                    // Unfullscreen and float in one call so it has a chance to notice and request a
                    // (0, 0) size, rather than the scrolling tile size.
                    self.toggle_window_floating(Some(window));
                    return;
                }
            }
        }

        let tile = self
            .scrolling
            .tiles()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let was_normal = tile.window().pending_sizing_mode().is_normal();

        self.scrolling.set_fullscreen(window, is_fullscreen);

        // When going from normal to fullscreen, remember if we should unfullscreen to floating.
        let tile = self
            .scrolling
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        if was_normal && !tile.window().pending_sizing_mode().is_normal() {
            tile.restore_to_floating = restore_to_floating;
        }
    }

    pub fn toggle_fullscreen(&mut self, window: &W::Id) {
        if self.floating.has_window(window) {
            let current = self
                .floating
                .tiles()
                .find(|tile| tile.window().id() == window)
                .is_some_and(|tile| tile.window().is_pending_windowed_fullscreen());
            self.set_fullscreen(window, !current);
            return;
        }

        let tile = self
            .tiles()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        // Use scrolling.is_fullscreen() as the source of truth instead of pending_sizing_mode()
        // because pending_sizing_mode() updates asynchronously after animations complete.
        let current = self.scrolling.is_fullscreen(tile.window());
        self.set_fullscreen(window, !current);
    }

    pub fn set_maximized(&mut self, window: &W::Id, maximize: bool) {
        let mut restore_to_floating = false;
        if self.floating.has_window(window) {
            if maximize {
                restore_to_floating = true;
                self.toggle_window_floating(Some(window));
            } else {
                // Floating windows are never maximized, so this is an unmaximize request for an
                // already unmaximized window.
                return;
            }
        } else if !maximize {
            // The window is in the scrolling layout and we're requesting to unmaximize. If it is
            // indeed maximized (i.e. this isn't a duplicate unmaximize request), then we may
            // need to unmaximize into floating.
            let tile = self
                .scrolling
                .tiles()
                .find(|tile| tile.window().id() == window)
                .unwrap();
            if tile.window().pending_sizing_mode().is_fullscreen() {
                self.scrolling.set_maximized(window, maximize);
                return;
            }
            if tile.pending_maximized && tile.restore_to_floating {
                // Unmaximize and float in one call so it has a chance to notice and request a
                // (0, 0) size, rather than the scrolling tile size.
                self.toggle_window_floating(Some(window));
                return;
            }
        }

        let tile = self
            .scrolling
            .tiles()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        let was_normal = tile.window().pending_sizing_mode().is_normal();

        self.scrolling.set_maximized(window, maximize);

        // When going from normal to maximized, remember if we should unmaximize to floating.
        let tile = self
            .scrolling
            .tiles_mut()
            .find(|tile| tile.window().id() == window)
            .unwrap();
        if was_normal && tile.pending_maximized {
            tile.restore_to_floating = restore_to_floating;
        }
    }

    pub fn toggle_maximized(&mut self, window: &W::Id) {
        let current = self
            .scrolling
            .tiles()
            .find(|tile| tile.window().id() == window)
            .is_some_and(|tile| tile.pending_maximized);

        self.set_maximized(window, !current);
    }

    pub fn toggle_window_floating(&mut self, id: Option<&W::Id>) {
        let mut command_context = self.command_context();

        if id.is_none() && command_context == CommandContext::Workspace {
            // Match sway workspace-context toggle behavior:
            // - if tiling is empty, no target exists (no-op);
            // - otherwise operate on tiling root container.
            if self.scrolling.is_empty() {
                return;
            }
            if self.scrolling.select_root_container() {
                command_context = CommandContext::Tiling;
            }
        }

        let explicit_window = id.is_some();
        let active_id = self.active_window().map(|win| win.id().clone());
        let target_is_active = id.is_none_or(|id| Some(id) == active_id.as_ref());
        let preserve_selection_path_on_unfloat =
            if !explicit_window && target_is_active && command_context == CommandContext::Floating {
                self.floating.active_command_container_path()
            } else {
                None
            };
        let Some(id) = id.cloned().or(active_id) else {
            return;
        };
        let tiling_restore_target = if self.floating.has_window(&id) {
            self.inactive_tiling_restore_target()
        } else {
            None
        };

        // Match sway: if a tiling container is selected (focus-parent semantics),
        // floating toggle targets that selected container even if floating focus mode
        // is currently active.
        if !explicit_window
            && target_is_active
            && command_context == CommandContext::Tiling
            && self.scrolling.selected_is_container()
        {
            let old_parent_ref = self
                .scrolling
                .inactive_tiling_reference_for_parent_of_selected_reference();
            if let Some((subtree, origin, rect)) = self.scrolling.take_selected_subtree() {
                let focus_id = subtree
                    .tiles()
                    .into_iter()
                    .any(|tile| tile.window().id() == &id)
                    .then_some(id.clone());
                if let Some(reference) = old_parent_ref {
                    if self
                        .scrolling
                        .insert_parent_info_from_inactive_tiling_reference(&reference)
                        .is_some()
                    {
                        self.remember_inactive_tiling_reference(reference);
                    }
                }
                self.floating
                    .add_subtree(subtree, rect, origin, target_is_active, focus_id.as_ref());
                if target_is_active {
                    if let Some(focus_id) = focus_id.as_ref() {
                        self.floating.select_wrapper_for_window(focus_id);
                    }
                    self.floating_is_active = FloatingActive::Yes;
                    self.floating_workspace_context = false;
                }
            }
            return;
        }

        if self.floating.has_window(&id) {
            // Floating → Tiling: sway's container_set_floating(false) inserts directly
            // using the inactive tiling reference. No tree collapse/normalization.
            if !explicit_window {
                if let Some((subtree, origin, _rect)) = self.floating.take_container_subtree(&id)
                {
                    let scrolling_was_empty = self.scrolling.is_empty();
                    // Match sway container_set_floating(false): when tiling is empty, do not
                    // restore against inactive references/origin; insert directly as workspace
                    // tiling root.
                    let restore_info = if scrolling_was_empty {
                        None
                    } else {
                        match tiling_restore_target.as_ref() {
                            Some((info, InactiveTilingRestoreSource::Current)) => {
                                origin.as_ref().or(Some(info))
                            }
                            Some((info, _)) => Some(info),
                            None => origin.as_ref(),
                        }
                    };
                    if let Some(info) = restore_info {
                        self.scrolling
                            .insert_subtree_with_parent_info(info, subtree, target_is_active);
                    } else {
                        self.scrolling
                            .add_subtree_as_workspace_tiling_fallback(subtree, target_is_active);
                    }

                    if target_is_active {
                        if scrolling_was_empty {
                            let _ = self.scrolling.activate_window(&id);
                            if let Some(path) = preserve_selection_path_on_unfloat.as_ref() {
                                let _ = self.scrolling.select_container_path(path);
                            }
                        }
                        self.floating_is_active = FloatingActive::No;
                        if self.floating.is_empty() {
                            self.floating_workspace_context = false;
                        }
                        self.sync_tiling_focus_context_from_scrolling();
                    }
                    return;
                }
            }
        }

        let render_pos = self
            .tiles_with_render_positions()
            .find(|(tile, _, _)| *tile.window().id() == id)
            .map(|(_, pos, _)| pos);

        if self.floating.has_window(&id) {
            // Single window floating → tiling
            let removed = self.floating.remove_tile(&id);
            let mut tile = removed.tile;
            tile.set_scratchpad(false);
            if !self.scrolling.is_empty() {
                if let Some((info, _)) = tiling_restore_target.as_ref() {
                self.scrolling.insert_subtree_with_parent_info(
                    info,
                    DetachedNode::Leaf(tile),
                    target_is_active,
                );
                } else {
                    self.scrolling
                        .add_tile_as_workspace_tiling_fallback(tile, target_is_active);
                }
            } else {
                self.scrolling
                    .add_tile_as_workspace_tiling_fallback(tile, target_is_active);
            }
            if target_is_active {
                self.floating_is_active = FloatingActive::No;
                if self.floating.is_empty() {
                    self.floating_workspace_context = false;
                }
                self.sync_tiling_focus_context_from_scrolling();
            }
        } else {
            // Tiling → Floating
            let old_parent_ref = if target_is_active {
                self.scrolling
                    .inactive_tiling_reference_for_parent_of_window(&id)
            } else {
                None
            };
            let mut remembered_old_parent_ref = false;
            let mut removed = self.scrolling.remove_tile(&id, Transaction::new());
            if target_is_active {
                if let Some(reference) = old_parent_ref {
                    if self
                        .scrolling
                        .insert_parent_info_from_inactive_tiling_reference(&reference)
                    .is_some()
                    {
                        self.remember_inactive_tiling_reference(reference);
                        remembered_old_parent_ref = true;
                    }
                }
            }
            removed.tile.stop_move_animations();
            removed.tile.pending_maximized = false;

            let stored_or_default = self.floating.stored_or_default_tile_pos(&removed.tile);
            if stored_or_default.is_none() {
                removed.tile.floating_pos = None;
                self.assign_default_floating_size_if_missing(&mut removed.tile, true);
            }

            self.floating
                .add_tile_with_restore_hint(removed.tile, target_is_active);
            if target_is_active {
                self.floating_is_active = FloatingActive::Yes;
                self.floating_workspace_context = false;
                if !remembered_old_parent_ref && !self.scrolling.is_empty() {
                    self.remember_current_tiling_focused_leaf_reference();
                }
            }
        }

        // Animate position transition if possible.
        if let (Some(render_pos), Some((tile, new_render_pos))) = (
            render_pos,
            self.tiles_with_render_positions_mut(false)
                .find(|(tile, _)| *tile.window().id() == id),
        ) {
            tile.animate_move_from(render_pos - new_render_pos);
        }
    }

    pub fn scratchpad_window_id(&self) -> Option<W::Id> {
        self.floating
            .tiles()
            .find(|tile| tile.is_scratchpad())
            .map(|tile| tile.window().id().clone())
    }

    pub fn take_tile_for_scratchpad(&mut self, id: &W::Id) -> Option<Tile<W>> {
        let removed = self.remove_tile(id, Transaction::new());
        let mut tile = removed.tile;
        tile.set_scratchpad(true);
        tile.window_mut().set_floating(true);

        if !removed.is_floating {
            tile.stop_move_animations();
            tile.clear_resize_animation();
            tile.pending_maximized = false;
            // Always center scratchpad windows when first shown.
            tile.floating_pos = None;

            if let Some(size) = self.assign_default_floating_size_if_missing(&mut tile, false) {
                let working_size = self.floating.working_area().size;
                let size_f = Size::from((size.w as f64, size.h as f64));
                let pos = center_preferring_top_left_in_area(self.floating.working_area(), size_f);
                tile.floating_pos = Some(self.floating.logical_to_size_frac(pos));

                let border_config = self.options.layout.border.merged_with(&tile.window().rules().border);
                let bounds = compute_toplevel_bounds(border_config, working_size);
                let win = tile.window_mut();
                win.set_bounds(bounds);
                win.send_pending_configure();
                win.refresh();
            }
        }

        Some(tile)
    }

    pub fn add_scratchpad_tile(&mut self, mut tile: Tile<W>, activate: bool) {
        tile.set_scratchpad(true);
        tile.window_mut().set_floating(true);
        self.enter_output_for_window(tile.window());
        self.floating.add_tile(tile, activate);

        if activate || self.scrolling.is_empty() {
            self.floating_is_active = FloatingActive::Yes;
        }
    }

    pub fn set_window_floating(&mut self, id: Option<&W::Id>, floating: bool) {
        if self.is_floating_target(id) == floating {
            return;
        }

        self.toggle_window_floating(id);
    }

    pub fn focus_floating(&mut self) {
        if !self.floating_is_active.get() {
            self.switch_focus_floating_tiling();
        }
    }

    pub fn focus_tiling(&mut self) {
        if self.floating_is_active.get() {
            self.switch_focus_floating_tiling();
        }
    }

    pub fn switch_focus_floating_tiling(&mut self) {
        if self.floating.is_empty() {
            return;
        } else if self.scrolling.is_empty() {
            return;
        }

        self.scrolling.clear_selection_context();
        self.floating.clear_selection_context();
        if !self.floating_is_active.get() {
            self.remember_current_tiling_reference();
        }
        self.floating_workspace_context = false;
        self.floating_is_active = if self.floating_is_active.get() {
            FloatingActive::No
        } else {
            FloatingActive::Yes
        };
        if !self.floating_is_active.get() {
            self.sync_tiling_focus_context_from_scrolling();
        }
    }

    pub fn clear_selection_context(&mut self) {
        self.scrolling.clear_selection_context();
        self.floating.clear_selection_context();
        self.floating_workspace_context = false;
    }

    pub fn move_floating_window(
        &mut self,
        id: Option<&W::Id>,
        x: PositionChange,
        y: PositionChange,
        animate: bool,
    ) {
        if self.is_floating_target(id) {
            self.floating.move_window(id, x, y, animate);
        } else {
            // If the target tile isn't floating, set its stored floating position.
            let tile = if let Some(id) = id {
                self.scrolling
                    .tiles_mut()
                    .find(|tile| tile.window().id() == id)
                    .unwrap()
            } else if let Some(tile) = self.scrolling.active_tile_mut() {
                tile
            } else {
                return;
            };

            let pos = self.floating.stored_or_default_tile_pos(tile);

            // If there's no stored floating position, we can only set both components at once, not
            // adjust.
            let pos = pos.or_else(|| {
                (matches!(
                    x,
                    PositionChange::SetFixed(_) | PositionChange::SetProportion(_)
                ) && matches!(
                    y,
                    PositionChange::SetFixed(_) | PositionChange::SetProportion(_)
                ))
                .then_some(Point::default())
            });

            let Some(mut pos) = pos else {
                return;
            };

            let working_area = self.floating.working_area();
            let available_width = working_area.size.w;
            let available_height = working_area.size.h;
            let working_area_loc = working_area.loc;

            const MAX_F: f64 = 10000.;

            match x {
                PositionChange::SetFixed(x) => pos.x = x + working_area_loc.x,
                PositionChange::SetProportion(prop) => {
                    let prop = (prop / 100.).clamp(0., MAX_F);
                    pos.x = available_width * prop + working_area_loc.x;
                }
                PositionChange::AdjustFixed(x) => pos.x += x,
                PositionChange::AdjustProportion(prop) => {
                    let current_prop = (pos.x - working_area_loc.x) / available_width.max(1.);
                    let prop = (current_prop + prop / 100.).clamp(0., MAX_F);
                    pos.x = available_width * prop + working_area_loc.x;
                }
            }
            match y {
                PositionChange::SetFixed(y) => pos.y = y + working_area_loc.y,
                PositionChange::SetProportion(prop) => {
                    let prop = (prop / 100.).clamp(0., MAX_F);
                    pos.y = available_height * prop + working_area_loc.y;
                }
                PositionChange::AdjustFixed(y) => pos.y += y,
                PositionChange::AdjustProportion(prop) => {
                    let current_prop = (pos.y - working_area_loc.y) / available_height.max(1.);
                    let prop = (current_prop + prop / 100.).clamp(0., MAX_F);
                    pos.y = available_height * prop + working_area_loc.y;
                }
            }

            let pos = self.floating.logical_to_size_frac(pos);
            tile.floating_pos = Some(pos);
        }
    }

    pub fn has_windows(&self) -> bool {
        self.windows().next().is_some()
    }

    pub fn has_window(&self, window: &W::Id) -> bool {
        self.windows().any(|win| win.id() == window)
    }

    pub fn find_wl_surface(&self, wl_surface: &WlSurface) -> Option<&W> {
        self.windows().find(|win| win.is_wl_surface(wl_surface))
    }

    pub fn find_wl_surface_mut(&mut self, wl_surface: &WlSurface) -> Option<&mut W> {
        self.windows_mut().find(|win| win.is_wl_surface(wl_surface))
    }

    pub fn tiles_with_render_positions(
        &self,
    ) -> impl Iterator<Item = (&Tile<W>, Point<f64, Logical>, bool)> {
        let scrolling = self.scrolling.tiles_with_render_positions();

        let floating = self.floating.tiles_with_render_positions();
        let visible = self.is_floating_visible();
        let floating = floating.map(move |(tile, pos)| (tile, pos, visible));

        floating.chain(scrolling)
    }

    pub fn tiles_with_render_positions_mut(
        &mut self,
        round: bool,
    ) -> impl Iterator<Item = (&mut Tile<W>, Point<f64, Logical>)> {
        let scrolling = self.scrolling.tiles_with_render_positions_mut(round);
        let floating = self.floating.tiles_with_render_positions_mut(round);
        floating.chain(scrolling)
    }

    pub fn tiles_with_ipc_layouts(&self) -> impl Iterator<Item = (&Tile<W>, WindowLayout)> {
        let scrolling = self.scrolling.tiles_with_ipc_layouts();
        let floating = self.floating.tiles_with_ipc_layouts();
        floating.chain(scrolling)
    }

    pub fn active_tile_visual_rectangle(&self) -> Option<Rectangle<f64, Logical>> {
        if self.floating_is_active.get() {
            self.floating.active_tile_visual_rectangle()
        } else {
            self.scrolling.active_tile_visual_rectangle()
        }
    }

    pub fn popup_target_rect(&self, window: &W::Id) -> Option<Rectangle<f64, Logical>> {
        if self.floating.has_window(window) {
            self.floating.popup_target_rect(window)
        } else {
            self.scrolling.popup_target_rect(window)
        }
    }

    pub fn render_scrolling<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        focus_ring: bool,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        let scrolling_focus_ring = focus_ring && !self.floating_is_active();
        self.scrolling
            .render(renderer, target, scrolling_focus_ring, &mut |elem| {
                push(elem.into())
            });
    }

    pub fn render_floating<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        target: RenderTarget,
        focus_ring: bool,
        push: &mut dyn FnMut(WorkspaceRenderElement<R>),
    ) {
        if !self.is_floating_visible() {
            return;
        }

        let view_rect = Rectangle::from_size(self.view_size);
        let floating_focus_ring = focus_ring && self.floating_is_active();
        self.floating.render(
            renderer,
            view_rect,
            target,
            floating_focus_ring,
            &mut |elem| push(elem.into()),
        );
    }

    pub fn render_shadow<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        push: &mut dyn FnMut(ShadowRenderElement),
    ) {
        self.shadow.render(renderer, Point::from((0., 0.)), push);
    }

    pub fn render_background(&self) -> SolidColorRenderElement {
        SolidColorRenderElement::from_buffer(
            &self.background_buffer,
            Point::new(0., 0.),
            1.,
            Kind::Unspecified,
        )
    }

    pub fn render_above_top_layer(&self) -> bool {
        self.scrolling.render_above_top_layer()
    }

    pub fn is_floating_visible(&self) -> bool {
        // If the focus is on a fullscreen scrolling window, hide the floating windows.
        matches!(
            self.floating_is_active,
            FloatingActive::Yes | FloatingActive::NoButRaised
        ) || !self.render_above_top_layer()
    }

    pub fn store_unmap_snapshot_if_empty(&mut self, renderer: &mut GlesRenderer, window: &W::Id) {
        let view_size = self.view_size();
        for (tile, tile_pos) in self.tiles_with_render_positions_mut(false) {
            if tile.window().id() == window {
                let view_pos = Point::from((-tile_pos.x, -tile_pos.y));
                let view_rect = Rectangle::new(view_pos, view_size);
                tile.update_render_elements(
                    false,
                    false,
                    crate::layout::focus_ring::FocusRingEdges::all(),
                    None,
                    view_rect,
                );
                tile.store_unmap_snapshot_if_empty(renderer);
                return;
            }
        }
    }

    pub fn clear_unmap_snapshot(&mut self, window: &W::Id) {
        for tile in self.tiles_mut() {
            if tile.window().id() == window {
                let _ = tile.take_unmap_snapshot();
                return;
            }
        }
    }

    pub fn start_close_animation_for_window(
        &mut self,
        renderer: &mut GlesRenderer,
        window: &W::Id,
        blocker: TransactionBlocker,
    ) {
        if self.floating.has_window(window) {
            self.floating
                .start_close_animation_for_window(renderer, window, blocker);
        } else {
            self.scrolling
                .start_close_animation_for_window(renderer, window, blocker);
        }
    }

    pub fn start_close_animation_for_tile(
        &mut self,
        renderer: &mut GlesRenderer,
        snapshot: TileRenderSnapshot,
        tile_size: Size<f64, Logical>,
        tile_pos: Point<f64, Logical>,
        blocker: TransactionBlocker,
    ) {
        self.floating
            .start_close_animation_for_tile(renderer, snapshot, tile_size, tile_pos, blocker);
    }

    pub fn start_open_animation(&mut self, id: &W::Id) -> bool {
        self.scrolling.start_open_animation(id) || self.floating.start_open_animation(id)
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        if self.is_floating_visible() {
            if let Some(rv) = self.floating.window_under(pos) {
                return Some(rv);
            }
        }

        self.scrolling.window_under(pos)
    }

    pub fn resize_edges_under(&mut self, pos: Point<f64, Logical>) -> Option<ResizeEdge> {
        self.resize_hit_under(pos).map(|hit| hit.edges)
    }

    pub fn resize_hit_under(&mut self, pos: Point<f64, Logical>) -> Option<ResizeHit<W::Id>> {
        if self.is_active_pending_fullscreen() {
            return None;
        }

        if self.is_floating_visible() {
            match self.floating.resize_hit_under(pos) {
                FloatingResizeResult::Hit(hit) => {
                    let cursor = if !hit.external_edges.is_empty() {
                        external_resize_cursor_icon(hit.external_edges)
                    } else {
                        hit.edges.cursor_icon()
                    };
                    return Some(ResizeHit {
                        window: hit.window,
                        edges: hit.edges,
                        cursor,
                        is_floating: true,
                    });
                }
                FloatingResizeResult::Blocked => return None,
                FloatingResizeResult::None => {}
            }
            if self.floating_is_active() {
                return None;
            }
        }

        self.scrolling.resize_hit_under(pos)
    }

    pub fn descendants_added(&mut self, id: &W::Id) -> bool {
        self.floating.descendants_added(id)
    }

    pub fn update_window(&mut self, window: &W::Id, serial: Option<Serial>) {
        if !self.floating.update_window(window, serial) {
            self.scrolling.update_window(window, serial);
        }
    }

    pub fn refresh(&mut self, is_active: bool, is_focused: bool) {
        self.scrolling
            .refresh(is_active && !self.floating_is_active.get(), is_focused);
        self.floating
            .refresh(is_active && self.floating_is_active.get(), is_focused);
    }

    pub fn scroll_amount_to_activate(&self, window: &W::Id) -> f64 {
        if self.floating.has_window(window) {
            return 0.;
        }

        self.scrolling.scroll_amount_to_activate(window)
    }

    pub fn is_urgent(&self) -> bool {
        self.windows().any(|win| win.is_urgent())
    }

    pub fn activate_window(&mut self, window: &W::Id) -> bool {
        if self.floating.activate_window(window) {
            self.floating_is_active = FloatingActive::Yes;
            true
        } else if self.scrolling.activate_window(window) {
            self.floating_is_active = FloatingActive::No;
            true
        } else {
            false
        }
    }

    pub fn activate_window_without_raising(&mut self, window: &W::Id) -> bool {
        if self.floating.activate_window_without_raising(window) {
            self.floating_is_active = FloatingActive::Yes;
            true
        } else if self.scrolling.activate_window(window) {
            self.floating_is_active = match self.floating_is_active {
                FloatingActive::No => FloatingActive::No,
                FloatingActive::NoButRaised => FloatingActive::NoButRaised,
                FloatingActive::Yes => FloatingActive::NoButRaised,
            };
            true
        } else {
            false
        }
    }

    pub(super) fn scrolling_insert_position(&self, pos: Point<f64, Logical>) -> InsertPosition {
        self.scrolling.insert_position(pos)
    }

    pub(super) fn insert_hint_area(
        &self,
        position: &InsertPosition,
    ) -> Option<Rectangle<f64, Logical>> {
        self.scrolling.insert_hint_area(position)
    }

    pub fn view_offset_gesture_begin(&mut self, is_touchpad: bool) {
        self.scrolling.view_offset_gesture_begin(is_touchpad);
    }

    pub fn view_offset_gesture_update(
        &mut self,
        delta_x: f64,
        timestamp: Duration,
        is_touchpad: bool,
    ) -> Option<bool> {
        self.scrolling
            .view_offset_gesture_update(delta_x, timestamp, is_touchpad)
    }

    pub fn view_offset_gesture_end(&mut self, is_touchpad: Option<bool>) -> bool {
        self.scrolling.view_offset_gesture_end(is_touchpad)
    }

    pub fn interactive_resize_begin(&mut self, window: W::Id, edges: ResizeEdge) -> bool {
        if self.floating.has_window(&window) {
            self.floating.interactive_resize_begin(window, edges)
        } else {
            self.scrolling.interactive_resize_begin(window, edges)
        }
    }

    pub fn interactive_resize_begin_at(
        &mut self,
        window: W::Id,
        edges: ResizeEdge,
        pos: Point<f64, Logical>,
    ) -> bool {
        if self.floating.has_window(&window) {
            self.floating.interactive_resize_begin(window, edges)
        } else {
            self.scrolling
                .interactive_resize_begin_at(window, edges, pos)
        }
    }

    pub fn interactive_resize_update(
        &mut self,
        window: &W::Id,
        delta: Point<f64, Logical>,
    ) -> bool {
        if self.floating.has_window(window) {
            self.floating.interactive_resize_update(window, delta)
        } else {
            self.scrolling.interactive_resize_update(window, delta)
        }
    }

    pub fn interactive_resize_end(&mut self, window: Option<&W::Id>) {
        if let Some(window) = window {
            if self.floating.has_window(window) {
                self.floating.interactive_resize_end(Some(window));
            } else {
                self.scrolling.interactive_resize_end(Some(window));
            }
        } else {
            self.floating.interactive_resize_end(None);
            self.scrolling.interactive_resize_end(None);
        }
    }

    pub fn floating_is_active(&self) -> bool {
        self.floating_is_active.get()
    }

    pub fn floating_logical_to_size_frac(
        &self,
        logical_pos: Point<f64, Logical>,
    ) -> Point<f64, SizeFrac> {
        self.floating.logical_to_size_frac(logical_pos)
    }

    pub(super) fn floating_container_allows_splits(&self, id: &W::Id) -> bool {
        self.floating.container_allows_splits(id)
    }

    pub(super) fn floating_container_pos(&self, id: &W::Id) -> Option<Point<f64, Logical>> {
        self.floating.container_pos(id)
    }

    pub(super) fn move_floating_container_for_window_to(
        &mut self,
        id: &W::Id,
        pos: Point<f64, Logical>,
    ) -> bool {
        self.floating.move_container_for_window_to(id, pos, false)
    }

    pub fn working_area(&self) -> Rectangle<f64, Logical> {
        self.working_area
    }

    pub fn layout_config(&self) -> Option<&tiri_config::LayoutPart> {
        self.layout_config.as_ref()
    }

    #[cfg(test)]
    pub fn scrolling(&self) -> &TilingSpace<W> {
        &self.scrolling
    }

    #[cfg(test)]
    pub fn floating(&self) -> &FloatingSpace<W> {
        &self.floating
    }

    #[cfg(test)]
    pub fn debug_inactive_tiling_focus_stack(&self) -> Vec<String> {
        self.inactive_tiling_focus_stack
            .iter()
            .map(|reference| format!("{reference:?}"))
            .collect()
    }

    #[cfg(test)]
    pub fn debug_active_floating_wrapper_selected(&self) -> bool {
        self.floating.active_wrapper_selected()
    }

    #[cfg(test)]
    pub fn debug_active_floating_container_allows_splits(&self) -> bool {
        self.floating.active_container_allows_splits()
    }

    #[cfg(test)]
    pub fn debug_active_floating_command_container_path(&self) -> Option<Vec<usize>> {
        self.floating.active_command_container_path()
    }

    #[cfg(test)]
    pub fn debug_command_context(&self) -> &'static str {
        match self.command_context() {
            CommandContext::Workspace => "workspace",
            CommandContext::Tiling => "tiling",
            CommandContext::Floating => "floating",
        }
    }

    #[cfg(test)]
    pub fn debug_floating_workspace_context(&self) -> bool {
        self.floating_workspace_context
    }

    #[cfg(test)]
    pub fn debug_inactive_tiling_restore_target(&mut self) -> Option<String> {
        self.inactive_tiling_restore_target()
            .map(|(info, source)| format!("{source:?} {info:?}"))
    }

    #[cfg(test)]
    pub fn verify_invariants(&self, move_win_id: Option<&W::Id>) {
        use approx::assert_abs_diff_eq;

        let scale = self.scale.fractional_scale();
        assert!(scale > 0.);
        assert!(scale.is_finite());

        let options = Options::clone(&self.base_options)
            .with_merged_layout(self.layout_config.as_ref())
            .adjusted_for_scale(scale);
        assert_eq!(
            &*self.options, &options,
            "options must be base options adjusted for scale"
        );

        assert!(self.view_size.w > 0.);
        assert!(self.view_size.h > 0.);

        assert_eq!(self.background_buffer.size(), self.view_size);
        assert_eq!(
            self.background_buffer.color().components(),
            options.layout.background_color.to_array_unpremul(),
        );

        assert_eq!(self.view_size, self.scrolling.view_size());
        assert_eq!(self.working_area, self.scrolling.parent_area());
        assert_eq!(&self.clock, self.scrolling.clock());
        assert!(Rc::ptr_eq(&self.options, self.scrolling.options()));
        self.scrolling.verify_invariants();

        assert_eq!(self.view_size, self.floating.view_size());
        assert_eq!(self.working_area, self.floating.working_area());
        assert_eq!(&self.clock, self.floating.clock());
        assert!(Rc::ptr_eq(&self.options, self.floating.options()));
        self.floating.verify_invariants();

        if self.floating.is_empty() {
            assert!(
                !self.floating_is_active.get(),
                "when floating is empty it must never be active"
            );
        } else if self.scrolling.is_empty() {
            assert!(
                self.floating_is_active.get(),
                "when scrolling is empty but floating isn't, floating should be active"
            );
        }

        for (tile, tile_pos, visible) in self.tiles_with_render_positions() {
            if Some(tile.window().id()) != move_win_id {
                assert_eq!(tile.interactive_move_offset, Point::from((0., 0.)));
            }

            let rounded_pos = tile_pos.to_physical_precise_round(scale).to_logical(scale);

            // Tile positions must be rounded to physical pixels.
            assert_abs_diff_eq!(tile_pos.x, rounded_pos.x, epsilon = 1e-5);
            assert_abs_diff_eq!(tile_pos.y, rounded_pos.y, epsilon = 1e-5);

            if let Some(alpha) = &tile.alpha_animation {
                let anim = &alpha.anim;
                if visible {
                    assert_eq!(anim.to(), 1., "visible tiles can animate alpha only to 1");
                }

                assert!(
                    !alpha.hold_after_done,
                    "tiles in the layout cannot have held alpha animation"
                );
            }
        }
    }
}

impl Workspace<crate::window::Mapped> {
    pub(crate) fn layout_tree(&self) -> Option<LayoutTreeNode> {
        if self.floating_is_active.get() {
            self.scrolling.layout_tree_unfocused()
        } else {
            self.scrolling.layout_tree()
        }
    }
}

pub(super) fn compute_working_area(output: &Output) -> Rectangle<f64, Logical> {
    layer_map_for_output(output).non_exclusive_zone().to_f64()
}

fn compute_workspace_shadow_config(
    config: tiri_config::WorkspaceShadow,
    view_size: Size<f64, Logical>,
) -> tiri_config::Shadow {
    // Gaps between workspaces are a multiple of the view height, so shadow settings should also be
    // normalized to the view height to prevent them from overlapping on lower resolutions.
    let norm = view_size.h / 1080.;

    let mut config = tiri_config::Shadow::from(config);
    config.softness *= norm;
    config.spread *= norm;
    config.offset.x.0 *= norm;
    config.offset.y.0 *= norm;

    config
}
