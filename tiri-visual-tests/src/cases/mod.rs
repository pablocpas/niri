use std::time::Duration;

use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Size};
use tiri::animation::Clock;

pub mod gradient_angle;
pub mod gradient_area;
pub mod gradient_oklab;
pub mod gradient_oklab_alpha;
pub mod gradient_oklch_alpha;
pub mod gradient_oklch_decreasing;
pub mod gradient_oklch_increasing;
pub mod gradient_oklch_longer;
pub mod gradient_oklch_shorter;
pub mod gradient_srgb;
pub mod gradient_srgb_alpha;
pub mod gradient_srgblinear;
pub mod gradient_srgblinear_alpha;
pub mod layout;
pub mod tile;
pub mod window;

pub struct Args {
    pub size: Size<i32, Logical>,
    pub clock: Clock,
}

pub trait TestCase {
    fn resize(&mut self, _width: i32, _height: i32) {}
    fn are_animations_ongoing(&self) -> bool {
        false
    }
    fn advance_animations(&mut self, _current_time: Duration) {}
    fn render(
        &mut self,
        renderer: &mut GlesRenderer,
        size: Size<i32, Physical>,
    ) -> Vec<Box<dyn RenderElement<GlesRenderer>>>;
}

pub type MakeCaseFn = fn(Args) -> Box<dyn TestCase>;

#[derive(Clone, Copy, Debug)]
pub struct TestCaseSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub make: MakeCaseFn,
}

pub fn all_test_cases() -> &'static [TestCaseSpec] {
    &[
        TestCaseSpec {
            id: "window-freeform",
            title: "Freeform Window",
            make: make_window_freeform,
        },
        TestCaseSpec {
            id: "window-fixed-size",
            title: "Fixed Size Window",
            make: make_window_fixed_size,
        },
        TestCaseSpec {
            id: "window-fixed-size-csd-shadow",
            title: "Fixed Size Window - CSD Shadow",
            make: make_window_fixed_size_csd_shadow,
        },
        TestCaseSpec {
            id: "tile-freeform",
            title: "Freeform Tile",
            make: make_tile_freeform,
        },
        TestCaseSpec {
            id: "tile-fixed-size",
            title: "Fixed Size Tile",
            make: make_tile_fixed_size,
        },
        TestCaseSpec {
            id: "tile-fixed-size-csd-shadow",
            title: "Fixed Size Tile - CSD Shadow",
            make: make_tile_fixed_size_csd_shadow,
        },
        TestCaseSpec {
            id: "tile-freeform-open",
            title: "Freeform Tile - Open",
            make: make_tile_freeform_open,
        },
        TestCaseSpec {
            id: "tile-fixed-size-open",
            title: "Fixed Size Tile - Open",
            make: make_tile_fixed_size_open,
        },
        TestCaseSpec {
            id: "tile-fixed-size-csd-shadow-open",
            title: "Fixed Size Tile - CSD Shadow - Open",
            make: make_tile_fixed_size_csd_shadow_open,
        },
        TestCaseSpec {
            id: "layout-open-in-between",
            title: "Layout - Open In-Between",
            make: make_layout_open_in_between,
        },
        TestCaseSpec {
            id: "layout-open-multiple-quickly",
            title: "Layout - Open Multiple Quickly",
            make: make_layout_open_multiple_quickly,
        },
        TestCaseSpec {
            id: "layout-open-multiple-quickly-big",
            title: "Layout - Open Multiple Quickly - Big",
            make: make_layout_open_multiple_quickly_big,
        },
        TestCaseSpec {
            id: "layout-open-to-the-left",
            title: "Layout - Open To The Left",
            make: make_layout_open_to_the_left,
        },
        TestCaseSpec {
            id: "layout-open-to-the-left-big",
            title: "Layout - Open To The Left - Big",
            make: make_layout_open_to_the_left_big,
        },
        TestCaseSpec {
            id: "layout-tabbed-switching",
            title: "Layout - Tabbed Switching",
            make: make_layout_tabbed_switching,
        },
        TestCaseSpec {
            id: "layout-floating-toggle",
            title: "Layout - Floating Toggle",
            make: make_layout_floating_toggle,
        },
        TestCaseSpec {
            id: "layout-fullscreen-toggle",
            title: "Layout - Fullscreen Toggle",
            make: make_layout_fullscreen_toggle,
        },
        TestCaseSpec {
            id: "gradient-angle",
            title: "Gradient - Angle",
            make: make_gradient_angle,
        },
        TestCaseSpec {
            id: "gradient-area",
            title: "Gradient - Area",
            make: make_gradient_area,
        },
        TestCaseSpec {
            id: "gradient-srgb",
            title: "Gradient - Srgb",
            make: make_gradient_srgb,
        },
        TestCaseSpec {
            id: "gradient-srgblinear",
            title: "Gradient - SrgbLinear",
            make: make_gradient_srgblinear,
        },
        TestCaseSpec {
            id: "gradient-oklab",
            title: "Gradient - Oklab",
            make: make_gradient_oklab,
        },
        TestCaseSpec {
            id: "gradient-oklch-shorter",
            title: "Gradient - Oklch Shorter",
            make: make_gradient_oklch_shorter,
        },
        TestCaseSpec {
            id: "gradient-oklch-longer",
            title: "Gradient - Oklch Longer",
            make: make_gradient_oklch_longer,
        },
        TestCaseSpec {
            id: "gradient-oklch-increasing",
            title: "Gradient - Oklch Increasing",
            make: make_gradient_oklch_increasing,
        },
        TestCaseSpec {
            id: "gradient-oklch-decreasing",
            title: "Gradient - Oklch Decreasing",
            make: make_gradient_oklch_decreasing,
        },
        TestCaseSpec {
            id: "gradient-srgb-alpha",
            title: "Gradient - Srgb Alpha",
            make: make_gradient_srgb_alpha,
        },
        TestCaseSpec {
            id: "gradient-srgblinear-alpha",
            title: "Gradient - SrgbLinear Alpha",
            make: make_gradient_srgblinear_alpha,
        },
        TestCaseSpec {
            id: "gradient-oklab-alpha",
            title: "Gradient - Oklab Alpha",
            make: make_gradient_oklab_alpha,
        },
        TestCaseSpec {
            id: "gradient-oklch-alpha",
            title: "Gradient - Oklch Alpha",
            make: make_gradient_oklch_alpha,
        },
    ]
}

