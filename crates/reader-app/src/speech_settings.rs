use crate::{app::ReaderApp, settings_controls as controls, theme::Palette};
use eframe::egui::{self, RichText, Ui};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct AivisCatalog {
    requested: bool,
    loading: bool,
    voices: Vec<reader_speech::AivisVoice>,
    status: String,
    error: Option<String>,
}

#[derive(Default)]
struct NativeAivisCatalog {
    requested: bool,
    loading: bool,
    catalog: reader_speech::AivisModelCatalog,
    imported: Option<reader_speech::AivisModelEntry>,
    status: String,
    error: Option<String>,
}
fn fetch_native_aivis(
    state: Arc<Mutex<NativeAivisCatalog>>,
    config: reader_speech::ProviderConfig,
    source: Option<std::path::PathBuf>,
    context: egui::Context,
) {
    {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        if state.loading {
            return;
        }
        state.loading = true;
        state.requested = true;
        state.error = None;
        state.status = if source.is_some() {
            "モデルを検証・追加しています"
        } else {
            "モデル一覧を取得しています"
        }
        .into();
    }
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<_> {
            let imported = source
                .as_ref()
                .map(|source| {
                    reader_speech::import_aivis_native_model(
                        source,
                        &config,
                        &reader_speech::Cancellation::uncancelled(),
                        &|message| {
                            state
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .status = message;
                            context.request_repaint();
                        },
                    )
                })
                .transpose()?;
            let catalog = reader_speech::aivis_native_catalog(&config)?;
            Ok((catalog, imported))
        })();
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.loading = false;
        state.status.clear();
        match result {
            Ok((catalog, imported)) => {
                state.catalog = catalog;
                state.imported = imported;
            }
            Err(error) => state.error = Some(format!("{error:#}")),
        }
        context.request_repaint();
    });
}

fn fetch_aivis_catalog(
    state: Arc<Mutex<AivisCatalog>>,
    config: reader_speech::ProviderConfig,
    context: egui::Context,
) {
    {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        if state.loading {
            return;
        }
        state.loading = true;
        state.requested = true;
        state.error = None;
        state.status = "話者を取得しています".into();
    }
    std::thread::spawn(move || {
        let result = reader_speech::aivis_voices(
            &config,
            &reader_speech::Cancellation::uncancelled(),
            &|message| {
                state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .status = message;
                context.request_repaint();
            },
        );
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.loading = false;
        state.status.clear();
        match result {
            Ok(voices) => state.voices = voices,
            Err(error) => state.error = Some(format!("{error:#}")),
        }
        context.request_repaint();
    });
}

