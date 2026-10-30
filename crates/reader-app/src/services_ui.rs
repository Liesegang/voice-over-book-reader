//! Explicitly invoked native network tools. Opening a dialog never starts HTTP.
use eframe::egui::{self, RichText, Ui};
use reader_library::{
    OpdsClient, OpdsConfig, OpdsPage, Settings, TranslationClient, TranslationConfig,
    TranslationProvider, TranslationResult, WebImportResult, import_web_page, unix_millis,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

pub enum ServiceAction {
    OpenBook(PathBuf),
    OpenParallel(PathBuf),
    SettingsChanged,
    Notice(String),
}

type ServiceResult<T> = Result<T, String>;

/// Generations reject replies after close/reopen. Cancellation also stops the
/// next translation chunk; an already sent HTTP request cannot be recalled.
#[derive(Default)]
struct Request {
    generation: u64,
    busy: bool,
    error: Option<String>,
    cancelled: Arc<AtomicBool>,
}
impl Request {
    fn start(&mut self) -> (u64, Arc<AtomicBool>) {
        self.cancel();
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.busy = true;
        self.error = None;
        (self.generation, self.cancelled.clone())
    }
    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.generation = self.generation.wrapping_add(1);
        self.busy = false;
    }
    fn current(&self, generation: u64) -> bool {
        self.busy && self.generation == generation
    }
    fn finish(&mut self, generation: u64) -> bool {
        if self.current(generation) {
            self.busy = false;
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy)]
enum Navigation {
    Root,
    Forward,
    Back,
    Reload,
}
enum Work {
    Catalog {
        generation: u64,
        navigation: Navigation,
        config: OpdsConfig,
        result: ServiceResult<OpdsPage>,
    },
    Download {
        generation: u64,
        result: ServiceResult<PathBuf>,
    },
    Imported {
        generation: u64,
        result: ServiceResult<WebImportResult>,
    },
    Progress {
        generation: u64,
        completed: usize,
        total: usize,
    },
    Translated {
        generation: u64,
        target: String,
        title: String,
        result: ServiceResult<TranslationResult>,
    },
    Saved {
        generation: u64,
        result: ServiceResult<PathBuf>,
    },
}
struct TranslationOutput {
    title: String,
    target: String,
    result: TranslationResult,
}

pub struct ServicesUi {
    data_dir: PathBuf,
    tx: Sender<Work>,
    rx: Receiver<Work>,
    catalog_open: bool,
    catalog_url: String,
    catalog_username: String,
    catalog_password: String,
    remember_credentials: bool,
    catalog_page: Option<OpdsPage>,
    catalog_config: Option<OpdsConfig>,
    catalog_history: Vec<String>,
    catalog_request: Request,
    download_request: Request,
    downloading_title: String,
    web_open: bool,
    web_url: String,
    web_request: Request,
    web_message: Option<String>,
    translation_open: bool,
    translation_title: String,
    translation_source: String,
    translation_provider: TranslationProvider,
    translation_key: String,
    translation_folder: String,
    translation_target: String,
    source_language: String,
    translation_request: Request,
    translation_progress: (usize, usize),
    translation_output: Option<TranslationOutput>,
    save_request: Request,
    saved_translation: Option<PathBuf>,
}

impl ServicesUi {
    pub fn new(settings: &Settings, data_dir: PathBuf) -> Self {
        let catalog = settings.opds.clone().unwrap_or_default();
        let translation = settings.translation.clone().unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        Self {
            data_dir,
            tx,
            rx,
            catalog_open: false,
            remember_credentials: catalog.username.is_some() || catalog.password.is_some(),
            catalog_url: catalog.url,
            catalog_username: catalog.username.unwrap_or_default(),
            catalog_password: catalog.password.unwrap_or_default(),
            catalog_page: None,
            catalog_config: None,
            catalog_history: Vec::new(),
            catalog_request: Request::default(),
            download_request: Request::default(),
            downloading_title: String::new(),
            web_open: false,
            web_url: String::new(),
            web_request: Request::default(),
            web_message: None,
            translation_open: false,
            translation_title: String::new(),
            translation_source: String::new(),
            translation_provider: translation.provider,
            translation_key: translation.api_key,
            translation_folder: translation.folder_id.unwrap_or_default(),
            translation_target: settings.translation_target.clone(),
            source_language: String::new(),
            translation_request: Request::default(),
            translation_progress: (0, 0),
            translation_output: None,
            save_request: Request::default(),
            saved_translation: None,
        }
    }
    pub fn open_catalog(&mut self) {
        self.catalog_open = true;
    }
    pub fn open_web_import(&mut self) {
        self.web_open = true;
    }
    pub fn open_translation(&mut self, text: String, title: String) {
        self.translation_request.cancel();
        self.save_request.cancel();
        self.translation_request.error = None;
        self.save_request.error = None;
        self.translation_source = text;
        self.translation_title = title;
        self.translation_output = None;
        self.saved_translation = None;
        self.translation_progress = (0, 0);
        self.translation_open = true;
    }
    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) -> Vec<ServiceAction> {
        let mut actions = Vec::new();
        self.receive(&mut actions);
        if self.catalog_open {
            self.catalog_window(ctx, settings, &mut actions);
        }
        if self.web_open {
            self.web_window(ctx);
        }
        if self.translation_open {
            self.translation_window(ctx, settings, &mut actions);
        }
        if self.catalog_request.busy
            || self.download_request.busy
            || self.web_request.busy
            || self.translation_request.busy
            || self.save_request.busy
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        actions
    }
    fn receive(&mut self, actions: &mut Vec<ServiceAction>) {
        while let Ok(work) = self.rx.try_recv() {
            match work {
                Work::Catalog {
                    generation,
                    navigation,
                    config,
                    result,
                } if self.catalog_request.finish(generation) => match result {
                    Ok(page) => {
                        match navigation {
                            Navigation::Root => self.catalog_history.clear(),
                            Navigation::Forward => {
                                if let Some(old) = &self.catalog_page
                                    && old.url != page.url
                                {
                                    self.catalog_history.push(old.url.clone());
                                }
                            }
                            Navigation::Back => {
                                self.catalog_history.pop();
                            }
                            Navigation::Reload => {}
                        }
                        self.catalog_config = Some(config);
                        self.catalog_page = Some(page);
                    }
                    Err(error) => self.catalog_request.error = Some(error),
                },
                Work::Download { generation, result }
                    if self.download_request.finish(generation) =>
                {
                    match result {
                        Ok(path) => actions.push(ServiceAction::OpenBook(path)),
                        Err(error) => self.download_request.error = Some(error),
                    }
                }
                Work::Imported { generation, result } if self.web_request.finish(generation) => {
                    match result {
                        Ok(imported) => {
                            let mut message = format!(
                                "「{}」を取り込みました（画像 {} 件）",
                                imported.title, imported.image_count
                            );
                            for warning in imported.warnings {
                                message.push('\n');
                                message.push_str(&warning);
                            }
                            self.web_message = Some(message.clone());
                            actions.push(ServiceAction::Notice(message));
                            actions.push(ServiceAction::OpenBook(imported.path));
                        }
                        Err(error) => self.web_request.error = Some(error),
                    }
                }
                Work::Progress {
                    generation,
                    completed,
                    total,
                } if self.translation_request.current(generation) => {
                    self.translation_progress = (completed, total);
                }
                Work::Translated {
                    generation,
                    target,
                    title,
                    result,
                } if self.translation_request.finish(generation) => match result {
                    Ok(result) => {
                        self.translation_output = Some(TranslationOutput {
                            title,
                            target,
                            result,
                        })
                    }
                    Err(error) => self.translation_request.error = Some(error),
                },
                Work::Saved { generation, result } if self.save_request.finish(generation) => {
                    match result {
                        Ok(path) => {
                            self.saved_translation = Some(path.clone());
                            actions.push(ServiceAction::OpenParallel(path));
                        }
                        Err(error) => self.save_request.error = Some(error),
                    }
                }
                _ => {} // A closed/replaced dialog cannot apply an old reply.
            }
        }
    }

    fn catalog_window(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        actions: &mut Vec<ServiceAction>,
    ) {
        let mut open = self.catalog_open;
        egui::Window::new("オンライン書庫 · OPDS")
            .id(egui::Id::unique("network-catalog"))
            .open(&mut open)
            .default_width(700.)
            .max_height((ctx.content_rect().height() - 48.).max(240.))
            .vscroll(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("OPDS 1 / 2 カタログからEPUBをダウンロードできます。");
                ui.add_space(8.);
                ui.label("カタログURL");
                ui.add(
                    egui::TextEdit::singleline(&mut self.catalog_url)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://example.org/opds"),
                );
                egui::CollapsingHeader::new("認証・保存設定").show(ui, |ui| {
                    ui.label("ユーザー名（必要な場合）");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.catalog_username)
                            .desired_width(f32::INFINITY),
                    );
                    ui.label("パスワード");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.catalog_password)
                            .password(true)
                            .desired_width(f32::INFINITY),
                    );
                    ui.checkbox(
                        &mut self.remember_credentials,
                        "認証情報もこの端末の設定に保存する",
                    );
                    ui.small("認証情報を保存すると、アプリの設定ファイルに平文で記録されます。");
                    if ui.button("カタログ設定を保存").clicked() {
                        let mut config = self.input_catalog_config();
                        if !self.remember_credentials {
                            config.username = None;
                            config.password = None;
                        }
                        settings.opds = (!config.url.is_empty()).then_some(config);
                        actions.push(ServiceAction::SettingsChanged);
                        actions.push(ServiceAction::Notice("カタログ設定を保存しました".into()));
                    }
                });
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            !self.catalog_request.busy && !self.catalog_url.trim().is_empty(),
                            egui::Button::new("接続"),
                        )
                        .clicked()
                    {
                        self.fetch_catalog(
                            ctx,
                            self.catalog_url.trim().to_owned(),
                            Navigation::Root,
                        );
                    }
                    if ui
                        .add_enabled(
                            !self.catalog_request.busy && !self.catalog_history.is_empty(),
                            egui::Button::new("戻る"),
                        )
                        .clicked()
                        && let Some(url) = self.catalog_history.last().cloned()
                    {
                        self.fetch_catalog(ctx, url, Navigation::Back);
                    }
                    if ui
                        .add_enabled(
                            !self.catalog_request.busy && self.catalog_page.is_some(),
                            egui::Button::new("再読み込み"),
                        )
                        .clicked()
                        && let Some(page) = &self.catalog_page
                    {
                        self.fetch_catalog(ctx, page.url.clone(), Navigation::Reload);
                    }
                    request_status(ui, &mut self.catalog_request, "カタログを取得中…");
                });
                show_error(ui, &self.catalog_request);
                let label = format!("「{}」をダウンロード中…", self.downloading_title);
                request_status(ui, &mut self.download_request, &label);
                show_error(ui, &self.download_request);
                ui.separator();
                let mut navigate = None;
                let mut download = None;
                if let Some(page) = &self.catalog_page {
                    ui.heading(&page.title);
                    ui.horizontal_wrapped(|ui| {
                        ui.small(format!(
                            "{} 冊 · {} カテゴリ",
                            page.publications.len(),
                            page.navigation.len()
                        ));
                        if let Some(url) = &page.previous
                            && ui
                                .add_enabled(
                                    !self.catalog_request.busy,
                                    egui::Button::new("前のページ"),
                                )
                                .clicked()
                        {
                            navigate = Some(url.clone());
                        }
                        if let Some(url) = &page.next
                            && ui
                                .add_enabled(
                                    !self.catalog_request.busy,
                                    egui::Button::new("次のページ"),
                                )
                                .clicked()
                        {
                            navigate = Some(url.clone());
                        }
                    });
                    for link in &page.navigation {
                        let label = if link.title.is_empty() {
                            "カタログを開く"
                        } else {
                            &link.title
                        };
                        if ui
                            .add_enabled(!self.catalog_request.busy, egui::Button::new(label))
                            .clicked()
                        {
                            navigate = Some(link.href.clone());
                        }
                    }
                    if !page.navigation.is_empty() {
                        ui.separator();
                    }
                    if page.publications.is_empty() && page.navigation.is_empty() {
                        ui.label("このカタログに表示できる本はありません。");
                    }
                    for (index, publication) in page.publications.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.add_space(6.);
                            ui.strong(&publication.title);
                            if !publication.authors.is_empty() {
                                ui.small(publication.authors.join("、"));
                            }
                            if let Some(description) = &publication.description {
                                ui.label(preview(description, 220));
                            }
                            if let Some(url) = &publication.epub_url {
                                if ui
                                    .add_enabled(
                                        !self.download_request.busy,
                                        egui::Button::new("EPUBをダウンロードして開く"),
                                    )
                                    .clicked()
                                {
                                    download = Some((url.clone(), publication.title.clone()));
                                }
                            } else {
                                ui.weak("直接ダウンロードできるEPUBはありません");
                            }
                            ui.add_space(6.);
                            ui.separator();
                        });
                    }
                } else if !self.catalog_request.busy {
                    ui.weak("URLを入力して「接続」を押してください。");
                }
                if let Some(url) = navigate {
                    self.fetch_catalog(ctx, url, Navigation::Forward);
                }
                if let Some((url, title)) = download {
                    self.download_book(ctx, url, title);
                }
            });
        self.catalog_open = open;
        if !open {
            self.catalog_request.cancel();
            self.download_request.cancel();
        }
    }
    fn input_catalog_config(&self) -> OpdsConfig {
        OpdsConfig {
            url: self.catalog_url.trim().to_owned(),
            username: optional(&self.catalog_username),
            password: (!self.catalog_password.is_empty()).then(|| self.catalog_password.clone()),
        }
    }
    fn fetch_catalog(&mut self, ctx: &egui::Context, url: String, navigation: Navigation) {
        let config = if matches!(navigation, Navigation::Root) {
            self.input_catalog_config()
        } else {
            self.catalog_config
                .clone()
                .unwrap_or_else(|| self.input_catalog_config())
        };
        let (generation, _) = self.catalog_request.start();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = OpdsClient::new(config.clone())
                .and_then(|client| client.fetch(&url))
                .map_err(|error| error.to_string());
            let _ = tx.send(Work::Catalog {
                generation,
                navigation,
                config,
                result,
            });
            ctx.request_repaint();
        });
    }
    fn download_book(&mut self, ctx: &egui::Context, url: String, title: String) {
        let Some(config) = self.catalog_config.clone() else {
            return;
        };
        self.downloading_title = title;
        let (generation, _) = self.download_request.start();
        let data_dir = self.data_dir.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = OpdsClient::new(config)
                .and_then(|client| client.download_epub(&url, &data_dir))
                .map_err(|error| error.to_string());
            let _ = tx.send(Work::Download { generation, result });
            ctx.request_repaint();
        });
    }

    fn web_window(&mut self, ctx: &egui::Context) {
        let mut open = self.web_open;
        egui::Window::new("Webページを取り込む").id(egui::Id::unique("network-web-import"))
            .open(&mut open).default_width(580.).max_height((ctx.content_rect().height() - 48.).max(240.))
            .vscroll(true).collapsible(false).show(ctx, |ui| {
                ui.label("記事の本文と画像を保存し、このリーダーで開きます。");
                ui.small("静的なHTMLに対応しています。ログインやJavaScriptが必要なページは取り込めません。");
                ui.add_space(12.); ui.label("ページURL");
                ui.add_enabled(!self.web_request.busy, egui::TextEdit::singleline(&mut self.web_url).desired_width(f32::INFINITY).hint_text("https://example.org/article"));
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.web_request.busy && !self.web_url.trim().is_empty(), egui::Button::new("取り込んで開く")).clicked() { self.start_web_import(ctx); }
                    request_status(ui, &mut self.web_request, "本文と画像を取得中…");
                });
                show_error(ui, &self.web_request);
                if let Some(message) = &self.web_message { ui.label(message); }
            });
        self.web_open = open;
        if !open {
            self.web_request.cancel();
        }
    }
    fn start_web_import(&mut self, ctx: &egui::Context) {
        let url = self.web_url.trim().to_owned();
        let data_dir = self.data_dir.clone();
        let (generation, _) = self.web_request.start();
        self.web_message = None;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = import_web_page(&url, &data_dir).map_err(|error| error.to_string());
            let _ = tx.send(Work::Imported { generation, result });
            ctx.request_repaint();
        });
    }

    fn translation_window(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        actions: &mut Vec<ServiceAction>,
    ) {
        let mut open = self.translation_open;
        egui::Window::new("翻訳")
            .id(egui::Id::unique("network-translation"))
            .open(&mut open)
            .default_width(720.)
            .max_height((ctx.content_rect().height() - 48.).max(240.))
            .vscroll(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.strong(&self.translation_title);
                ui.add_space(8.);
                let busy = self.translation_request.busy;
                egui::CollapsingHeader::new("翻訳サービス・APIキー")
                    .default_open(self.translation_key.is_empty())
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.label("サービス");
                            egui::ComboBox::from_id_salt("translation-provider")
                                .selected_text(provider_name(self.translation_provider))
                                .show_ui(ui, |ui| {
                                    for provider in [
                                        TranslationProvider::DeepLFree,
                                        TranslationProvider::DeepLPro,
                                        TranslationProvider::Yandex,
                                    ] {
                                        ui.selectable_value(
                                            &mut self.translation_provider,
                                            provider,
                                            provider_name(provider),
                                        );
                                    }
                                });
                            ui.label("APIキー");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.translation_key)
                                    .password(true)
                                    .desired_width(f32::INFINITY),
                            );
                            if self.translation_provider == TranslationProvider::Yandex {
                                ui.label("Yandex Cloud フォルダーID");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.translation_folder)
                                        .desired_width(f32::INFINITY),
                                );
                            }
                            ui.small(
                                "キーを保存すると、この端末の設定ファイルに平文で記録されます。",
                            );
                            if ui.button("翻訳設定・キーを保存").clicked() {
                                settings.translation = (!self.translation_key.trim().is_empty())
                                    .then(|| self.input_translation_config());
                                settings.translation_target =
                                    self.translation_target.trim().to_owned();
                                actions.push(ServiceAction::SettingsChanged);
                                actions
                                    .push(ServiceAction::Notice("翻訳設定を保存しました".into()));
                            }
                        });
                    });
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("訳文の言語");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.translation_target)
                                .desired_width(72.)
                                .hint_text("JA"),
                        );
                        ui.label("原文の言語");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.source_language)
                                .desired_width(100.)
                                .hint_text("空欄で自動検出"),
                        );
                    });
                    ui.small("言語コード例：JA（日本語）、EN（英語）、DE（ドイツ語）");
                    ui.add_space(8.);
                    ui.label("原文");
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut self.translation_source)
                                .desired_width(f32::INFINITY)
                                .desired_rows(5),
                        )
                        .changed()
                    {
                        self.translation_output = None;
                        self.saved_translation = None;
                        self.save_request.cancel();
                    }
                });
                ui.small(format!(
                    "{} 文字 · 長い章は10,000文字以内に分割して順に送信します",
                    self.translation_source.chars().count()
                ));
                ui.small("原文は選んだサービスへ送信されます。サービスのAPI利用料が適用されます。");
                ui.add_space(8.);
                let can_translate = !busy
                    && !self.translation_source.trim().is_empty()
                    && !self.translation_key.trim().is_empty()
                    && !self.translation_target.trim().is_empty();
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(can_translate, egui::Button::new("翻訳する"))
                        .clicked()
                    {
                        self.start_translation(ctx);
                    }
                    let (completed, total) = self.translation_progress;
                    let label = if total > 1 {
                        format!("翻訳中… {completed} / {total}")
                    } else {
                        "翻訳中…".into()
                    };
                    request_status(ui, &mut self.translation_request, &label);
                });
                show_error(ui, &self.translation_request);
                let mut save = false;
                if let Some(output) = &mut self.translation_output {
                    ui.add_space(12.);
                    ui.separator();
                    ui.heading(format!("訳文 · {}", output.target));
                    if let Some(source) = &output.result.detected_source {
                        ui.small(format!("検出された原文の言語：{source}"));
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut output.result.text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(12)
                            .interactive(false),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("訳文をコピー").clicked() {
                            ctx.copy_text(output.result.text.clone());
                            actions.push(ServiceAction::Notice("訳文をコピーしました".into()));
                        }
                        if ui
                            .add_enabled(
                                !self.save_request.busy,
                                egui::Button::new("TXTを保存して並べて読む"),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                        request_status(ui, &mut self.save_request, "保存中…");
                    });
                    ui.small("訳文のTXTはアプリのデータフォルダーに保存されます。");
                }
                if save {
                    if let Some(path) = &self.saved_translation {
                        actions.push(ServiceAction::OpenParallel(path.clone()));
                    } else {
                        self.save_translation(ctx);
                    }
                }
                show_error(ui, &self.save_request);
            });
        self.translation_open = open;
        if !open {
            self.translation_request.cancel();
            self.save_request.cancel();
        }
    }
    fn input_translation_config(&self) -> TranslationConfig {
        TranslationConfig {
            provider: self.translation_provider,
            api_key: self.translation_key.trim().to_owned(),
            folder_id: optional(&self.translation_folder),
        }
    }
    fn start_translation(&mut self, ctx: &egui::Context) {
        let client = match TranslationClient::new(self.input_translation_config()) {
            Ok(client) => client,
            Err(error) => {
                self.translation_request.error = Some(error.to_string());
                return;
            }
        };
        let target = self.translation_target.trim().to_owned();
        if !valid_language_code(&target) {
            self.translation_request.error =
                Some("訳文の言語をJAやENなどの言語コードで指定してください".into());
            return;
        }
        let source = optional(&self.source_language);
        if source.as_deref().is_some_and(|source| {
            !source.eq_ignore_ascii_case("auto") && !valid_language_code(source)
        }) {
            self.translation_request.error =
                Some("原文の言語を言語コードで指定するか、空欄にしてください".into());
            return;
        }
        let chunks = translation_chunks(&self.translation_source);
        if chunks.is_empty() {
            return;
        }
        let title = self.translation_title.clone();
        self.translation_output = None;
        self.saved_translation = None;
        self.save_request.cancel();
        self.save_request.error = None;
        self.translation_progress = (0, chunks.len());
        let (generation, cancelled) = self.translation_request.start();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let total = chunks.len();
            let mut translated = Vec::with_capacity(total);
            let mut detected_source = None;
            for (index, chunk) in chunks.iter().enumerate() {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                if chunk.trim().is_empty() {
                    // Retain source spacing without sending a billable request
                    // for an empty segment.
                    translated.push(chunk.clone());
                    continue;
                }
                match client.translate(chunk, &target, source.as_deref()) {
                    Ok(result) => {
                        if detected_source.is_none() {
                            detected_source = result.detected_source;
                        }
                        translated.push(result.text);
                    }
                    Err(error) => {
                        let result = Err(format!(
                            "翻訳に失敗しました（{} / {}）。自動再試行はしていません。\n{}",
                            index + 1,
                            total,
                            error
                        ));
                        let _ = tx.send(Work::Translated {
                            generation,
                            target,
                            title,
                            result,
                        });
                        ctx.request_repaint();
                        return;
                    }
                }
                let _ = tx.send(Work::Progress {
                    generation,
                    completed: index + 1,
                    total,
                });
                ctx.request_repaint();
            }
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let result = Ok(TranslationResult {
                text: translated.join("\n\n"),
                detected_source,
            });
            let _ = tx.send(Work::Translated {
                generation,
                target,
                title,
                result,
            });
            ctx.request_repaint();
        });
    }
    fn save_translation(&mut self, ctx: &egui::Context) {
        let Some(output) = &self.translation_output else {
            return;
        };
        let title = output.title.clone();
        let text = output.result.text.clone();
        let data_dir = self.data_dir.clone();
        let (generation, _) = self.save_request.start();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = write_translation(&data_dir, &title, &text, generation)
                .map_err(|error| format!("訳文を保存できませんでした：{error}"));
            let _ = tx.send(Work::Saved { generation, result });
            ctx.request_repaint();
        });
    }
}

