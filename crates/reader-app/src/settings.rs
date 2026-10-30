use crate::{
    actions::{self, Action},
    app::{ReaderApp, SettingTab, region},
    icons::{self, Icon},
    layout, reading_surface, settings_controls as controls,
    theme::Palette,
};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Id, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};
use reader_library::{DictionaryCatalog, DictionaryStatus, ReadingMode, SpreadMode, Theme};

#[derive(Clone)]
struct DictionaryDrag {
    id: String,
}
#[derive(Clone)]
struct NativePreview {
    key: String,
    page: layout::Page,
}

impl ReaderApp {
    pub(crate) fn settings_view(&mut self, ui: &mut Ui, p: Palette) {
        let before = serde_json::to_string(&self.settings).unwrap_or_default();
        let available = ui.max_rect();
        let width = available.width();
        let height = available.height();
        ui.painter().rect_filled(available, 0., p.paper);
        region(ui, "settings-full-page", available, |ui| {
            ui.set_min_size(vec2(width, height));
            ui.set_max_size(vec2(width, height));
            ui.spacing_mut().item_spacing = vec2(10., 8.);
            let all = Rect::from_min_size(ui.cursor().min, vec2(width, height));
            let header = Rect::from_min_size(all.min, vec2(width, 60.));
            ui.painter().line_segment(
                [header.left_bottom(), header.right_bottom()],
                Stroke::new(1., p.border),
            );
            region(
                ui,
                "settings-header",
                header.shrink2(vec2(18., 10.)),
                |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.label(RichText::new("設定").size(21.).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icons::button(ui, Icon::Close, "設定を閉じる", false, p.accent)
                                .clicked()
                            {
                                self.binding_capture = None;
                                self.screen = self.previous_screen;
                            }
                        });
                    });
                },
            );
            let nav = Rect::from_min_max(
                pos2(all.left(), header.bottom()),
                pos2(all.left() + 204., all.bottom()),
            );
            ui.painter().rect_filled(nav, 0., p.panel);
            region(
                ui,
                "settings-categories",
                nav.shrink2(vec2(10., 18.)),
                |ui| {
                    for (tab, icon, label) in [
                        (SettingTab::Appearance, Icon::Sun, "外観"),
                        (SettingTab::Reading, Icon::Book, "読書"),
                        (SettingTab::Speech, Icon::Speaker, "音声"),
                        (SettingTab::Dictionaries, Icon::Note, "辞書"),
                        (SettingTab::Library, Icon::Library, "本棚・Calibre"),
                        (SettingTab::Keys, Icon::Settings, "キー割り当て"),
                    ] {
                        if icons::nav_row(
                            ui,
                            icon,
                            label,
                            ui.available_width(),
                            self.setting_tab == tab,
                            p.accent,
                        )
                        .clicked()
                        {
                            self.setting_tab = tab;
                        }
                        ui.add_space(2.);
                    }
                },
            );
            let body = Rect::from_min_max(
                pos2(nav.right() + 24., header.bottom() + 22.),
                all.max - vec2(24., 20.),
            );
            region(ui, "settings-dialog-content", body, |ui| {
                if self.setting_tab == SettingTab::Appearance {
                    self.appearance_settings(ui, p);
                } else {
                    if self.setting_tab == SettingTab::Keys {
                        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("settings-section", self.setting_tab as u8))
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(if self.setting_tab == SettingTab::Keys {
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible
                        } else {
                            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                        })
                        .show(ui, |ui| match self.setting_tab {
                            SettingTab::Appearance => {}
                            SettingTab::Reading => self.reading_settings(ui, p),
                            SettingTab::Speech => self.speech_settings(ui, p),
                            SettingTab::Dictionaries => self.dictionary_settings(ui, p),
                            SettingTab::Library => self.library_settings(ui, p),
                            SettingTab::Keys => self.key_settings(ui, p),
                        });
                }
            });
        });
        if before != serde_json::to_string(&self.settings).unwrap_or_default() {
            self.save_settings();
        }
    }

    fn appearance_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::heading(ui, "外観", "", p);
        let area = ui.available_rect_before_wrap();
        let wide = area.width() >= 760.;
        let preview_width = if wide {
            (area.width() * 0.39).clamp(270., 350.)
        } else {
            0.
        };
        let form = if wide {
            Rect::from_min_max(
                area.min,
                pos2(area.right() - preview_width - 24., area.bottom()),
            )
        } else {
            area
        };
        region(ui, "appearance-controls", form, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("appearance-controls-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !wide {
                        egui::CollapsingHeader::new("表示全体のプレビュー")
                            .default_open(false)
                            .show(ui, |ui| self.appearance_preview(ui, p));
                        ui.add_space(8.);
                    }
                    self.appearance_form(ui, p);
                });
        });
        if wide {
            let preview = Rect::from_min_max(pos2(form.right() + 24., area.top()), area.max);
            region(ui, "appearance-native-preview", preview, |ui| {
                self.appearance_preview(ui, p)
            });
        }
    }

    fn appearance_form(&mut self, ui: &mut Ui, p: Palette) {
        ui.label(RichText::new("テーマ").strong());
        ui.add_space(4.);
        let gap = 10.;
        let columns = if ui.available_width() >= 460. { 3 } else { 2 };
        let width = ((ui.available_width() - gap * (columns - 1) as f32) / columns as f32).max(80.);
        for themes in Theme::ALL.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &theme in themes {
                    let base = Palette::for_theme(theme, self.settings.accent);
                    let (rect, response) = ui.allocate_exact_size(vec2(width, 90.), Sense::click());
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, theme.label())
                    });
                    let selected = self.settings.theme == theme
                        && self.settings.custom_background.is_none()
                        && self.settings.custom_foreground.is_none();
                    ui.painter().rect_filled(rect, 7., base.paper);
                    ui.painter().rect_stroke(
                        rect,
                        7.,
                        Stroke::new(
                            if selected { 2. } else { 1. },
                            if selected { p.accent } else { p.border },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    for (column, text) in ["頁をひらく", "朝の光を読む"].into_iter().enumerate()
                    {
                        for (index, ch) in text.chars().take(5).enumerate() {
                            ui.painter().text(
                                pos2(
                                    rect.center().x + 12. - column as f32 * 16.,
                                    rect.top() + 11. + index as f32 * 10.,
                                ),
                                Align2::CENTER_TOP,
                                ch,
                                FontId::proportional(9.),
                                base.ink,
                            );
                        }
                    }
                    ui.painter().text(
                        pos2(rect.center().x, rect.bottom() - 13.),
                        Align2::CENTER_CENTER,
                        theme.label(),
                        FontId::proportional(12.),
                        base.ink,
                    );
                    if response.clicked() {
                        self.settings.set_theme(theme);
                    }
                }
            });
        }
        controls::section(ui, "カスタム配色", p);
        let base = Palette::for_theme(self.settings.theme, self.settings.accent);
        color_override(
            ui,
            "背景色",
            &mut self.settings.custom_background,
            base.paper,
        );
        color_override(ui, "文字色", &mut self.settings.custom_foreground, base.ink);
        controls::check(
            ui,
            &mut self.settings.adapt_monochrome_images,
            "単色の外字・数式画像を文字色に合わせる",
        );
        controls::section(ui, "アクセントカラー", p);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.;
            for (color, label) in [
                ([37, 99, 235], "青"),
                ([39, 120, 107], "緑"),
                ([192, 129, 24], "琥珀"),
                ([146, 80, 188], "紫"),
            ] {
                let c = Color32::from_rgb(color[0], color[1], color[2]);
                let (rect, response) = ui.allocate_exact_size(vec2(34., 34.), Sense::click());
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                ui.painter().circle_filled(rect.center(), 11., c);
                if self.settings.accent == color {
                    ui.painter()
                        .circle_stroke(rect.center(), 15., Stroke::new(1.8, c));
                }
                if response.on_hover_text(label).clicked() {
                    self.settings.accent = color;
                }
            }
            ui.add_space(6.);
            ui.color_edit_button_srgb(&mut self.settings.accent);
            ui.label(RichText::new("カスタム").size(12.).color(p.muted));
        });
        controls::section(ui, "文字組み", p);
        controls::row(ui, "書体", |ui, label_id| {
            let selected = self.settings.font_family.clone();
            egui::ComboBox::from_id_salt("reader-font-family")
                .width(ui.available_width())
                .height(260.)
                .selected_text(&selected)
                .show_ui(ui, |ui| {
                    let label = ui.label("書体を検索");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.font_query)
                            .desired_width(ui.available_width()),
                    )
                    .labelled_by(label.id);
                    let query = self.font_query.to_lowercase();
                    let families = self.fonts.families().to_vec();
                    for family in families
                        .iter()
                        .filter(|family| query.is_empty() || family.to_lowercase().contains(&query))
                    {
                        ui.selectable_value(&mut self.settings.font_family, family.clone(), family);
                    }
                })
                .response
                .labelled_by(label_id);
        });
        controls::slider_row(
            ui,
            "文字サイズ",
            &mut self.settings.font_size,
            14.0..=64.0,
            0,
            " px",
        );
        controls::slider_row(
            ui,
            "字間",
            &mut self.settings.letter_spacing,
            0.0..=0.3,
            2,
            " em",
        );
        controls::slider_row(
            ui,
            "行間",
            &mut self.settings.line_spacing,
            1.0..=3.0,
            2,
            "",
        );
        controls::section(ui, "ページの余白", p);
        let mut vertical = self.settings.vertical_margin();
        if controls::slider_row(ui, "上下", &mut vertical, 0.0..=160.0, 0, " px").changed() {
            self.settings.margin_vertical = Some(vertical);
        }
        let mut horizontal = self.settings.horizontal_margin();
        if controls::slider_row(ui, "左右", &mut horizontal, 0.0..=160.0, 0, " px").changed() {
            self.settings.margin_horizontal = Some(horizontal);
        }
        let image_pages = reading_surface::is_image_chapter(&self.settings_preview_source().0);
        let mut gap = self.settings.spread_gap(image_pages);
        if controls::slider_row(ui, "ページ間", &mut gap, 0.0..=120.0, 0, " px").changed() {
            self.settings.page_gap = Some(gap);
        }
        controls::section(ui, "ルビ", p);
        controls::check(ui, &mut self.settings.show_ruby, "ルビを表示する");
        ui.add_enabled_ui(self.settings.show_ruby, |ui| {
            controls::slider_row(
                ui,
                "ルビのサイズ",
                &mut self.settings.ruby_scale,
                0.2..=1.0,
                2,
                "",
            );
        });
        ui.add_space(14.);
        if ui
            .add_sized([176., 34.], egui::Button::new("外観を初期値に戻す"))
            .clicked()
        {
            let defaults = reader_library::Settings::default();
            self.settings.set_theme(defaults.theme);
            self.settings.accent = defaults.accent;
            self.settings.font_family = defaults.font_family;
            self.settings.font_size = defaults.font_size;
            self.settings.line_spacing = defaults.line_spacing;
            self.settings.letter_spacing = defaults.letter_spacing;
            self.settings.margins = defaults.margins;
            self.settings.margin_horizontal = defaults.margin_horizontal;
            self.settings.margin_vertical = defaults.margin_vertical;
            self.settings.page_gap = defaults.page_gap;
            self.settings.show_ruby = defaults.show_ruby;
            self.settings.adapt_monochrome_images = defaults.adapt_monochrome_images;
            self.settings.ruby_scale = defaults.ruby_scale;
            self.font_query.clear();
        }
    }

    fn appearance_preview(&mut self, ui: &mut Ui, p: Palette) {
        ui.label(RichText::new("表示全体").size(12.).color(p.muted));
        let extent = self.reading_extent.max(vec2(400., 300.));
        let width = ui.available_width().max(100.);
        let height = (ui.available_height() - 50.).max(100.);
        let scale = (width / extent.x).min(height / extent.y).min(1.);
        let size = extent * scale;
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let palette = Palette::for_settings(&self.settings);
        ui.painter().rect_filled(rect, 2., palette.paper);
        let (chapter, book_id, rtl) = self.settings_preview_source();
        let content = reading_surface::content_rect(
            Rect::from_min_size(egui::Pos2::ZERO, extent),
            &chapter,
            &self.settings,
        );
        let effective = reading_surface::effective_layout_settings(&self.settings, &chapter);
        let plan = self.settings_preview_spread(content.size());
        let count = plan.columns;
        let sibling = if plan.second.is_some() {
            self.settings_preview_sibling()
        } else {
            None
        };
        let sibling_key = sibling
            .as_ref()
            .map(|chapter| format!("{}:{}:{}", chapter.id, chapter.href, chapter.blocks.len()))
            .unwrap_or_default();
        let key = format!(
            "{}:{}:{}:{}:{:?}:{}:{:.2}:{:.2}:{:.2}:{:.1}:{}:{:.2}:{:?}:{:.1}:{:.1}:{}:{}:{}",
            book_id,
            chapter.id,
            chapter.href,
            chapter.blocks.len(),
            self.settings.reading_mode,
            self.settings.font_family,
            self.settings.font_size,
            self.settings.line_spacing,
            self.settings.letter_spacing,
            self.settings.margins,
            self.settings.show_ruby,
            self.settings.ruby_scale,
            self.settings.spread,
            extent.x,
            extent.y,
            rtl,
            count,
            sibling_key
        );
        let key = format!(
            "{key}:{:?}",
            (
                self.settings.horizontal_margin(),
                self.settings.vertical_margin(),
                self.settings.page_gap,
                self.settings.theme,
                self.settings.custom_foreground
            )
        );
        let cache_id = Id::unique("settings-full-native-preview");
        let cached = ui
            .ctx()
            .data_mut(|data| data.get_temp::<NativePreview>(cache_id));
        let page = if let Some(cache) = cached.filter(|cache| cache.key == key) {
            cache.page
        } else {
            let layout = layout::chapter_layout(
                &mut self.fonts,
                &chapter,
                plan.extent(content.size()),
                &effective,
            );
            let window = reading_surface::page_window(&chapter, 0, layout.pages.len(), count, rtl);
            let second = if layout.pages.len() == 1 {
                sibling.and_then(|sibling| {
                    layout::chapter_layout(
                        &mut self.fonts,
                        &sibling,
                        plan.extent(content.size()),
                        &reading_surface::effective_layout_settings(&self.settings, &sibling),
                    )
                    .pages
                    .first()
                    .cloned()
                })
            } else if window.end > 1 {
                layout.pages.get(1).cloned()
            } else {
                None
            };
            let page = reading_surface::compose(
                layout.pages.first().cloned().unwrap_or_default(),
                second,
                content.size(),
                count,
                if layout.pages.len() == 1 {
                    plan.first_slot
                } else {
                    window.first_slot
                },
                plan.gap,
            );
            let mut full_page = layout::Page::default();
            full_page.append_at(page, content.min.to_vec2(), 1.);
            let page = full_page;
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    cache_id,
                    NativePreview {
                        key,
                        page: page.clone(),
                    },
                )
            });
            page
        };
        let transform = |source: Rect| {
            Rect::from_min_max(
                rect.min + source.min.to_vec2() * scale,
                rect.min + source.max.to_vec2() * scale,
            )
        };
        let painter = ui.painter().with_clip_rect(rect);
        for fill in &page.header_fills {
            painter.rect_filled(transform(*fill), 0., palette.panel);
        }
        for picture in &page.pictures {
            let target = transform(picture.rect);
            let mut miniature = picture.clone();
            miniature.rect = Rect::from_min_size(pos2(0., 0.), picture.rect.size() * scale);
            let image_chapter = plan
                .second
                .filter(|_| {
                    usize::from(picture.rect.center().x >= extent.x * 0.5) != plan.first_slot
                })
                .unwrap_or(plan.first);
            if let Some(texture) = self.picture_texture_for(ui.ctx(), &miniature, image_chapter) {
                painter.image(
                    texture.id(),
                    target,
                    Rect::from_min_max(pos2(0., 0.), pos2(1., 1.)),
                    Color32::WHITE,
                );
            } else {
                painter.rect_filled(target, 1., palette.panel);
            }
        }
        for glyph in &page.glyphs {
            self.fonts.draw_styled(
                &painter,
                glyph.face,
                glyph.id,
                rect.min + glyph.origin.to_vec2() * scale,
                glyph.size * scale,
                palette.ink,
                glyph.rotated,
                glyph.style,
            );
        }
        for rule in &page.rules {
            painter.rect_stroke(
                transform(*rule),
                0.,
                Stroke::new(0.7, palette.border),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().rect_stroke(
            rect,
            2.,
            Stroke::new(1., p.border),
            egui::StrokeKind::Inside,
        );
        ui.add_space(6.);
        ui.label(
            RichText::new(if count == 2 {
                "現在の画面比率・見開き"
            } else {
                "現在の画面比率・単ページ"
            })
            .size(11.)
            .color(p.muted),
        );
    }

    fn reading_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::heading(ui, "読書", "", p);
        controls::section(ui, "文字の方向", p);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.settings.reading_mode,
                ReadingMode::Auto,
                "書籍に合わせる",
            );
            ui.selectable_value(
                &mut self.settings.reading_mode,
                ReadingMode::Vertical,
                "縦書き",
            );
            ui.selectable_value(
                &mut self.settings.reading_mode,
                ReadingMode::Horizontal,
                "横書き",
            );
        });
        controls::section(ui, "画像・ページの見開き", p);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(
                &mut self.settings.spread,
                SpreadMode::Auto,
                "書籍に合わせる",
            );
            ui.selectable_value(&mut self.settings.spread, SpreadMode::Single, "単ページ");
            ui.selectable_value(&mut self.settings.spread, SpreadMode::Double, "見開き");
        });
        ui.add_space(8.);
        controls::check(
            ui,
            &mut self.settings.text_spread,
            "指定がない本文を2ページに分けて表示",
        );
        controls::slider_row(
            ui,
            "スクロール量",
            &mut self.settings.scroll_speed,
            0.5..=4.0,
            2,
            " ×",
        );
        ui.add_space(16.);
        ui.separator();
        ui.add_space(12.);
        controls::check(
            ui,
            &mut self.settings.tts.auto_follow,
            "読み上げている位置を自動で表示する",
        );
    }

    fn dictionary_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::heading(ui, "辞書", "", p);
        ui.horizontal(|ui| {
            if ui
                .add_sized([116., 32.], egui::Button::new("辞書を追加"))
                .clicked()
            {
                self.add_dictionaries(false);
            }
            if ui
                .add_sized([140., 32.], egui::Button::new("フォルダーを追加"))
                .clicked()
            {
                self.add_dictionaries(true);
            }
        });
        ui.add_space(8.);
        ui.horizontal(|ui| {
            let label = ui.label("絞り込み");
            ui.add(
                egui::TextEdit::singleline(&mut self.dictionary_query)
                    .desired_width(ui.available_width()),
            )
            .labelled_by(label.id);
        });
        ui.add_space(10.);
        let dictionaries = self.dictionary_infos.clone();
        let query = self.dictionary_query.to_lowercase();
        let mut action: Option<(String, Option<bool>, Option<usize>, bool)> = None;
        for (index, dict) in dictionaries.iter().enumerate() {
            if !query.is_empty() && !dict.title.to_lowercase().contains(&query) {
                continue;
            }
            let (rect, row_response) =
                ui.allocate_exact_size(vec2(ui.available_width(), 46.), Sense::hover());
            if self.dictionary_drag.as_deref() == Some(dict.id.as_str()) {
                ui.painter().rect_filled(rect, 5., p.selection);
            }
            region(
                ui,
                &format!("dictionary-settings-{}", dict.id),
                rect.shrink2(vec2(2., 4.)),
                |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.;
                        let mut enabled = dict.enabled;
                        if ui
                            .checkbox(&mut enabled, "")
                            .on_hover_text("この辞書を使う")
                            .changed()
                        {
                            action = Some((dict.id.clone(), Some(enabled), None, false));
                        }
                        let (grip, drag) = ui.allocate_exact_size(vec2(24., 30.), Sense::drag());
                        drag.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                format!("{}の表示順を変更", dict.title),
                            )
                        });
                        icons::paint(ui.painter(), grip.center(), Icon::Grip, p.muted);
                        if drag.drag_started() {
                            self.dictionary_drag = Some(dict.id.clone());
                        }
                        drag.dnd_set_drag_payload(DictionaryDrag {
                            id: dict.id.clone(),
                        });
                        drag.on_hover_text("ドラッグして表示順を変更");
                        let title_width = (ui.available_width() - 44.).max(100.);
                        let title = icons::label(
                            ui,
                            vec2(title_width, 32.),
                            RichText::new(&dict.title).size(14.),
                            Sense::hover(),
                        );
                        title.on_hover_text(match &dict.status {
                            DictionaryStatus::Unavailable(error) => {
                                format!("{}\n{}", dict.path.display(), error)
                            }
                            _ => dict.path.display().to_string(),
                        });
                        if icons::button(ui, Icon::Trash, "辞書の登録を解除", false, p.accent)
                            .clicked()
                        {
                            action = Some((dict.id.clone(), None, None, true));
                        }
                    });
                },
            );
            if let Some(payload) = row_response.dnd_hover_payload::<DictionaryDrag>()
                && payload.id != dict.id
            {
                let below = ui
                    .input(|input| input.pointer.hover_pos())
                    .is_some_and(|pos| pos.y >= rect.center().y);
                let y = if below { rect.bottom() } else { rect.top() };
                ui.painter().line_segment(
                    [pos2(rect.left() + 32., y), pos2(rect.right(), y)],
                    Stroke::new(2., p.accent),
                );
            }
            if let Some(payload) = row_response.dnd_release_payload::<DictionaryDrag>() {
                let below = ui
                    .input(|input| input.pointer.hover_pos())
                    .is_some_and(|pos| pos.y >= rect.center().y);
                let mut target = index + usize::from(below);
                if let Some(source) = dictionaries.iter().position(|dict| dict.id == payload.id) {
                    if source < target {
                        target = target.saturating_sub(1);
                    }
                    if source != target {
                        action = Some((payload.id.clone(), None, Some(target), false));
                    }
                }
                self.dictionary_drag = None;
            }
            ui.painter().line_segment(
                [rect.left_bottom(), rect.right_bottom()],
                Stroke::new(0.7, p.border),
            );
        }
        if ui.input(|input| input.pointer.any_released())
            || egui::DragAndDrop::payload::<DictionaryDrag>(ui.ctx()).is_none()
        {
            self.dictionary_drag = None;
        }
        if let Some((id, enabled, position, remove)) = action {
            let result = (|| -> anyhow::Result<()> {
                let mut catalog =
                    DictionaryCatalog::open(self.store.data_dir().join("dictionaries"))?;
                if remove {
                    catalog.remove(&id)?;
                } else if let Some(enabled) = enabled {
                    catalog.set_enabled(&id, enabled)?;
                } else if let Some(position) = position {
                    catalog.move_to(&id, position)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.notify(error.to_string());
            }
            self.refresh_dictionaries();
        }
        if dictionaries.is_empty() {
            ui.add_space(20.);
            ui.label(
                RichText::new("MDXファイル、または辞書が入ったフォルダーを追加してください。")
                    .color(p.muted),
            );
        }
    }

    fn library_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::heading(ui, "本棚・Calibre", "", p);
        controls::section(ui, "Calibreライブラリ", p);
        let path = self
            .settings
            .calibre_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "まだ選択されていません".into());
        ui.add(egui::Label::new(RichText::new(path).size(13.)).wrap());
        ui.add_space(8.);
        ui.horizontal(|ui| {
            if ui
                .add_sized([140., 34.], egui::Button::new("フォルダーを選ぶ"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new().pick_folder()
            {
                self.settings.calibre_path = Some(path);
                self.refresh_shelf();
            }
            if ui
                .add_sized([116., 34.], egui::Button::new("本棚を更新"))
                .clicked()
            {
                self.refresh_shelf();
            }
        });
        ui.add_space(20.);
        ui.separator();
        controls::section(ui, "オンライン", p);
        ui.horizontal_wrapped(|ui| {
            if ui.button("オンライン本棚 (OPDS)").clicked() {
                self.screen = self.previous_screen;
                self.open_catalog_dialog();
            }
        });
        controls::section(ui, "保存先", p);
        ui.add(
            egui::Label::new(
                RichText::new(self.store.data_dir().display().to_string())
                    .size(12.)
                    .color(p.muted),
            )
            .wrap(),
        );
    }

    pub(crate) fn key_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::heading(ui, "キー割り当て", "", p);
        let (search, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.), Sense::hover());
        let reset = Rect::from_min_max(pos2(search.right() - 108., search.top()), search.max);
        let search_label = icons::label_at(
            ui,
            Rect::from_min_max(search.min, pos2(search.left() + 68., search.bottom())),
            "操作を検索",
            Sense::hover(),
        );
        ui.place(
            Rect::from_min_max(
                pos2(search.left() + 78., search.top()),
                pos2(reset.left() - 10., search.bottom()),
            ),
            egui::TextEdit::singleline(&mut self.binding_query)
                .desired_width(f32::INFINITY)
                .hint_text("操作名・キー"),
        )
        .labelled_by(search_label.id);
        if ui
            .place(reset, egui::Button::new("すべて初期化").truncate())
            .clicked()
        {
            self.settings.key_bindings.clear();
            self.binding_capture = None;
        }
        // Reserve this line even while idle so entering capture never moves
        // the clicked row. Escape itself can be assigned like any other key.
        let (status, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.), Sense::hover());
        if self.binding_capture.is_some() {
            icons::label_at(
                ui,
                Rect::from_min_max(status.min, pos2(status.right() - 64., status.bottom())),
                RichText::new("割り当てるキーを押してください").color(p.accent),
                Sense::hover(),
            );
            if ui
                .place(
                    Rect::from_min_max(pos2(status.right() - 56., status.top()), status.max),
                    egui::Button::new("取消").frame(false).truncate(),
                )
                .clicked()
            {
                self.binding_capture = None;
            }
        }
        ui.add_space(10.);
        let mut rows = Action::ALL
            .iter()
            .map(|&action| {
                (
                    action.id().to_owned(),
                    action.label().to_owned(),
                    action.group().to_owned(),
                    actions::binding(action, &self.settings.key_bindings).to_owned(),
                )
            })
            .collect::<Vec<_>>();
        for name in self.settings.custom_shelves.keys() {
            let id = format!("Shelf:{name}");
            let binding = self
                .settings
                .key_bindings
                .get(&id)
                .cloned()
                .unwrap_or_default();
            rows.push((id, format!("本棚: {name}"), "追加した本棚".into(), binding));
        }
        let query = self.binding_query.to_lowercase();
        let mut group = String::new();
        for (id, label, row_group, binding) in &rows {
            if !query.is_empty()
                && !format!("{id} {label} {row_group} {binding}")
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            if group != *row_group {
                controls::section(ui, row_group, p);
                group = row_group.clone();
            }
            ui.push_id(id, |ui| {
                let (row, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 38.), Sense::hover());
                let row = Rect::from_min_max(row.min + vec2(16., 0.), row.max);
                let reset = Rect::from_min_max(
                    pos2(row.right() - 52., row.top() + 3.),
                    pos2(row.right(), row.bottom() - 3.),
                );
                let clear = Rect::from_min_max(
                    pos2(reset.left() - 38., reset.top()),
                    pos2(reset.left() - 8., reset.bottom()),
                );
                let binding_width = 146.0_f32.min((clear.left() - 8. - row.left()).max(0.));
                let binding_rect = Rect::from_min_max(
                    pos2(clear.left() - 8. - binding_width, clear.top()),
                    pos2(clear.left() - 8., clear.bottom()),
                );
                let label_rect = Rect::from_min_max(
                    row.min,
                    pos2((binding_rect.left() - 12.).max(row.left()), row.bottom()),
                );
                let label_response =
                    icons::label_at(ui, label_rect, label.as_str(), Sense::hover());
                label_response.on_hover_text(label);
                let capturing = self.binding_capture.as_ref() == Some(id);
                let button_text = if capturing {
                    "キーを押してください…".to_owned()
                } else {
                    display_binding(binding)
                };
                let response = ui.place(
                    binding_rect,
                    egui::Button::new(RichText::new(button_text).color(if capturing {
                        p.accent
                    } else {
                        p.ink
                    }))
                    .selected(capturing)
                    .truncate(),
                );
                if response.clicked() {
                    self.binding_capture = Some(id.clone());
                }
                let duplicates = rows
                    .iter()
                    .filter(|(other_id, _, _, other)| {
                        other_id != id && !binding.is_empty() && other == binding
                    })
                    .map(|(_, label, _, _)| label.as_str())
                    .collect::<Vec<_>>();
                response.on_hover_text(if duplicates.is_empty() {
                    format!(
                        "{label}: {}",
                        if binding.is_empty() {
                            "未割り当て"
                        } else {
                            binding
                        }
                    )
                } else {
                    format!("同じキー: {}", duplicates.join("、"))
                });
                if ui
                    .place(
                        clear,
                        egui::Button::new("×").small().frame(false).truncate(),
                    )
                    .on_hover_text("割り当てを解除")
                    .clicked()
                {
                    self.settings.key_bindings.insert(id.clone(), String::new());
                    self.binding_capture = None;
                }
                if ui
                    .place(
                        reset,
                        egui::Button::new("初期化").small().frame(false).truncate(),
                    )
                    .on_hover_text("この操作を初期値に戻す")
                    .clicked()
                {
                    self.settings.key_bindings.remove(id);
                    self.binding_capture = None;
                }
            });
        }
    }
}