impl ReaderApp {
    pub(crate) fn speech_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::heading(ui, "音声", "", p);
        let previous = self.speech_provider.clone();
        controls::row(ui, "エンジン", |ui, label_id| {
            egui::ComboBox::from_id_salt("speech-provider")
                .width(ui.available_width())
                .selected_text(provider_label(&self.speech_provider))
                .show_ui(ui, |ui| {
                    for (id, label) in [
                        ("irodori", "Irodori（組み込み）"),
                        ("kokoro", "Kokoro（組み込み）"),
                        ("aivis_native", "AivisSpeech（組み込み）"),
                        ("aivis", "AivisSpeech"),
                        ("voiceroid2", "VOICEROID2"),
                        ("system", "システム音声"),
                        ("openai", "OpenAI互換"),
                        ("voicevox", "VOICEVOX"),
                        ("command", "外部コマンド"),
                    ] {
                        ui.selectable_value(&mut self.speech_provider, id.into(), label);
                    }
                })
                .response
                .labelled_by(label_id);
        });
        if previous != self.speech_provider {
            self.finish_provider_change(&previous);
            self.speech_error = None;
            self.speech_progress.clear();
        }
        ui.add_space(4.);
        ui.add(
            egui::Label::new(
                RichText::new(self.provider_config().capability().detail)
                    .size(12.)
                    .color(p.muted),
            )
            .wrap(),
        );
        match self.speech_provider.as_str() {
            "irodori" => {
                controls::section(ui, "声を選ぶ", p);
                controls::row(ui, "声の雰囲気", |ui, label_id| {
                    if ui
                        .add_sized(
                            [ui.available_width(), 34.],
                            egui::TextEdit::singleline(&mut self.speech_instruction)
                                .hint_text("例：落ち着いた、柔らかい声"),
                        )
                        .labelled_by(label_id)
                        .changed()
                    {
                        self.speech_reference = None;
                    }
                });
                ui.add_space(6.);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_sized([92., 34.], egui::Button::new("試聴する"))
                        .clicked()
                    {
                        self.preview_voice();
                    }
                    if ui
                        .add_sized([146., 34.], egui::Button::new("別の声を試す"))
                        .clicked()
                    {
                        self.speech_reference = None;
                        self.speech_seed = self.speech_seed.wrapping_add(1);
                        self.sync_speech_settings();
                        self.preview_voice();
                    }
                    if ui
                        .add_sized(
                            [146., 34.],
                            egui::Button::new(RichText::new("この声で読む").color(p.accent)),
                        )
                        .clicked()
                    {
                        self.accept_voice_sample();
                    }
                });
                ui.label(
                    RichText::new(if self.speech_reference.is_some() {
                        "選んだサンプルの声を使います。"
                    } else {
                        "気に入ったサンプルで「この声で読む」を選ぶと、その声を保って読みます。"
                    })
                    .size(12.)
                    .color(p.muted),
                );
                egui::CollapsingHeader::new("声の参照・バリエーション").show(ui, |ui| {
                    controls::row(ui, "参照音声", |ui, label_id| {
                        let name = self
                            .speech_reference
                            .as_ref()
                            .and_then(|path| path.file_name())
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "未選択".into());
                        let (row, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 32.),
                            egui::Sense::hover(),
                        );
                        let clear = egui::Rect::from_min_size(
                            egui::pos2(row.right() - 32., row.top()),
                            egui::vec2(32., 32.),
                        );
                        let pick = egui::Rect::from_min_size(
                            egui::pos2(clear.left() - 8. - 92., row.top()),
                            egui::vec2(92., 32.),
                        );
                        let filename = egui::Rect::from_min_max(
                            row.min,
                            egui::pos2((pick.left() - 12.).max(row.left()), row.bottom()),
                        );
                        crate::icons::label_at(ui, filename, name, egui::Sense::hover())
                            .labelled_by(label_id);
                        if ui.place(pick, egui::Button::new("WAVを選ぶ")).clicked()
                            && let Some(path) = rfd::FileDialog::new()
                                .add_filter("参照する音声", &["wav"])
                                .pick_file()
                        {
                            self.speech_reference = Some(path);
                            self.sync_speech_settings();
                        }
                        if ui
                            .place(clear, egui::Button::new("×").frame(false))
                            .on_hover_text("参照音声を外す")
                            .clicked()
                        {
                            self.speech_reference = None;
                        }
                    });
                    controls::row(ui, "バリエーション", |ui, label_id| {
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.speech_seed)
                                    .speed(1)
                                    .range(0..=u32::MAX as u64),
                            )
                            .labelled_by(label_id)
                            .changed()
                        {
                            self.speech_reference = None;
                        }
                    });
                });
            }
            "kokoro" => {
                controls::section(ui, "固定の声", p);
                controls::row(ui, "声", |ui, label_id| {
                    let label = reader_speech::kokoro_voices()
                        .iter()
                        .find(|(id, _)| *id == self.speech_voice)
                        .map(|(_, label)| *label)
                        .unwrap_or("Alpha");
                    egui::ComboBox::from_id_salt("kokoro-fixed-voice")
                        .width(ui.available_width())
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for (id, label) in reader_speech::kokoro_voices() {
                                ui.selectable_value(&mut self.speech_voice, (*id).into(), *label);
                            }
                        })
                        .response
                        .labelled_by(label_id);
                });
                controls::section(ui, "NVIDIA GPU", p);
                ui.horizontal_wrapped(|ui| {
                    ui.hyperlink_to(
                        "CUDA / cuBLAS の利用条件",
                        "https://docs.nvidia.com/cuda/archive/12.8.1/eula/index.html",
                    );
                    ui.hyperlink_to(
                        "cuDNN の利用条件",
                        "https://docs.nvidia.com/deeplearning/cudnn/backend/v9.8.0/reference/eula.html",
                    );
                });
                egui::CollapsingHeader::new("取得パッケージの利用条件（原文）").show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("kokoro-nvidia-original-terms")
                        .max_height(220.)
                        .show(ui, |ui| {
                            for (name, terms) in reader_speech::nvidia_runtime_terms() {
                                ui.strong(name);
                                ui.add(egui::Label::new(RichText::new(terms).size(11.)).wrap());
                            }
                        });
                });
                let mut config = self.provider_config();
                if let reader_speech::ProviderConfig::Kokoro {
                    allow_nvidia_download,
                    ..
                } = &mut config
                    && controls::check(
                        ui,
                        allow_nvidia_download,
                        "利用条件に同意してGPUランタイムの取得を許可する",
                    )
                    .changed()
                {
                    self.set_kokoro_profile(config);
                }
                ui.add(
                    egui::Label::new(
                        RichText::new("未同意の場合はCPUで読みます。利用できるGPUライブラリが既にある場合は、そのライブラリを使います。")
                            .size(12.)
                            .color(p.muted),
                    )
                    .wrap(),
                );
                ui.add_space(6.);
                if ui
                    .add_sized([120., 34.], egui::Button::new("選んだ声を試聴"))
                    .clicked()
                {
                    self.preview_voice();
                }
            }
            "aivis_native" => {
                self.native_aivis_settings(ui, p);
            }
            "aivis" => {
                controls::section(ui, "AivisSpeech Engine", p);
                let mut editing_endpoint = false;
                controls::row(ui, "接続先URL", |ui, label_id| {
                    let response = ui
                        .add_sized(
                            [ui.available_width(), 34.],
                            egui::TextEdit::singleline(&mut self.speech_endpoint),
                        )
                        .labelled_by(label_id);
                    editing_endpoint = response.has_focus() || response.changed();
                });
                let config = self.provider_config();
                let mut connection = serde_json::to_value(&config).unwrap_or_default();
                if let Some(object) = connection.as_object_mut() {
                    object.remove("speaker");
                }
                let key = egui::Id::unique(("aivis-voices", connection.to_string()));
                let existing = ui
                    .ctx()
                    .data(|data| data.get_temp::<Arc<Mutex<AivisCatalog>>>(key));
                let state = existing.clone().unwrap_or_default();
                if existing.is_none() {
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(key, state.clone()));
                }
                let requested = state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .requested;
                if !requested && !editing_endpoint {
                    fetch_aivis_catalog(state.clone(), config.clone(), ui.ctx().clone());
                }
                let (loading, voices, status, error) = {
                    let state = state.lock().unwrap_or_else(|error| error.into_inner());
                    (
                        state.loading,
                        state.voices.clone(),
                        state.status.clone(),
                        state.error.clone(),
                    )
                };
                let selected = self.speech_voice.parse::<i32>().ok();
                let voice = voices.iter().find(|voice| Some(voice.id) == selected);
                controls::row(ui, "声・スタイル", |ui, label_id| {
                    egui::ComboBox::from_id_salt("aivis-voice")
                        .width(ui.available_width())
                        .selected_text(
                            voice
                                .map(reader_speech::AivisVoice::label)
                                .unwrap_or_else(|| "声を選んでください".into()),
                        )
                        .show_ui(ui, |ui| {
                            for voice in &voices {
                                ui.selectable_value(
                                    &mut self.speech_voice,
                                    voice.id.to_string(),
                                    voice.label(),
                                );
                            }
                        })
                        .response
                        .labelled_by(label_id);
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(!loading, egui::Button::new("話者を更新"))
                        .clicked()
                    {
                        fetch_aivis_catalog(state.clone(), config, ui.ctx().clone());
                    }
                    if ui
                        .add_enabled(voice.is_some(), egui::Button::new("選んだ声を試聴"))
                        .clicked()
                    {
                        self.preview_voice();
                    }
                    if loading {
                        ui.spinner();
                    }
                });
                if !status.is_empty() {
                    ui.label(RichText::new(status).size(12.).color(p.muted));
                }
                if let Some(error) = error {
                    ui.add(
                        egui::Label::new(
                            RichText::new(error)
                                .size(12.)
                                .color(egui::Color32::from_rgb(190, 70, 60)),
                        )
                        .wrap(),
                    );
                }
                if let reader_speech::ProviderConfig::AivisSpeech {
                    mut engine_path,
                    mut auto_start,
                    ..
                } = self.provider_config()
                {
                    let mut changed = false;
                    egui::CollapsingHeader::new("エンジンの起動設定").show(ui, |ui| {
                        changed |= ui
                            .checkbox(&mut auto_start, "必要なときにローカルエンジンを起動する")
                            .changed();
                        controls::row(ui, "実行ファイル", |ui, label_id| {
                            ui.add(
                                egui::Label::new(
                                    engine_path
                                        .as_ref()
                                        .map(|path| path.display().to_string())
                                        .unwrap_or_else(|| {
                                            "自動検出（導入済みのAivisSpeech Engine）".into()
                                        }),
                                )
                                .wrap(),
                            )
                            .labelled_by(label_id);
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("ファイルを選ぶ").clicked()
                                    && let Some(path) = rfd::FileDialog::new()
                                        .set_title("AivisSpeech Engineのrun.exeを選択")
                                        .pick_file()
                                {
                                    engine_path = Some(path);
                                    changed = true;
                                }
                                if engine_path.is_some() && ui.button("自動検出に戻す").clicked()
                                {
                                    engine_path = None;
                                    changed = true;
                                }
                            });
                        });
                    });
                    if changed {
                        self.set_aivis_runtime(engine_path, auto_start);
                    }
                }
            }
            "voiceroid2" => {
                controls::section(ui, "インストール済みのVOICEROID2", p);
                let mut config = self.provider_config();
                if let reader_speech::ProviderConfig::Voiceroid2 {
                    install_dir,
                    voice,
                    volume,
                    pitch,
                    range,
                } = &mut config
                {
                    controls::row(ui, "インストール先", |ui, label_id| {
                        let mut path = install_dir.display().to_string();
                        if ui
                            .add_sized(
                                [ui.available_width(), 34.],
                                egui::TextEdit::singleline(&mut path),
                            )
                            .labelled_by(label_id)
                            .changed()
                        {
                            *install_dir = path.into();
                        }
                        if ui.button("フォルダーを選ぶ").clicked()
                            && let Some(path) = rfd::FileDialog::new()
                                .set_title("VOICEROID2のインストール先")
                                .pick_folder()
                        {
                            *install_dir = path;
                        }
                    });
                    let voices = reader_speech::voiceroid2_voices(install_dir);
                    controls::row(ui, "声", |ui, label_id| {
                        let label = voices
                            .iter()
                            .find(|v| v.id == self.speech_voice)
                            .map(|v| v.label.as_str())
                            .unwrap_or(&self.speech_voice);
                        egui::ComboBox::from_id_salt("voiceroid2-voice")
                            .width(ui.available_width())
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                for candidate in &voices {
                                    ui.selectable_value(
                                        &mut self.speech_voice,
                                        candidate.id.clone(),
                                        &candidate.label,
                                    );
                                }
                            })
                            .response
                            .labelled_by(label_id);
                    });
                    if voices.is_empty() {
                        ui.label(
                            RichText::new("このフォルダーに利用できる音声が見つかりません。")
                                .size(12.)
                                .color(p.muted),
                        );
                    }
                    *voice = self.speech_voice.clone();
                    controls::slider_row(ui, "声の高さ", pitch, 0.5..=2.0, 2, " ×");
                    controls::slider_row(ui, "抑揚", range, 0.0..=2.0, 2, "");
                    controls::slider_row(ui, "声の音量", volume, 0.0..=2.0, 2, " ×");
                    self.set_voiceroid2_profile(config);
                }
                if ui
                    .add_sized([120., 34.], egui::Button::new("選んだ声を試聴"))
                    .clicked()
                {
                    self.preview_voice();
                }
            }
            "system" => {
                controls::section(ui, "声", p);
                controls::row(ui, "システムの声", |ui, label_id| {
                    ui.add_sized(
                        [ui.available_width(), 34.],
                        egui::TextEdit::singleline(&mut self.speech_voice)
                            .hint_text("空欄でシステムの標準音声"),
                    )
                    .labelled_by(label_id);
                });
            }
            "openai" | "voicevox" => {
                controls::section(ui, "接続", p);
                controls::row(ui, "接続先URL", |ui, label_id| {
                    ui.add_sized(
                        [ui.available_width(), 34.],
                        egui::TextEdit::singleline(&mut self.speech_endpoint),
                    )
                    .labelled_by(label_id);
                });
                let label = if self.speech_provider == "voicevox" {
                    "話者ID"
                } else {
                    "声の名前"
                };
                controls::row(ui, label, |ui, label_id| {
                    ui.add_sized(
                        [ui.available_width(), 34.],
                        egui::TextEdit::singleline(&mut self.speech_voice),
                    )
                    .labelled_by(label_id);
                });
                if self.speech_provider == "openai" {
                    controls::row(ui, "モデル", |ui, label_id| {
                        ui.add_sized(
                            [ui.available_width(), 34.],
                            egui::TextEdit::singleline(&mut self.speech_model),
                        )
                        .labelled_by(label_id);
                    });
                    controls::row(ui, "APIキー", |ui, label_id| {
                        ui.add_sized(
                            [ui.available_width(), 34.],
                            egui::TextEdit::singleline(&mut self.speech_api_key).password(true),
                        )
                        .labelled_by(label_id);
                    });
                }
            }
            "command" => {
                controls::section(ui, "外部コマンド", p);
                controls::row(ui, "実行ファイル", |ui, label_id| {
                    ui.add_sized(
                        [ui.available_width(), 34.],
                        egui::TextEdit::singleline(&mut self.speech_command),
                    )
                    .labelled_by(label_id);
                });
                controls::row(ui, "引数", |ui, label_id| {
                    ui.add_sized(
                        [ui.available_width(), 34.],
                        egui::TextEdit::singleline(&mut self.speech_command_args),
                    )
                    .labelled_by(label_id);
                });
                if serde_json::from_str::<Vec<String>>(&self.speech_command_args).is_err() {
                    ui.colored_label(
                        egui::Color32::from_rgb(190, 70, 60),
                        "引数は文字列の配列で入力してください。例: [\"--voice\", \"voice.json\"]",
                    );
                }
            }
            _ => {}
        }
        if !matches!(
            self.speech_provider.as_str(),
            "irodori" | "kokoro" | "aivis" | "aivis_native" | "voiceroid2"
        ) {
            ui.add_space(8.);
            if ui
                .add_sized([110., 34.], egui::Button::new("試聴する"))
                .clicked()
            {
                self.preview_voice();
            }
        }
        if let Some(error) = &self.speech_error {
            ui.add_space(8.);
            ui.add(
                egui::Label::new(
                    RichText::new(error)
                        .size(12.)
                        .color(egui::Color32::from_rgb(190, 70, 60)),
                )
                .wrap(),
            );
        } else if !self.speech_progress.is_empty() {
            ui.add_space(8.);
            ui.add(
                egui::Label::new(
                    RichText::new(&self.speech_progress)
                        .size(12.)
                        .color(p.muted),
                )
                .wrap(),
            );
        }
        controls::section(ui, "読み上げ", p);
        controls::slider_row(
            ui,
            "速度",
            &mut self.settings.tts.speed,
            0.5..=10.0,
            2,
            " ×",
        );
        controls::check(
            ui,
            &mut self.settings.tts.pre_cache,
            "音声を先読みする（取得済みモデルのみ）",
        );
        controls::check(
            ui,
            &mut self.settings.tts.auto_follow,
            "読み上げている位置を自動で表示する",
        );
        self.pronunciation_settings_section(ui, p);
        self.sync_speech_settings();
    }
}

