//! Native recorded-audio screen; separate from EPUB speech.
use crate::{
    icons::{self, Icon},
    theme::Palette,
};
use eframe::egui::{self, Color32, Response, RichText, Stroke, vec2};
use reader_speech::{AudiobookEvent, AudiobookInfo, AudiobookPlayer, AudiobookState};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

pub enum AudiobookAction {
    Back,
    Notice(String),
}
struct Opened {
    generation: u64,
    result: Result<AudiobookPlayer, String>,
}
pub struct AudiobookUi {
    generation: u64,
    opening: Option<Receiver<Opened>>,
    path: Option<PathBuf>,
    player: Option<AudiobookPlayer>,
    state: AudiobookState,
    position: Duration,
    speed: f32,
    scrub: f64,
    scrubbing: bool,
    error: Option<String>,
}
impl AudiobookUi {
    pub fn new() -> Self {
        Self {
            generation: 0,
            opening: None,
            path: None,
            player: None,
            state: AudiobookState::Stopped,
            position: Duration::ZERO,
            speed: 1.0,
            scrub: 0.0,
            scrubbing: false,
            error: None,
        }
    }
    /// Metadata and decoder creation stay off the GUI thread. Opening never
    /// starts playback, including when an obsolete open finishes late.
    pub fn open(&mut self, path: PathBuf) {
        self.stop();
        self.player = None;
        self.path = Some(path.clone());
        self.error = None;
        let generation = self.generation;
        let (tx, rx) = mpsc::channel();
        match thread::Builder::new()
            .name("reader-audiobook-open".into())
            .spawn(move || {
                let result = AudiobookPlayer::open(path).map_err(|error| format!("{error:#}"));
                // A superseded receiver is dropped. Its unplayed player is then
                // dropped too, closing the worker without touching the audio DAC.
                let _ = tx.send(Opened { generation, result });
            }) {
            Ok(_) => {
                self.opening = Some(rx);
                self.state = AudiobookState::Preparing;
            }
            Err(error) => self.error = Some(format!("音声ファイルを開けません: {error}")),
        }
    }
    pub fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.opening = None;
        if let Some(player) = &self.player {
            player.stop();
        }
        self.state = AudiobookState::Stopped;
        self.position = Duration::ZERO;
        self.scrub = 0.0;
        self.scrubbing = false;
    }
    fn poll(&mut self, actions: &mut Vec<AudiobookAction>) {
        if let Some(opened) = self.opening.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.opening = None;
            if opened.generation == self.generation {
                match opened.result {
                    Ok(player) => {
                        if let Err(error) = player.set_speed(self.speed) {
                            self.fail(error.to_string(), actions);
                        }
                        self.player = Some(player);
                        self.state = AudiobookState::Stopped;
                    }
                    Err(error) => self.fail(error, actions),
                }
            }
        }
        let mut errors = Vec::new();
        if let Some(player) = &self.player {
            while let Some(event) = player.try_recv() {
                match event {
                    AudiobookEvent::State(state) => self.state = state,
                    AudiobookEvent::Position { position, .. } => self.position = position,
                    AudiobookEvent::Error(error) => errors.push(error),
                }
            }
            self.position = player.position();
        }
        for error in errors {
            self.fail(error, actions);
        }
    }
    fn fail(&mut self, error: String, actions: &mut Vec<AudiobookAction>) {
        self.state = AudiobookState::Stopped;
        self.error = Some(error.clone());
        actions.push(AudiobookAction::Notice(error));
    }
    fn seek(&mut self, seconds: f64, actions: &mut Vec<AudiobookAction>) {
        let position = Duration::from_secs_f64(seconds.max(0.0));
        if let Some(player) = &self.player {
            match player.seek(position) {
                Ok(()) => {
                    self.position = position;
                    self.scrub = position.as_secs_f64();
                }
                Err(error) => self.fail(error.to_string(), actions),
            }
        }
    }
    fn toggle_play(&mut self) {
        if let Some(player) = &self.player {
            if matches!(
                self.state,
                AudiobookState::Playing | AudiobookState::Preparing
            ) {
                player.pause();
                self.state = AudiobookState::Paused;
            } else {
                self.error = None;
                player.play();
                self.state = AudiobookState::Preparing;
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, palette: Palette) -> Vec<AudiobookAction> {
        let mut actions = Vec::new();
        self.poll(&mut actions);
        if self.opening.is_some()
            || matches!(
                self.state,
                AudiobookState::Preparing | AudiobookState::Playing
            )
        {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        ui.horizontal(|ui| {
            if icon_button(ui, Icon::Back, "本棚へ戻る", false, palette).clicked() {
                self.stop();
                actions.push(AudiobookAction::Back);
            }
            ui.add_space(6.0);
            ui.label(RichText::new("オーディオブック").size(18.0).strong());
        });
        ui.add_space(16.0);
        if self.opening.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("音声ファイルを開いています…");
            });
            if let Some(path) = &self.path {
                ui.label(
                    RichText::new(path.file_name().unwrap_or_default().to_string_lossy())
                        .color(palette.muted),
                );
            }
            return actions;
        }
        let Some(info) = self.player.as_ref().map(|player| player.info().clone()) else {
            if let Some(error) = &self.error {
                ui.label(RichText::new(error).color(palette.ink));
            } else {
                ui.label(
                    RichText::new("音声ファイルを開くと、ここで再生できます。")
                        .color(palette.muted),
                );
            }
            return actions;
        };
        if !ui.ctx().egui_wants_keyboard_input() {
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
                self.toggle_play();
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft))
            {
                self.seek(self.position.as_secs_f64() - 15.0, &mut actions);
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
            {
                self.seek(
                    (self.position + Duration::from_secs(15))
                        .as_secs_f64()
                        .min(info.duration.unwrap_or(Duration::MAX).as_secs_f64()),
                    &mut actions,
                );
            }
        }
        egui::ScrollArea::vertical()
            .id_salt("audiobook_screen")
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.set_max_width(760.0);
                    ui.horizontal(|ui| {
                        cover_placeholder(ui, palette);
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(&info.title)
                                    .size(28.0)
                                    .strong()
                                    .color(palette.ink),
                            );
                            if let Some(artist) = &info.artist {
                                ui.label(RichText::new(artist).size(17.0).color(palette.muted));
                            }
                            if let Some(album) = &info.album {
                                ui.label(RichText::new(album).color(palette.muted));
                            }
                            ui.add_space(12.0);
                            ui.label(
                                RichText::new(state_label(self.state))
                                    .size(13.0)
                                    .color(palette.muted),
                            );
                        });
                    });
                    ui.add_space(28.0);
                    self.transport(ui, &info, palette, &mut actions);
                    if let Some(error) = &self.error {
                        ui.add_space(12.0);
                        ui.label(RichText::new(error).color(palette.ink));
                    }
                    if !info.chapters.is_empty() {
                        ui.add_space(28.0);
                        ui.separator();
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("チャプター").size(16.0).strong());
                            ui.label(
                                RichText::new(format!("{}", info.chapters.len()))
                                    .color(palette.muted),
                            );
                        });
                        let current = current_chapter(&info, self.position);
                        egui::ScrollArea::vertical()
                            .id_salt("audiobook_chapters")
                            .max_height((ui.available_height() - 16.0).max(150.0))
                            .show_rows(ui, 42.0, info.chapters.len(), |ui, rows| {
                                for index in rows {
                                    let chapter = &info.chapters[index];
                                    let label = format!(
                                        "{:02}    {}    {}",
                                        index + 1,
                                        chapter.title,
                                        format_time(chapter.start)
                                    );
                                    let selected = current == Some(index);
                                    let response = ui.add_sized(
                                        [ui.available_width(), 38.0],
                                        egui::Button::new(RichText::new(label).color(
                                            if selected {
                                                palette.accent
                                            } else {
                                                palette.ink
                                            },
                                        ))
                                        .selected(selected)
                                        .frame(selected),
                                    );
                                    if response.clicked() {
                                        self.seek(chapter.start.as_secs_f64(), &mut actions);
                                    }
                                }
                            });
                    } else {
                        ui.add_space(24.0);
                        ui.label(
                            RichText::new("このファイルにチャプター情報はありません。")
                                .size(13.0)
                                .color(palette.muted),
                        );
                    }
                });
            });
        actions
    }
    fn transport(
        &mut self,
        ui: &mut egui::Ui,
        info: &AudiobookInfo,
        palette: Palette,
        actions: &mut Vec<AudiobookAction>,
    ) {
        if !self.scrubbing {
            self.scrub = self.position.as_secs_f64();
        }
        let maximum = info
            .duration
            .map(|duration| duration.as_secs_f64())
            .unwrap_or_else(|| (self.position.as_secs_f64() + 60.0).max(60.0))
            .max(0.001);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format_time(Duration::from_secs_f64(self.scrub.max(0.0))))
                    .monospace()
                    .size(14.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(
                        info.duration
                            .map(format_time)
                            .unwrap_or_else(|| "--:--".into()),
                    )
                    .monospace()
                    .size(14.0)
                    .color(palette.muted),
                );
            });
        });
        ui.spacing_mut().slider_width = (ui.available_width() - 4.0).max(100.0);
        let response = ui.add(
            egui::Slider::new(&mut self.scrub, 0.0..=maximum)
                .show_value(false)
                .trailing_fill(true),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Slider, ui.is_enabled(), "再生位置")
        });
        self.scrubbing = response.dragged();
        if response.drag_stopped() || (response.changed() && !response.dragged()) {
            self.seek(self.scrub, actions);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let controls_width = 4.0 * 44.0 + 3.0 * 10.0 + 100.0;
            ui.add_space(((ui.available_width() - controls_width) * 0.5).max(0.0));
            if icon_button(
                ui,
                Icon::Back,
                if info.chapters.is_empty() {
                    "15秒戻る（←）"
                } else {
                    "前のチャプター"
                },
                false,
                palette,
            )
            .clicked()
            {
                self.seek(
                    previous_position(info, self.position).as_secs_f64(),
                    actions,
                );
            }
            let active = matches!(
                self.state,
                AudiobookState::Preparing | AudiobookState::Playing
            );
            if icon_button(
                ui,
                if active { Icon::Pause } else { Icon::Play },
                if active {
                    "一時停止（Space）"
                } else {
                    "再生（Space）"
                },
                true,
                palette,
            )
            .clicked()
            {
                self.toggle_play();
            }
            if icon_button(
                ui,
                Icon::Next,
                if info.chapters.is_empty() {
                    "15秒進む（→）"
                } else {
                    "次のチャプター"
                },
                false,
                palette,
            )
            .clicked()
            {
                self.seek(next_position(info, self.position).as_secs_f64(), actions);
            }
            if icon_button(ui, Icon::Stop, "停止して先頭へ戻る", false, palette).clicked()
            {
                self.stop();
            }
            ui.add_space(12.0);
            let old = self.speed;
            egui::ComboBox::from_id_salt("audiobook_speed")
                .width(78.0)
                .selected_text(format!("{:.2}×", self.speed))
                .show_ui(ui, |ui| {
                    for speed in [
                        0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 7.5, 10.0,
                    ] {
                        ui.selectable_value(&mut self.speed, speed, format!("{speed:.2}×"));
                    }
                })
                .response
                .on_hover_text("再生速度");
            if old != self.speed
                && let Some(player) = &self.player
                && let Err(error) = player.set_speed(self.speed)
            {
                self.speed = old;
                self.fail(error.to_string(), actions);
            }
        });
    }
}
fn state_label(state: AudiobookState) -> &'static str {
    match state {
        AudiobookState::Stopped => "再生待ち",
        AudiobookState::Preparing => "再生を準備しています…",
        AudiobookState::Playing => "再生中",
        AudiobookState::Paused => "一時停止",
        AudiobookState::Ended => "再生終了",
    }
}
fn format_time(time: Duration) -> String {
    let seconds = time.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}
