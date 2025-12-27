use anyhow::{bail, Context, Result};
use niri_config::{Color, TabBar};
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::{self, Alignment, EllipsizeMode, FontDescription};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Logical, Rectangle, Transform};

use super::container::{Layout, TabBarTab};
use crate::render_helpers::texture::TextureBuffer;
use crate::utils::{round_logical_in_physical_max1, to_physical_precise_round};

pub fn tab_bar_row_height(config: &TabBar, scale: f64) -> f64 {
    let mut height = config.height;
    if height <= 0.0 {
        let mut font = FontDescription::from_string(&config.font);
        font.set_absolute_size(to_physical_precise_round::<f64>(scale, font.size()));

        if let Ok(surface) = ImageSurface::create(cairo::Format::ARgb32, 1, 1) {
            if let Ok(cr) = cairo::Context::new(&surface) {
                let layout = pangocairo::functions::create_layout(&cr);
                layout.context().set_round_glyph_positions(false);
                layout.set_font_description(Some(&font));
                layout.set_text("Ag");
                let (_w, h_px) = layout.pixel_size();
                let font_height = (h_px as f64) / scale;
                height = font_height + config.padding_y * 2.0;
            }
        }
    }

    round_logical_in_physical_max1(scale, height)
}

fn set_source_color(cr: &cairo::Context, color: Color) {
    let [r, g, b, a] = color.to_array_unpremul();
    cr.set_source_rgba(f64::from(r), f64::from(g), f64::from(b), f64::from(a));
}

fn tab_colors(config: &TabBar, tab: &TabBarTab, is_active_workspace: bool) -> (Color, Color) {
    if tab.is_urgent {
        (config.urgent_bg, config.urgent_fg)
    } else if tab.is_focused && is_active_workspace {
        (config.active_bg, config.active_fg)
    } else {
        (config.inactive_bg, config.inactive_fg)
    }
}

pub struct TabBarRenderOutput {
    pub buffer: TextureBuffer<GlesTexture>,
    pub tab_widths_px: Vec<i32>,
}

pub fn render_tab_bar(
    renderer: &mut GlesRenderer,
    config: &TabBar,
    layout: Layout,
    rect: Rectangle<f64, Logical>,
    row_height: f64,
    tabs: &[TabBarTab],
    is_active_workspace: bool,
    scale: f64,
) -> Result<TabBarRenderOutput> {
    let tab_count = tabs.len();
    if tab_count == 0 || rect.size.w <= 0.0 || rect.size.h <= 0.0 {
        bail!("tab bar has no visible size");
    }

    let width_px: i32 = to_physical_precise_round::<i32>(scale, rect.size.w).max(1);
    let height_px: i32 = to_physical_precise_round::<i32>(scale, rect.size.h).max(1);
    let row_height_px: i32 = to_physical_precise_round::<i32>(scale, row_height).max(1);
    let padding_x_px: i32 = to_physical_precise_round::<i32>(scale, config.padding_x).max(0);
    let padding_y_px: i32 = to_physical_precise_round::<i32>(scale, config.padding_y).max(0);
    let separator_width_px: i32 =
        to_physical_precise_round::<i32>(scale, config.separator_width).max(0);

    let mut font = FontDescription::from_string(&config.font);
    font.set_absolute_size(to_physical_precise_round::<f64>(scale, font.size()));

    let tab_widths = if layout == Layout::Tabbed {
        let tab_count_i32 = tab_count as i32;
        let base = width_px / tab_count_i32;
        let mut widths = vec![base.max(1); tab_count];
        let remainder = width_px - base * tab_count_i32;
        for idx in 0..remainder as usize {
            widths[idx] += 1;
        }
        widths
    } else {
        vec![width_px; tab_count]
    };

    let surface = ImageSurface::create(cairo::Format::ARgb32, width_px, height_px)?;
    let cr = cairo::Context::new(&surface)?;
    set_source_color(&cr, config.inactive_bg);
    cr.paint()?;

    let text_layout = pangocairo::functions::create_layout(&cr);
    text_layout.context().set_round_glyph_positions(false);
    text_layout.set_font_description(Some(&font));
    text_layout.set_ellipsize(EllipsizeMode::End);
    text_layout.set_alignment(Alignment::Left);

    let mut cursor_x = 0;
    for (idx, tab) in tabs.iter().enumerate() {
        let width = tab_widths[idx];
        let (x, y, w, h) = if layout == Layout::Tabbed {
            (cursor_x, 0, width, row_height_px)
        } else {
            (0, idx as i32 * row_height_px, width_px, row_height_px)
        };

        let (bg, fg) = tab_colors(config, tab, is_active_workspace);
        set_source_color(&cr, bg);
        cr.rectangle(f64::from(x), f64::from(y), f64::from(w), f64::from(h));
        cr.fill()?;

        let title = if tab.title.trim().is_empty() {
            "untitled"
        } else {
            tab.title.as_str()
        };
        let text_width = (w - padding_x_px * 2).max(1);
        text_layout.set_width(text_width * pango::SCALE);
        text_layout.set_text(title);
        let (_tw, th) = text_layout.pixel_size();
        let text_x = x + padding_x_px;
        let text_area_height = (h - padding_y_px * 2).max(1);
        let text_y = y + padding_y_px + ((text_area_height - th) / 2).max(0);

        set_source_color(&cr, fg);
        cr.move_to(f64::from(text_x), f64::from(text_y));
        pangocairo::functions::show_layout(&cr, &text_layout);

        if separator_width_px > 0 && idx + 1 < tab_count {
            set_source_color(&cr, config.separator_color);
            if layout == Layout::Tabbed {
                cr.rectangle(
                    f64::from(x + w - separator_width_px),
                    f64::from(y),
                    f64::from(separator_width_px),
                    f64::from(h),
                );
            } else {
                cr.rectangle(
                    f64::from(x),
                    f64::from(y + h - separator_width_px),
                    f64::from(w),
                    f64::from(separator_width_px),
                );
            }
            cr.fill()?;
        }

        cursor_x += w;
    }

    let row_count = if layout == Layout::Tabbed { 1 } else { tab_count };
    let extra_height = height_px - row_height_px.saturating_mul(row_count as i32);
    if extra_height > 0 {
        let focused = tabs.iter().find(|tab| tab.is_focused).unwrap_or(&tabs[0]);
        let (bg, _fg) = tab_colors(config, focused, is_active_workspace);
        set_source_color(&cr, bg);
        cr.rectangle(
            0.0,
            f64::from(height_px - extra_height),
            f64::from(width_px),
            f64::from(extra_height),
        );
        cr.fill()?;
    }

    drop(text_layout);
    drop(cr);

    let data = surface
        .take_data()
        .context("failed to read tab bar surface data")?;
    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width_px, height_px),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok(TabBarRenderOutput {
        buffer,
        tab_widths_px: tab_widths,
    })
}
