use eframe::egui::{self, Color32, Pos2, Response, Sense, Stroke, Ui, pos2, vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Book,
    Menu,
    Bookmark,
    Search,
    Settings,
    Back,
    Next,
    Play,
    Pause,
    Speaker,
    AudioSettings,
    Point,
    Close,
    Sun,
    Follow,
    Grid,
    Note,
    Stop,
    Library,
    List,
    Trash,
    Grip,
    Minimize,
    Maximize,
    Restore,
    Plus,
    Refresh,
}

pub fn button(ui: &mut Ui, icon: Icon, tip: &str, active: bool, accent: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), tip));
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.),
            8.,
            Stroke::new(1.5, accent),
            egui::StrokeKind::Inside,
        );
    }
    if response.hovered() || active {
        ui.painter().rect_filled(
            rect,
            8.0,
            accent.gamma_multiply(if active { 0.15 } else { 0.07 }),
        );
    }
    paint(
        ui.painter(),
        rect.center(),
        icon,
        if active {
            accent
        } else {
            ui.visuals().text_color()
        },
    );
    response.on_hover_text(tip)
}

pub fn nav_row(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    width: f32,
    selected: bool,
    accent: Color32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 36.), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    let ink = ui.visuals().text_color();
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            6.,
            accent.gamma_multiply(if selected { 0.16 } else { 0.07 }),
        );
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.),
            6.,
            Stroke::new(1., accent),
            egui::StrokeKind::Inside,
        );
    }
    paint(
        ui.painter(),
        pos2(rect.left() + 18., rect.center().y),
        icon,
        if selected { accent } else { ink },
    );
    ui.painter().text(
        pos2(rect.left() + 42., rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.),
        if selected { accent } else { ink },
    );
    response
}

/// Text rows stay at the left edge even inside a manually sized UI region.
pub fn label_at(
    ui: &mut Ui,
    rect: egui::Rect,
    text: impl Into<egui::WidgetText>,
    sense: Sense,
) -> Response {
    let text = text.into();
    let response = ui.interact(
        rect,
        ui.scope_id().with((
            "left-label",
            rect.min.x.to_bits(),
            rect.min.y.to_bits(),
            text.text(),
        )),
        sense,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            if sense.senses_click() {
                egui::Role::Button
            } else {
                egui::Role::Label
            },
            ui.is_enabled(),
            text.text(),
        )
    });
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width().max(0.),
        egui::TextStyle::Body,
    );
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(
            pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
            galley,
            ui.visuals().text_color(),
        );
    response
}

pub fn label(
    ui: &mut Ui,
    size: egui::Vec2,
    text: impl Into<egui::WidgetText>,
    sense: Sense,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    label_at(ui, rect, text, sense)
}