fn current_chapter(info: &AudiobookInfo, position: Duration) -> Option<usize> {
    info.chapters
        .partition_point(|chapter| chapter.start <= position)
        .checked_sub(1)
}
fn previous_position(info: &AudiobookInfo, position: Duration) -> Duration {
    if info.chapters.is_empty() {
        return position.saturating_sub(Duration::from_secs(15));
    }
    let index = current_chapter(info, position).unwrap_or(0);
    let current = info.chapters[index].start;
    if position > current + Duration::from_secs(2) {
        current
    } else {
        info.chapters[index.saturating_sub(1)].start
    }
}
fn next_position(info: &AudiobookInfo, position: Duration) -> Duration {
    if info.chapters.is_empty() {
        return (position + Duration::from_secs(15)).min(info.duration.unwrap_or(Duration::MAX));
    }
    info.chapters
        .iter()
        .find(|chapter| chapter.start > position)
        .map_or_else(
            || info.duration.unwrap_or(position),
            |chapter| chapter.start,
        )
}
fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    active: bool,
    palette: Palette,
) -> Response {
    let response = ui.add(
        egui::Button::new("")
            .min_size(vec2(44.0, 44.0))
            .corner_radius(10.0)
            .fill(if active {
                palette.selection
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::NONE),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    icons::paint(
        ui.painter(),
        response.rect.center(),
        icon,
        if active { palette.accent } else { palette.ink },
    );
    response.on_hover_text(label)
}
fn cover_placeholder(ui: &mut egui::Ui, palette: Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(120.0, 144.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 16.0, palette.panel);
    let c = rect.center();
    let stroke = Stroke::new(2.0, palette.accent);
    painter.line_segment([c + vec2(-8.0, 19.0), c + vec2(-8.0, -20.0)], stroke);
    painter.line_segment([c + vec2(-8.0, -20.0), c + vec2(18.0, -27.0)], stroke);
    painter.line_segment([c + vec2(18.0, -27.0), c + vec2(18.0, 12.0)], stroke);
    painter.circle_filled(c + vec2(-14.0, 20.0), 6.5, palette.accent);
    painter.circle_filled(c + vec2(12.0, 13.0), 6.5, palette.accent);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn loaded_fixture() -> AudiobookUi {
        let data =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../reader-speech/tests/data");
        let mut player = AudiobookUi::new();
        player.open(data.join("tone.mp3"));
        player.open(data.join("chapters.m4b"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut actions = Vec::new();
        while player.opening.is_some() {
            player.poll(&mut actions);
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(actions.is_empty());
        assert_eq!(
            player.player.as_ref().unwrap().info().title,
            "Native chapter fixture"
        );
        assert_eq!(player.state, AudiobookState::Stopped);
        assert_eq!(player.position, Duration::ZERO);
        player
    }
    #[test]
    fn background_open_keeps_only_the_latest_file_and_never_autoplays() {
        let mut player = loaded_fixture();
        let data = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../reader-speech/tests/data/tone.mp3");
        player.open(data);
        player.stop();
        thread::sleep(Duration::from_millis(30));
        player.poll(&mut Vec::new());
        assert!(player.opening.is_none());
        assert!(player.player.is_none());
        assert_eq!(player.state, AudiobookState::Stopped);
    }
    #[test]
    fn native_audio_screen_fits_minimum_window_in_all_themes() {
        let mut player = loaded_fixture();
        for theme in [
            reader_library::Theme::Light,
            reader_library::Theme::Dark,
            reader_library::Theme::Sepia,
        ] {
            let settings = reader_library::Settings {
                theme,
                ..Default::default()
            };
            let palette = Palette::for_settings(&settings);
            for width in [820.0, 1280.0] {
                let context = egui::Context::default();
                palette.apply(&context, theme == reader_library::Theme::Dark);
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        vec2(width, 580.0),
                    )),
                    ..Default::default()
                };
                let mut output = context.run_ui(input, |root_ui| {
                    egui::CentralPanel::default().show(root_ui, |ui| {
                        let _ = player.show(ui, palette);
                        assert!(
                            ui.min_rect().right() <= width + 1.0,
                            "screen overflow at {width}: {:?}",
                            ui.min_rect()
                        );
                    });
                });
                assert!(!output.shapes.is_empty());
                output.textures_delta.clear();
            }
        }
    }
    #[test]
    fn chapter_navigation_has_explicit_restart_and_end_behavior() {
        use reader_speech::AudiobookChapter;
        let info = AudiobookInfo {
            path: "book.m4b".into(),
            title: "Book".into(),
            artist: None,
            album: None,
            duration: Some(Duration::from_secs(120)),
            sample_rate: 48000,
            channels: 2,
            chapters: [0, 40, 80]
                .map(|seconds| AudiobookChapter {
                    title: seconds.to_string(),
                    start: Duration::from_secs(seconds),
                    end: None,
                })
                .into(),
        };
        assert_eq!(
            previous_position(&info, Duration::from_secs(41)),
            Duration::ZERO
        );
        assert_eq!(
            previous_position(&info, Duration::from_secs(43)),
            Duration::from_secs(40)
        );
        assert_eq!(
            next_position(&info, Duration::from_secs(80)),
            Duration::from_secs(120)
        );
        assert_eq!(format_time(Duration::from_secs(3661)), "1:01:01");
    }
}
