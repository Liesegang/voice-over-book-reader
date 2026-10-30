//! Native inline rule management and a small, non-modal editor for selected text.
use crate::{app::ReaderApp, settings_controls as controls};
use eframe::egui::{self, Context, Id, RichText};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct Editor {
    open: bool,
    word: String,
    reading: String,
    editing: Option<String>,
    error: Option<String>,
}
fn editor_id() -> Id {
    Id::unique("pronunciation-editor-state")
}
fn save_rule(
    rules: &mut BTreeMap<String, String>,
    previous: Option<&str>,
    word: &str,
    reading: &str,
) -> Result<(), String> {
    let word = word.trim();
    let reading = reading.trim();
    if word.is_empty() {
        return Err("表記を入力してください".into());
    }
    if reading.is_empty() {
        return Err("読みを入力してください".into());
    }
    if rules.contains_key(word) && previous != Some(word) {
        return Err("その表記は登録済みです。一覧の「編集」から変更できます。".into());
    }
    if let Some(previous) = previous
        && previous != word
    {
        rules.remove(previous);
    }
    rules.insert(word.into(), reading.into());
    Ok(())
}
fn editor_fields(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    rules: &mut BTreeMap<String, String>,
    quick: bool,
) -> bool {
    ui.columns(2, |columns| {
        let width = columns[0].available_width();
        columns[0].label("表記");
        let word = columns[0].add_sized(
            [width, 34.],
            egui::TextEdit::singleline(&mut editor.word).hint_text("例：日本橋"),
        );
        let width = columns[1].available_width();
        columns[1].label("読み");
        let reading = columns[1].add_sized(
            [width, 34.],
            egui::TextEdit::singleline(&mut editor.reading).hint_text("例：にっぽんばし"),
        );
        if word.changed() || reading.changed() {
            editor.error = None;
        }
    });
    ui.add_space(10.);
    let mut changed = false;
    ui.horizontal(|ui| {
        let label = if editor.editing.is_some() {
            "変更を保存"
        } else {
            "登録する"
        };
        if ui
            .add_sized(
                [110., 34.],
                egui::Button::new(label).fill(ui.visuals().selection.bg_fill),
            )
            .clicked()
        {
            match save_rule(
                rules,
                editor.editing.as_deref(),
                &editor.word,
                &editor.reading,
            ) {
                Ok(()) => {
                    changed = true;
                    *editor = Editor::default();
                }
                Err(error) => editor.error = Some(error),
            }
        }
        if (quick || editor.editing.is_some())
            && ui
                .add_sized(
                    [110., 34.],
                    egui::Button::new(if quick {
                        "キャンセル"
                    } else {
                        "編集をやめる"
                    })
                    .frame(false),
                )
                .clicked()
        {
            *editor = Editor::default();
        }
    });
    if let Some(error) = &editor.error {
        ui.colored_label(egui::Color32::from_rgb(190, 70, 60), error);
    }
    changed
}
impl ReaderApp {
    pub(crate) fn open_pronunciation_editor(&mut self, ctx: &Context, word: &str) {
        let word = word.trim().to_owned();
        let reading = self.settings.tts.pronunciations.get(&word).cloned();
        ctx.data_mut(|data| {
            data.insert_temp(
                editor_id(),
                Editor {
                    open: true,
                    editing: reading.as_ref().map(|_| word.clone()),
                    word,
                    reading: reading.unwrap_or_default(),
                    error: None,
                },
            )
        });
    }
    pub(crate) fn pronunciation_editor(&mut self, ctx: &Context) {
        let mut editor = ctx
            .data_mut(|data| data.get_temp::<Editor>(editor_id()))
            .unwrap_or_default();
        if !editor.open {
            return;
        }
        let mut open = !ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let mut changed = false;
        egui::Window::new("読みを登録")
            .id(Id::unique("pronunciation-editor-window"))
            .default_width(480.)
            .min_width(360.)
            .resizable(false)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("すべての音声エンジンで使う読みを登録します。");
                ui.add_space(14.);
                changed =
                    editor_fields(ui, &mut editor, &mut self.settings.tts.pronunciations, true);
            });
        editor.open &= open;
        ctx.data_mut(|data| data.insert_temp(editor_id(), editor));
        if changed {
            self.save_settings();
        }
    }
    pub(crate) fn pronunciation_settings_section(
        &mut self,
        ui: &mut egui::Ui,
        p: crate::theme::Palette,
    ) {
        controls::section(ui, "読みの登録", p);
        ui.label(
            RichText::new("すべての音声エンジンで使います。長い表記を優先します。")
                .color(p.muted)
                .size(12.),
        );
        ui.add_space(12.);
        let id = Id::unique("pronunciation-inline-state");
        let mut editor = ui
            .ctx()
            .data_mut(|data| data.get_temp::<Editor>(id))
            .unwrap_or_default();
        let mut changed = editor_fields(
            ui,
            &mut editor,
            &mut self.settings.tts.pronunciations,
            false,
        );
        ui.add_space(16.);
        ui.separator();
        ui.add_space(10.);
        ui.label(
            RichText::new(format!(
                "登録済み · {} 件",
                self.settings.tts.pronunciations.len()
            ))
            .strong(),
        );
        ui.add_space(6.);
        let rows: Vec<_> = self
            .settings
            .tts
            .pronunciations
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("pronunciation-list")
            .max_height(300.)
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label("登録した表記と読みがここに表示されます。");
                }
                for (word, reading) in rows {
                    ui.push_id(&word, |ui| {
                        ui.horizontal(|ui| {
                            let column = ((ui.available_width() - 112.) * 0.5).max(60.);
                            ui.add_sized(
                                [column, 34.],
                                egui::Label::new(&word).truncate().halign(egui::Align::Min),
                            );
                            ui.add_sized(
                                [column, 34.],
                                egui::Label::new(&reading)
                                    .truncate()
                                    .halign(egui::Align::Min),
                            );
                            if ui
                                .add_sized([52., 32.], egui::Button::new("編集").frame(false))
                                .clicked()
                            {
                                editor.word = word.clone();
                                editor.reading = reading.clone();
                                editor.editing = Some(word.clone());
                                editor.error = None;
                            }
                            if ui
                                .add_sized([32., 32.], egui::Button::new("×").frame(false))
                                .on_hover_text("この読みを削除")
                                .clicked()
                            {
                                self.settings.tts.pronunciations.remove(&word);
                                changed = true;
                                if editor.editing.as_ref() == Some(&word) {
                                    editor = Editor::default();
                                }
                            }
                        });
                    });
                }
            });
        ui.ctx().data_mut(|data| data.insert_temp(id, editor));
        if changed {
            self.save_settings();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_empty_duplicate_and_rename_collision_without_losing_previous_rule() {
        let mut rules = [
            ("日本橋".into(), "にっぽんばし".into()),
            ("日本".into(), "にほん".into()),
        ]
        .into();
        assert!(save_rule(&mut rules, None, " ", "あ").is_err());
        assert!(save_rule(&mut rules, None, "大阪", " ").is_err());
        assert!(save_rule(&mut rules, None, "日本橋", "にほんばし").is_err());
        assert!(save_rule(&mut rules, Some("日本橋"), "日本", "にっぽん").is_err());
        assert_eq!(rules["日本橋"], "にっぽんばし");
        save_rule(&mut rules, Some("日本橋"), "日本橋", "にほんばし").unwrap();
        assert_eq!(rules["日本橋"], "にほんばし");
        save_rule(&mut rules, Some("日本橋"), " 橋 ", " はし ").unwrap();
        assert!(!rules.contains_key("日本橋"));
        assert_eq!(rules["橋"], "はし");
    }
}