impl ReaderApp {
    fn native_aivis_settings(&mut self, ui: &mut Ui, p: Palette) {
        controls::section(ui, "AivisSpeech · 組み込み", p);
        let mut config = self.provider_config();
        let original = serde_json::to_value(&config).unwrap_or_default();
        let key = egui::Id::unique("native-aivis-model-catalog");
        let state = ui.ctx().data_mut(|data| {
            if let Some(state) = data.get_temp::<Arc<Mutex<NativeAivisCatalog>>>(key) {
                state
            } else {
                let state = Arc::new(Mutex::new(NativeAivisCatalog::default()));
                data.insert_temp(key, state.clone());
                state
            }
        });
        if !state.lock().unwrap_or_else(|e| e.into_inner()).requested {
            fetch_native_aivis(state.clone(), config.clone(), None, ui.ctx().clone());
        }
        let (loading, catalog, imported, status, error) = {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.loading,
                state.catalog.clone(),
                state.imported.take(),
                state.status.clone(),
                state.error.clone(),
            )
        };
        let reader_speech::ProviderConfig::AivisNative {
            model_path,
            speaker,
            style,
            backend,
            auto_download,
            allow_nvidia_download,
            bert_dir,
            ..
        } = &mut config
        else {
            return;
        };
        if let Some(imported) = imported {
            *model_path = imported.path;
            if let Some(first) = imported.info.speakers.first() {
                *speaker = first.local_id;
                *style = first.styles[0].local_id;
            }
        }
        let selected = catalog
            .models
            .iter()
            .find(|entry| entry.path == *model_path);
        let mut changed_model = false;
        controls::row(ui, "音声モデル", |ui, label_id| {
            let width = ui.available_width();
            let label = selected
                .map(|entry| format!("{} {}", entry.info.name, entry.info.version))
                .unwrap_or_else(|| "モデル未選択 / 取得前".into());
            egui::ComboBox::from_id_salt("native-aivis-model")
                .width(width)
                .truncate()
                .selected_text(&label)
                .show_ui(ui, |ui| {
                    ui.set_width(width);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    for entry in &catalog.models {
                        let label = format!("{} {}", entry.info.name, entry.info.version);
                        changed_model |= ui
                            .selectable_value(model_path, entry.path.clone(), &label)
                            .on_hover_text(label)
                            .changed();
                    }
                })
                .response
                .labelled_by(label_id)
                .on_hover_text(label);
        });
        let selected = catalog
            .models
            .iter()
            .find(|entry| entry.path == *model_path);
        if changed_model && let Some(first) = selected.and_then(|entry| entry.info.speakers.first())
        {
            *speaker = first.local_id;
            *style = first.styles[0].local_id;
        }
        if let Some(entry) = selected {
            let mut changed_speaker = false;
            controls::row(ui, "話者", |ui, label_id| {
                let width = ui.available_width();
                let label = entry
                    .info
                    .speakers
                    .iter()
                    .find(|value| value.local_id == *speaker)
                    .map(|value| value.name.as_str())
                    .unwrap_or("話者を選択");
                egui::ComboBox::from_id_salt("native-aivis-speaker")
                    .width(width)
                    .truncate()
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.set_width(width);
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        for value in &entry.info.speakers {
                            changed_speaker |= ui
                                .selectable_value(speaker, value.local_id, &value.name)
                                .on_hover_text(&value.name)
                                .changed();
                        }
                    })
                    .response
                    .labelled_by(label_id)
                    .on_hover_text(label);
            });
            if let Some(voice) = entry
                .info
                .speakers
                .iter()
                .find(|value| value.local_id == *speaker)
            {
                if changed_speaker {
                    *style = voice.styles[0].local_id;
                }
                controls::row(ui, "スタイル", |ui, label_id| {
                    let width = ui.available_width();
                    let label = voice
                        .styles
                        .iter()
                        .find(|value| value.local_id == *style)
                        .map(|value| value.name.as_str())
                        .unwrap_or("スタイルを選択");
                    egui::ComboBox::from_id_salt("native-aivis-style")
                        .width(width)
                        .truncate()
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            ui.set_width(width);
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            for value in &voice.styles {
                                ui.selectable_value(style, value.local_id, &value.name)
                                    .on_hover_text(&value.name);
                            }
                        })
                        .response
                        .labelled_by(label_id)
                        .on_hover_text(label);
                });
            }
            egui::CollapsingHeader::new("選択したモデルの利用条件").show(ui,|ui|{
                ui.hyperlink_to("AivisHubのモデル情報",format!("https://hub.aivis-project.com/aivm-models/{}",entry.info.uuid));
                egui::ScrollArea::vertical().id_salt("native-aivis-model-terms").max_height(200.).show(ui,|ui|{
                    ui.add(egui::Label::new(RichText::new(entry.info.license.as_deref().unwrap_or("ライセンスはモデル内に未提供です。作者の利用条件を確認してください。ReaderのMITライセンスは音声モデルには適用されません。")).size(11.)).wrap());
                });
            });
        } else {
            ui.hyperlink_to(
                "まおのモデル情報・ACML 1.0",
                "https://hub.aivis-project.com/aivm-models/a59cb814-0083-4369-8542-f51a29e72af7",
            );
            ui.add(egui::Label::new(RichText::new("未導入の場合はまお約258MBと日本語BERT約653MBを初回利用時に取得します。各モデルの利用条件が適用されます。").size(12.).color(p.muted)).wrap());
        }
        let mut source = None;
        let mut refresh = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!loading, egui::Button::new("AIVMXを追加"))
                .clicked()
            {
                source = rfd::FileDialog::new()
                    .set_title("JP-Extra AIVMXを追加（元ファイルは変更しません）")
                    .add_filter("AIVMX", &["aivmx"])
                    .pick_file();
            }
            refresh = ui
                .add_enabled(!loading, egui::Button::new("一覧を更新"))
                .clicked();
            if loading {
                ui.spinner();
            }
        });
        if !status.is_empty() {
            ui.label(RichText::new(status).size(12.).color(p.muted));
        }
        if let Some(error) = error {
            ui.add(
                egui::Label::new(
                    RichText::new(error)
                        .size(12.)
                        .color(egui::Color32::from_rgb(190, 70, 60)),
                )
                .wrap(),
            );
        }
        if !catalog.diagnostics.is_empty() {
            egui::CollapsingHeader::new("読み込めないモデル").show(ui, |ui| {
                for error in &catalog.diagnostics {
                    ui.add(
                        egui::Label::new(format!("{}: {}", error.path.display(), error.message))
                            .wrap(),
                    );
                }
            });
        }
        egui::CollapsingHeader::new("実行設定").show(ui,|ui|{
            controls::row(ui,"計算方法",|ui,label_id|{
                egui::ComboBox::from_id_salt("native-aivis-backend").selected_text(backend.as_str()).width(ui.available_width()).show_ui(ui,|ui|{for value in ["auto","cpu","cuda"] {ui.selectable_value(backend,value.into(),value);}}).response.labelled_by(label_id);
            });
            controls::check(ui,auto_download,"不足するモデル・ランタイムを自動取得する");
            ui.horizontal_wrapped(|ui|{
                ui.hyperlink_to("CUDA / cuBLAS の利用条件","https://docs.nvidia.com/cuda/archive/12.8.1/eula/index.html");
                ui.hyperlink_to("cuDNN の利用条件","https://docs.nvidia.com/deeplearning/cudnn/backend/v9.8.0/reference/eula.html");
            });
            controls::check(ui,allow_nvidia_download,"利用条件に同意してNVIDIA GPUランタイムの取得を許可する");
            ui.add(egui::Label::new(RichText::new("未同意でも既存の互換GPUライブラリを利用できます。自動モードでGPUを利用できない場合は同じ音声モデルをCPUで実行します。").size(12.).color(p.muted)).wrap());
            controls::row(ui,"日本語BERT",|ui,label_id|{
                let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.), egui::Sense::hover());
                let pick = egui::Rect::from_min_size(egui::pos2(row.right()-132.,row.top()), egui::vec2(132.,32.));
                let path = egui::Rect::from_min_max(row.min, egui::pos2((pick.left()-8.).max(row.left()), row.bottom()));
                let full_path = bert_dir.display().to_string();
                crate::icons::label_at(ui, path, &full_path, egui::Sense::hover()).labelled_by(label_id).on_hover_text(full_path);
                if ui.place(pick, egui::Button::new("フォルダーを選択")).clicked() && let Some(path)=rfd::FileDialog::new().pick_folder() {*bert_dir=path;}
            });
        });
        if serde_json::to_value(&config).unwrap_or_default() != original {
            self.set_aivis_native_config(config.clone());
            self.sync_speech_settings();
        }
        if source.is_some() || refresh {
            fetch_native_aivis(state, config.clone(), source, ui.ctx().clone());
        }
        if ui
            .add_enabled(!loading, egui::Button::new("選んだ声を試聴（1倍速）"))
            .clicked()
        {
            self.preview_voice();
        }
    }
}

fn provider_label(id: &str) -> &str {
    match id {
        "irodori" => "Irodori（組み込み）",
        "kokoro" => "Kokoro（組み込み）",
        "aivis_native" => "AivisSpeech（組み込み）",
        "aivis" => "AivisSpeech",
        "voiceroid2" => "VOICEROID2",
        "system" => "システム音声",
        "openai" => "OpenAI互換",
        "voicevox" => "VOICEVOX",
        _ => "外部コマンド",
    }
}