fn color_override(ui: &mut Ui, label: &str, value: &mut Option<[u8; 3]>, fallback: Color32) {
    ui.push_id(label, |ui| {
        let [r, g, b, _] = fallback.to_array();
        let mut color = value.unwrap_or([r, g, b]);
        let id = ui.scope_id().with("hex-draft");
        let mut draft = ui
            .ctx()
            .data_mut(|data| data.get_temp::<(String, [u8; 3])>(id))
            .filter(|(_, saved)| *saved == color)
            .unwrap_or_else(|| (color_hex(color), color));
        controls::row(ui, label, |ui, label_id| {
            if ui.color_edit_button_srgb(&mut color).changed() {
                *value = Some(color);
                draft = (color_hex(color), color);
            }
            let response = ui
                .add(
                    egui::TextEdit::singleline(&mut draft.0)
                        .desired_width(100.)
                        .font(egui::TextStyle::Monospace),
                )
                .labelled_by(label_id);
            if response.changed()
                && let Some(parsed) = parse_color_hex(&draft.0)
            {
                color = parsed;
                *value = Some(parsed);
                draft.1 = parsed;
            }
            if value.is_some() && ui.button("テーマの色へ").clicked() {
                *value = None;
                draft = (color_hex([r, g, b]), [r, g, b]);
            }
        });
        ui.ctx().data_mut(|data| data.insert_temp(id, draft));
    });
}
fn color_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}
fn parse_color_hex(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..], 16).ok()?,
    ])
}

fn display_binding(binding: &str) -> String {
    if binding.is_empty() {
        return "未割り当て".into();
    }
    binding
        .replace(
            "Primary",
            if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Ctrl"
            },
        )
        .replace("ArrowLeft", "←")
        .replace("ArrowRight", "→")
        .replace("ArrowUp", "↑")
        .replace("ArrowDown", "↓")
        .replace("Escape", "Esc")
        .replace("Comma", ",")
}