impl Drop for ServicesUi {
    fn drop(&mut self) {
        self.catalog_request.cancel();
        self.download_request.cancel();
        self.web_request.cancel();
        self.translation_request.cancel();
        self.save_request.cancel();
    }
}
fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
fn provider_name(provider: TranslationProvider) -> &'static str {
    match provider {
        TranslationProvider::DeepLFree => "DeepL API Free",
        TranslationProvider::DeepLPro => "DeepL API Pro",
        TranslationProvider::Yandex => "Yandex Cloud Translate",
    }
}
fn valid_language_code(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
}
fn show_error(ui: &mut Ui, request: &Request) {
    if let Some(error) = &request.error {
        ui.add_space(6.);
        ui.label(RichText::new(error).color(ui.visuals().error_fg_color));
    }
}
fn request_status(ui: &mut Ui, request: &mut Request, label: &str) {
    if request.busy {
        ui.spinner();
        ui.label(label);
        if ui
            .button("結果を破棄")
            .on_hover_text(
                "開始済みの通信は続く場合があります。長文翻訳の後続リクエストは停止します。",
            )
            .clicked()
        {
            request.cancel();
        }
    }
}
fn preview(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}
/// Prefer paragraphs, preserve UTF-8, and keep each provider request <=10,000 characters.
fn translation_chunks(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut remaining = text;
    let mut chunks = Vec::new();
    while !remaining.is_empty() {
        let Some((hard_end, _)) = remaining.char_indices().nth(10_000) else {
            chunks.push(remaining.to_owned());
            break;
        };
        let candidate = &remaining[..hard_end];
        let end = candidate
            .rfind('\n')
            .map(|index| index + 1)
            .filter(|index| *index >= hard_end / 2)
            .unwrap_or(hard_end);
        let chunk = &remaining[..end];
        chunks.push(chunk.to_owned());
        remaining = &remaining[end..];
    }
    chunks
}
fn write_translation(
    data_dir: &Path,
    title: &str,
    text: &str,
    generation: u64,
) -> std::io::Result<PathBuf> {
    let directory = data_dir.join("translations");
    fs::create_dir_all(&directory)?;
    let stamp = unix_millis();
    for suffix in 0..100 {
        let path = directory.join(format!("translation-{stamp}-{generation}-{suffix}.txt"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                write!(file, "{title}\n\n{text}\n")?;
                file.sync_all()?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "Could not allocate a translation file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constructing_and_opening_dialogs_does_not_start_requests() {
        let mut services = ServicesUi::new(&Settings::default(), PathBuf::from("unused"));
        services.open_catalog();
        services.open_web_import();
        services.open_translation("原文".into(), "章".into());
        assert!(
            !services.catalog_request.busy
                && !services.download_request.busy
                && !services.web_request.busy
                && !services.translation_request.busy
        );
        assert!(services.rx.try_recv().is_err());
    }
    #[test]
    fn stale_reply_cannot_finish_new_request() {
        let mut request = Request::default();
        let (first, cancelled) = request.start();
        let (second, _) = request.start();
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!request.finish(first));
        assert!(request.busy);
        assert!(request.finish(second));
        assert!(!request.finish(second));
    }
    #[test]
    fn late_translation_cannot_replace_new_source() {
        let mut services = ServicesUi::new(&Settings::default(), PathBuf::from("unused"));
        let (generation, _) = services.translation_request.start();
        services.open_translation("new source".into(), "new title".into());
        services
            .tx
            .send(Work::Translated {
                generation,
                target: "JA".into(),
                title: "old".into(),
                result: Ok(TranslationResult {
                    text: "stale".into(),
                    detected_source: None,
                }),
            })
            .unwrap();
        services.receive(&mut Vec::new());
        assert!(services.translation_output.is_none());
        assert_eq!(services.translation_source, "new source");
    }
    #[test]
    fn long_unicode_chapter_is_split_without_losing_text() {
        let source = format!(
            "{}\n{}\n{}",
            "日本語😀".repeat(1900),
            "第二段落。".repeat(2000),
            "終章".repeat(5000)
        );
        let chunks = translation_chunks(&source);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 10_000));
        assert_eq!(chunks.concat(), source);
        assert!(translation_chunks(" \n\t").is_empty());
    }
    #[test]
    fn paragraph_and_exact_limit_are_respected() {
        let source = format!("{}\n{}", "a".repeat(6000), "b".repeat(6000));
        let chunks = translation_chunks(&source);
        assert_eq!(chunks[0].len(), 6001);
        assert_eq!(chunks.concat(), source);
        assert_eq!(translation_chunks(&"あ".repeat(10_000)).len(), 1);
        let padded = format!("{}{}", "a".repeat(10_000), " ".repeat(10_001));
        assert_eq!(translation_chunks(&padded).concat(), padded);
    }

    #[test]
    fn malformed_language_codes_are_rejected_before_api_calls() {
        assert!(valid_language_code("JA"));
        assert!(valid_language_code("EN-US"));
        for value in ["", "---", "-JA", "JA-", "JA--JP", "日本語", "EN  "] {
            assert!(!valid_language_code(value));
        }
    }

    #[test]
    fn discarded_import_never_opens_a_book() {
        let mut services = ServicesUi::new(&Settings::default(), PathBuf::from("unused"));
        let (generation, _) = services.web_request.start();
        services.web_request.cancel();
        services
            .tx
            .send(Work::Imported {
                generation,
                result: Ok(WebImportResult {
                    path: PathBuf::from("unused/article.html"),
                    title: "article".into(),
                    source_url: "https://example.test/article".into(),
                    warnings: Vec::new(),
                    image_count: 0,
                }),
            })
            .unwrap();
        let mut actions = Vec::new();
        services.receive(&mut actions);
        assert!(actions.is_empty());
        assert!(services.web_message.is_none());
    }

    #[test]
    fn saved_translation_is_native_readable_and_does_not_overwrite() {
        let directory = std::env::temp_dir().join(format!(
            "yomi-services-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        fs::create_dir(&directory).unwrap();
        let first = write_translation(&directory, "翻訳の章", "こんにちは。", 1).unwrap();
        let second = write_translation(&directory, "翻訳の章", "別の訳文。", 1).unwrap();
        assert_ne!(first, second);
        assert!(first.starts_with(directory.join("translations")));
        let book = reader_core::open(&first).unwrap();
        assert!(book.chapters[0].plain_text().contains("こんにちは。"));
        assert!(!fs::read_to_string(&first).unwrap().contains("別の訳文。"));
        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
        fs::remove_dir(directory.join("translations")).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
