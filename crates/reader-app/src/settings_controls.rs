//! Shared, aligned setting rows for the native settings dialog.
use crate::theme::Palette;
use eframe::egui::{self, Align, Id, Layout, Response, RichText, Ui, pos2, vec2};
use std::ops::RangeInclusive;

pub(crate) fn heading(ui: &mut Ui, title: &str, subtitle: &str, p: Palette) {
    ui.label(RichText::new(title).size(25.).strong().color(p.ink));
    if !subtitle.is_empty() {
        ui.label(RichText::new(subtitle).size(13.).color(p.muted));
    }
    ui.add_space(16.);
}

pub(crate) fn section(ui: &mut Ui, title: &str, p: Palette) {
    ui.add_space(28.);
    ui.label(RichText::new(title).size(16.).strong().color(p.ink));
    ui.add_space(12.);
}

pub(crate) fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui, Id) -> R) -> R {
    ui.push_id(label, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 16.;
            let label = ui
                .allocate_ui_with_layout(
                    vec2(108., 36.),
                    Layout::left_to_right(Align::Center),
                    |ui| ui.label(label),
                )
                .inner;
            let width = ui.available_width().max(80.);
            ui.allocate_ui_with_layout(
                vec2(width, 36.),
                Layout::left_to_right(Align::Center),
                |ui| add(ui, label.id),
            )
            .inner
        })
        .inner
    })
    .inner
}

pub(crate) fn slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    decimals: usize,
    suffix: &str,
) -> Response {
    row(ui, label, |ui, label_id| {
        let slider_width = (ui.available_width() - 90.).max(90.);
        ui.spacing_mut().slider_width = slider_width;
        let slider = ui
            .add_sized(
                [slider_width, 32.],
                egui::Slider::new(value, range.clone())
                    .show_value(false)
                    .trailing_fill(true),
            )
            .labelled_by(label_id);
        let number = ui
            .add_sized(
                [74., 30.],
                egui::DragValue::new(value)
                    .speed(if decimals == 0 { 1.0 } else { 0.01 })
                    .range(range)
                    .fixed_decimals(decimals)
                    .suffix(suffix),
            )
            .labelled_by(label_id);
        slider.union(number)
    })
}

pub(crate) fn check(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 36.), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
        response.request_focus();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *value, label)
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let accent = ui.visuals().hyperlink_color;
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 6., visuals.weak_bg_fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.shrink(1.),
                6.,
                egui::Stroke::new(1., accent),
                egui::StrokeKind::Inside,
            );
        }
        let track =
            egui::Rect::from_center_size(pos2(rect.right() - 20., rect.center().y), vec2(36., 20.));
        ui.painter()
            .rect_filled(track, 10., if *value { accent } else { visuals.bg_fill });
        if !*value {
            ui.painter()
                .rect_stroke(track, 10., visuals.bg_stroke, egui::StrokeKind::Inside);
        }
        let on = ui.ctx().animate_bool_responsive(response.id, *value);
        ui.painter().circle_filled(
            pos2(
                egui::lerp((track.left() + 10.)..=(track.right() - 10.), on),
                track.center().y,
            ),
            7.,
            if *value {
                egui::Color32::WHITE
            } else {
                visuals.fg_stroke.color
            },
        );
        let label_rect =
            egui::Rect::from_min_max(rect.min, pos2(track.left() - 16., rect.bottom()));
        let galley = egui::WidgetText::from(label).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            label_rect.width().max(0.),
            egui::TextStyle::Body,
        );
        ui.painter()
            .with_clip_rect(label_rect.intersect(ui.clip_rect()))
            .galley(
                pos2(
                    label_rect.left(),
                    label_rect.center().y - galley.size().y * 0.5,
                ),
                galley,
                visuals.fg_stroke.color,
            );
    }
    response
}
