//! Explicitly invoked native network tools. Opening a dialog never starts HTTP.
use eframe::egui::{self, RichText, Ui};
use reader_library::{
    OpdsClient, OpdsConfig, OpdsPage, Settings, WebImportResult, import_web_page,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

pub enum ServiceAction {
    OpenBook(PathBuf),
    SettingsChanged,
    Notice(String),
}

type ServiceResult<T> = Result<T, String>;

/// Generations reject replies after a dialog is closed or reopened.
#[derive(Default)]
struct Request {
    generation: u64,
    busy: bool,
    error: Option<String>,
}
impl Request {
    fn start(&mut self) -> u64 {
        self.cancel();
        self.busy = true;
        self.error = None;
        self.generation
    }
    fn cancel(&mut self) {
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
}

impl ServicesUi {
    pub fn new(settings: &Settings, data_dir: PathBuf) -> Self {
        let catalog = settings.opds.clone().unwrap_or_default();
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
        }
    }
    pub fn open_catalog(&mut self) {
        self.catalog_open = true;
    }
    pub fn open_web_import(&mut self) {
        self.web_open = true;
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
        if self.catalog_request.busy || self.download_request.busy || self.web_request.busy {
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
        let generation = self.catalog_request.start();
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
        let generation = self.download_request.start();
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
        let generation = self.web_request.start();
        self.web_message = None;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = import_web_page(&url, &data_dir).map_err(|error| error.to_string());
            let _ = tx.send(Work::Imported { generation, result });
            ctx.request_repaint();
        });
    }
}

impl Drop for ServicesUi {
    fn drop(&mut self) {
        self.catalog_request.cancel();
        self.download_request.cancel();
        self.web_request.cancel();
    }
}
fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
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
            .on_hover_text("開始済みの通信は続く場合があります。")
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constructing_and_opening_dialogs_does_not_start_requests() {
        let mut services = ServicesUi::new(&Settings::default(), PathBuf::from("unused"));
        services.open_catalog();
        services.open_web_import();
        assert!(
            !services.catalog_request.busy
                && !services.download_request.busy
                && !services.web_request.busy
        );
        assert!(services.rx.try_recv().is_err());
    }
    #[test]
    fn stale_reply_cannot_finish_new_request() {
        let mut request = Request::default();
        let first = request.start();
        let second = request.start();
        assert!(!request.finish(first));
        assert!(request.busy);
        assert!(request.finish(second));
        assert!(!request.finish(second));
    }
    #[test]
    fn discarded_import_never_opens_a_book() {
        let mut services = ServicesUi::new(&Settings::default(), PathBuf::from("unused"));
        let generation = services.web_request.start();
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
}