fn make_window_freeform(args: Args) -> Box<dyn TestCase> {
    Box::new(window::Window::freeform(args))
}

fn make_window_fixed_size(args: Args) -> Box<dyn TestCase> {
    Box::new(window::Window::fixed_size(args))
}

fn make_window_fixed_size_csd_shadow(args: Args) -> Box<dyn TestCase> {
    Box::new(window::Window::fixed_size_with_csd_shadow(args))
}

fn make_tile_freeform(args: Args) -> Box<dyn TestCase> {
    Box::new(tile::Tile::freeform(args))
}

fn make_tile_fixed_size(args: Args) -> Box<dyn TestCase> {
    Box::new(tile::Tile::fixed_size(args))
}

fn make_tile_fixed_size_csd_shadow(args: Args) -> Box<dyn TestCase> {
    Box::new(tile::Tile::fixed_size_with_csd_shadow(args))
}

fn make_tile_freeform_open(args: Args) -> Box<dyn TestCase> {
    Box::new(tile::Tile::freeform_open(args))
}

fn make_tile_fixed_size_open(args: Args) -> Box<dyn TestCase> {
    Box::new(tile::Tile::fixed_size_open(args))
}

fn make_tile_fixed_size_csd_shadow_open(args: Args) -> Box<dyn TestCase> {
    Box::new(tile::Tile::fixed_size_with_csd_shadow_open(args))
}

fn make_layout_open_in_between(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::open_in_between(args))
}

fn make_layout_open_multiple_quickly(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::open_multiple_quickly(args))
}

fn make_layout_open_multiple_quickly_big(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::open_multiple_quickly_big(args))
}

fn make_layout_open_to_the_left(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::open_to_the_left(args))
}

fn make_layout_open_to_the_left_big(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::open_to_the_left_big(args))
}

fn make_layout_tabbed_switching(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::tabbed_switching(args))
}

fn make_layout_floating_toggle(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::floating_toggle(args))
}

fn make_layout_fullscreen_toggle(args: Args) -> Box<dyn TestCase> {
    Box::new(layout::Layout::fullscreen_toggle(args))
}

fn make_gradient_angle(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_angle::GradientAngle::new(args))
}

fn make_gradient_area(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_area::GradientArea::new(args))
}

fn make_gradient_srgb(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_srgb::GradientSrgb::new(args))
}

fn make_gradient_srgblinear(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_srgblinear::GradientSrgbLinear::new(args))
}

fn make_gradient_oklab(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklab::GradientOklab::new(args))
}

fn make_gradient_oklch_shorter(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklch_shorter::GradientOklchShorter::new(args))
}

fn make_gradient_oklch_longer(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklch_longer::GradientOklchLonger::new(args))
}

fn make_gradient_oklch_increasing(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklch_increasing::GradientOklchIncreasing::new(
        args,
    ))
}

fn make_gradient_oklch_decreasing(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklch_decreasing::GradientOklchDecreasing::new(
        args,
    ))
}

fn make_gradient_srgb_alpha(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_srgb_alpha::GradientSrgbAlpha::new(args))
}

fn make_gradient_srgblinear_alpha(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_srgblinear_alpha::GradientSrgbLinearAlpha::new(
        args,
    ))
}

fn make_gradient_oklab_alpha(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklab_alpha::GradientOklabAlpha::new(args))
}

fn make_gradient_oklch_alpha(args: Args) -> Box<dyn TestCase> {
    Box::new(gradient_oklch_alpha::GradientOklchAlpha::new(args))
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::all_test_cases;

    #[test]
    fn visual_test_case_ids_are_unique() {
        let mut ids = HashSet::new();
        let mut titles = HashSet::new();

        for case in all_test_cases() {
            assert!(ids.insert(case.id), "duplicate case id: {}", case.id);
            assert!(
                titles.insert(case.title),
                "duplicate case title: {}",
                case.title
            );
        }
    }

    #[test]
    fn visual_test_suite_has_expected_sections() {
        let mut by_prefix: HashMap<&str, usize> = HashMap::new();
        for case in all_test_cases() {
            let prefix = case.id.split('-').next().unwrap_or_default();
            *by_prefix.entry(prefix).or_default() += 1;
        }

        assert_eq!(all_test_cases().len(), 30);
        assert_eq!(by_prefix.get("window"), Some(&3));
        assert_eq!(by_prefix.get("tile"), Some(&6));
        assert_eq!(by_prefix.get("layout"), Some(&8));
        assert_eq!(by_prefix.get("gradient"), Some(&13));
    }
}
