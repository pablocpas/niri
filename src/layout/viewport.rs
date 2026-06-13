use std::time::Duration;

/// Fixed, output-sized viewport used by the i3/sway-style tiling backend.
///
/// Upstream niri has a horizontally scrolling viewport. Tiri's tiling tree does not, so viewport
/// gestures are explicitly inert instead of being encoded as scattered no-op scrolling methods.
#[derive(Debug, Default)]
pub(super) struct FixedViewport;

impl FixedViewport {
    #[cfg(test)]
    pub(super) fn position(&self) -> f64 {
        0.0
    }

    pub(super) fn activation_distance(&self) -> f64 {
        0.0
    }

    pub(super) fn begin_horizontal_gesture(&mut self, _is_touchpad: bool) {}

    pub(super) fn update_horizontal_gesture(
        &mut self,
        _delta: f64,
        _timestamp: Duration,
        _is_touchpad: bool,
    ) -> Option<bool> {
        None
    }

    pub(super) fn end_horizontal_gesture(&mut self, _cancelled: Option<bool>) -> bool {
        false
    }
}