pub fn paint(p: &egui::Painter, center: Pos2, icon: Icon, color: Color32) {
    let stroke = Stroke::new(1.7, color);
    let at = |x: f32, y: f32| center + vec2(x, y);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    };
    let path = |points: &[(f32, f32)]| {
        p.add(egui::Shape::line(
            points.iter().map(|(x, y)| at(*x, *y)).collect(),
            stroke,
        ));
    };
    match icon {
        Icon::Book => {
            path(&[
                (-10., -8.),
                (-3., -8.),
                (0., -5.),
                (3., -8.),
                (10., -8.),
                (10., 8.),
                (3., 8.),
                (0., 10.),
                (-3., 8.),
                (-10., 8.),
                (-10., -8.),
            ]);
            line((0., -5.), (0., 10.));
        }
        Icon::Menu => {
            for y in [-7., 0., 7.] {
                line((-9., y), (9., y));
            }
        }
        Icon::Bookmark => path(&[
            (-6., -10.),
            (6., -10.),
            (6., 10.),
            (0., 6.),
            (-6., 10.),
            (-6., -10.),
        ]),
        Icon::Search => {
            p.circle_stroke(at(-2., -2.), 7., stroke);
            line((3., 3.), (10., 10.));
        }
        Icon::Settings => {
            p.circle_stroke(center, 4., stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.;
                let v = vec2(a.cos(), a.sin());
                p.line_segment([center + v * 7., center + v * 10.], stroke);
            }
            p.circle_stroke(center, 8., stroke);
        }
        Icon::Back => path(&[(4., -8.), (-4., 0.), (4., 8.)]),
        Icon::Next => path(&[(-4., -8.), (4., 0.), (-4., 8.)]),
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![at(-5., -9.), at(9., 0.), at(-5., 9.)],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            line((-4., -8.), (-4., 8.));
            line((4., -8.), (4., 8.));
        }
        Icon::Stop => {
            p.rect_filled(
                egui::Rect::from_center_size(center, vec2(12., 12.)),
                2.,
                color,
            );
        }
        Icon::Speaker => {
            path(&[
                (-9., -4.),
                (-5., -4.),
                (1., -9.),
                (1., 9.),
                (-5., 4.),
                (-9., 4.),
                (-9., -4.),
            ]);
            path(&[(5., -5.), (8., -2.), (8., 2.), (5., 5.)]);
        }
        Icon::AudioSettings => {
            path(&[
                (-11., -3.),
                (-8., -3.),
                (-3., -8.),
                (-3., 7.),
                (-8., 2.),
                (-11., 2.),
                (-11., -3.),
            ]);
            p.circle_stroke(at(6., 4.), 4., stroke);
            p.circle_stroke(at(6., 4.), 1.2, stroke);
            for (dx, dy) in [(0., -6.), (0., 6.), (-6., 0.), (6., 0.)] {
                line((6. + dx * 0.75, 4. + dy * 0.75), (6. + dx, 4. + dy));
            }
        }
        Icon::Point => {
            line((-6., -9.), (-6., 9.));
            line((-10., -9.), (-2., -9.));
            line((-10., 9.), (-2., 9.));
            path(&[(2., -6.), (10., 0.), (2., 6.), (2., -6.)]);
        }
        Icon::Close => {
            line((-6., -6.), (6., 6.));
            line((6., -6.), (-6., 6.));
        }
        Icon::Sun => {
            p.circle_stroke(center, 5., stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.;
                let v = vec2(a.cos(), a.sin());
                p.line_segment([center + v * 8., center + v * 11.], stroke);
            }
        }
        Icon::Follow => {
            for y in [-8., -2., 4.] {
                line((-10., y), (0., y));
            }
            line((6., -9.), (6., 9.));
            path(&[(2., 5.), (6., 9.), (10., 5.)]);
        }
        Icon::Grid => {
            for x in [-5., 5.] {
                for y in [-5., 5.] {
                    p.rect_stroke(
                        egui::Rect::from_center_size(at(x, y), vec2(7., 7.)),
                        1.,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
            }
        }
        Icon::Note => {
            path(&[
                (-7., -10.),
                (4., -10.),
                (9., -5.),
                (9., 10.),
                (-7., 10.),
                (-7., -10.),
            ]);
            for y in [-2., 3., 7.] {
                line((-3., y), (5., y));
            }
        }
        Icon::List => {
            for y in [-7., 0., 7.] {
                line((-10., y), (-7., y));
                line((-3., y), (10., y));
            }
        }
        Icon::Library => {
            for (x, h) in [(-8., 18.), (-1., 18.), (6., 15.)] {
                p.rect_stroke(
                    egui::Rect::from_min_size(at(x, -h / 2.), vec2(5., h)),
                    1.,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
            line((-11., 11.), (12., 11.));
        }
        Icon::Trash => {
            line((-9., -6.), (9., -6.));
            path(&[(-6., -6.), (-5., 10.), (5., 10.), (6., -6.)]);
            path(&[(-3., -7.), (-3., -10.), (3., -10.), (3., -7.)]);
            line((-2., -2.), (-2., 6.));
            line((2., -2.), (2., 6.));
        }
        Icon::Grip => {
            for y in [-5., 0., 5.] {
                line((-7., y), (7., y));
            }
        }
        Icon::Minimize => line((-6., 3.), (6., 3.)),
        Icon::Maximize => {
            p.rect_stroke(
                egui::Rect::from_center_size(center, vec2(13., 12.)),
                0.,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Restore => {
            path(&[(-4., -7.), (7., -7.), (7., 4.)]);
            p.rect_stroke(
                egui::Rect::from_center_size(center + vec2(-2., 2.), vec2(12., 11.)),
                0.,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Plus => {
            line((-7., 0.), (7., 0.));
            line((0., -7.), (0., 7.));
        }
        Icon::Refresh => {
            for (start, end) in [(-2.8_f32, -0.25_f32), (0.34_f32, 2.9_f32)] {
                let points = (0..=20)
                    .map(|i| {
                        let a = start + (end - start) * i as f32 / 20.;
                        at(8. * a.cos(), 8. * a.sin())
                    })
                    .collect();
                p.add(egui::Shape::line(points, stroke));
            }
            path(&[(3., -4.), (8., -2.), (10., -7.)]);
            path(&[(-3., 4.), (-8., 2.), (-10., 7.)]);
        }
    }
    let _ = pos2(0.0, 0.0);
}
