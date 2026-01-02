use niri_config::CornerRadius;
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::renderer::NiriRenderer;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Renderer;
use smithay::utils::{Buffer, Physical, Scale};

#[derive(Debug)]
pub struct InsertHintElement {
    config: niri_config::InsertHint,
    buffer: SolidColorBuffer,
}

#[derive(Debug, Clone)]
pub struct InsertHintRenderElement(pub SolidColorRenderElement);

impl InsertHintElement {
    pub fn new(config: niri_config::InsertHint) -> Self {
        let color = smithay::backend::renderer::Color32F::from(config.color * 0.5);
        Self {
            config,
            buffer: SolidColorBuffer::new(Size::from((0., 0.)), color),
        }
    }

    pub fn update_config(&mut self, config: niri_config::InsertHint) {
        self.config = config;
    }

    pub fn update_shaders(&mut self) {
        // No shaders for the solid rectangle.
    }

    pub fn update_render_elements(
        &mut self,
        size: Size<f64, Logical>,
        view_rect: Rectangle<f64, Logical>,
        radius: CornerRadius,
        scale: f64,
    ) {
        let _ = (view_rect, radius, scale);
        let color = smithay::backend::renderer::Color32F::from(self.config.color * 0.5);
        self.buffer.update(size, color);
    }

    pub fn render(
        &self,
        renderer: &mut impl NiriRenderer,
        location: Point<f64, Logical>,
        push: &mut dyn FnMut(InsertHintRenderElement),
    ) {
        let _ = renderer;
        if self.config.off {
            return;
        }
        let elem = SolidColorRenderElement::from_buffer(
            &self.buffer,
            location,
            1.0,
            Kind::Unspecified,
        );
        push(InsertHintRenderElement(elem));
    }
}

impl Element for InsertHintRenderElement {
    fn id(&self) -> &Id {
        self.0.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.0.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.0.src()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.0.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.0.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.0.alpha()
    }

    fn kind(&self) -> Kind {
        self.0.kind()
    }
}

impl<R: Renderer> RenderElement<R> for InsertHintRenderElement {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        RenderElement::<R>::draw(&self.0, frame, src, dst, damage, opaque_regions)
    }

    #[inline]
    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
