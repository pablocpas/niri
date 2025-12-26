use anyhow::{bail, Context, Result};
use niri_config::{Color, TabBar};
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::{self, Alignment, EllipsizeMode, FontDescription};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Logical, Rectangle, Transform};

use super::container::{Layout, TabBarTab};
use crate::render_helpers::texture::TextureBuffer;
use crate::utils::to_physical_precise_round;

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

    let measure_surface = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let measure_cr = cairo::Context::new(&measure_surface)?;
    let measure_layout = pangocairo::functions::create_layout(&measure_cr);
    measure_layout.context().set_round_glyph_positions(false);
    measure_layout.set_font_description(Some(&font));
    measure_layout.set_ellipsize(EllipsizeMode::End);
    measure_layout.set_alignment(Alignment::Left);

    let mut text_widths = Vec::with_capacity(tab_count);
    for tab in tabs {
        let title = if tab.title.trim().is_empty() {
            "untitled"
        } else {
            tab.title.as_str()
        };
        measure_layout.set_width(-1);
        measure_layout.set_text(title);
        let (w, _h) = measure_layout.pixel_size();
        text_widths.push(w);
    }

    let tab_widths = if layout == Layout::Tabbed {
        let mut widths: Vec<i32> = text_widths
            .iter()
            .map(|w| w.saturating_add(padding_x_px * 2))
            .collect();
        let total: i32 = widths.iter().sum();
        let min_width = (padding_x_px * 2).max(1);
        let tab_count_i32 = tab_count as i32;

        if total <= width_px {
            if let Some(last) = widths.last_mut() {
                *last += width_px - total;
            }
            widths
        } else if min_width.saturating_mul(tab_count_i32) > width_px {
            let base = width_px / tab_count_i32;
            let mut widths = vec![base; tab_count];
            let mut remainder = width_px - base * tab_count_i32;
            let mut idx = 0;
            while remainder > 0 {
                widths[idx] += 1;
                remainder -= 1;
                idx = (idx + 1) % tab_count;
            }
            widths
        } else {
            let scale = width_px as f64 / total as f64;
            widths = widths
                .iter()
                .map(|w| ((*w as f64 * scale).floor() as i32).max(min_width))
                .collect();

            let scaled_total: i32 = widths.iter().sum();
            if scaled_total < width_px {
                if let Some(last) = widths.last_mut() {
                    *last += width_px - scaled_total;
                }
                widths
            } else if scaled_total > width_px {
                let mut deficit = scaled_total - width_px;
                for width in widths.iter_mut().rev() {
                    if deficit == 0 {
                        break;
                    }
                    let slack = (*width - min_width).max(0);
                    if slack == 0 {
                        continue;
                    }
                    let take = slack.min(deficit);
                    *width -= take;
                    deficit -= take;
                }

                if deficit > 0 {
                    let base = width_px / tab_count_i32;
                    let mut widths = vec![base; tab_count];
                    let remainder = width_px - base * tab_count_i32;
                    for idx in 0..remainder as usize {
                        widths[idx] += 1;
                    }
                    widths
                } else {
                    widths
                }
            } else {
                widths
            }
        }
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
