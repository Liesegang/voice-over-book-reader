use super::*;
use reader_library::BookSource;
use tempfile::TempDir;

#[test]
fn two_prose_panes_follow_only_the_primary_document_without_duplicating_the_page() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    fixture.app.settings.spread = reader_library::SpreadMode::Double;
    fixture.app.settings.text_spread = true;
    fixture.app.settings.tts.auto_follow = true;
    fixture.app.follow_playback = true;
    let hit = fixture.app.layout.pages[1].hits.first().unwrap().clone();
    fixture.app.spoken_range = Some((hit.locator, hit.end));
    for mode in [
        reader_library::ReadingMode::Horizontal,
        reader_library::ReadingMode::Vertical,
    ] {
        fixture.app.settings.reading_mode = mode;
        let plan = crate::reading_surface::chapter_spread(
            &fixture.app.book,
            0,
            vec2(1280., 760.),
            &fixture.app.settings,
        );
        assert_eq!(plan.columns, 2);
        assert!(
            plan.second.is_none(),
            "both panes show different pages of this prose chapter"
        );
        assert!(!crate::reading_surface::is_image_chapter(
            &fixture.app.book.chapters[0]
        ));
        assert_eq!(fixture.app.chromium_follow_target(0, 0), Some(hit.locator));
        assert_eq!(
            fixture.app.chromium_follow_target(1, 0),
            None,
            "locating both copies of one chapter would show the same page twice"
        );
        assert_eq!(fixture.app.chromium_follow_target(0, 1), None);
    }
    fixture.app.follow_playback = false;
    assert_eq!(fixture.app.chromium_follow_target(0, 0), None);
    fixture.app.follow_playback = true;
    fixture.app.settings.tts.auto_follow = false;
    assert_eq!(fixture.app.chromium_follow_target(0, 0), None);
    assert!(fixture.app.speech.is_none());
}

#[test]
fn retargeted_document_rejects_old_chapter_and_revision_navigation_events() {
    use reader_renderer_cef::{NavigatorEvent, Revision};
    for kind in ["located", "position", "pagination"] {
        let mut payload = serde_json::json!({
            "type":kind,"revision":12,"chapter":0,"page":1,"pages":3,"start":null,
            "request_id":31,"location":{"chapter":0,"node":8,"offset":3},
            "found":true,"reason":null,"page_starts":[null,null,null],
            "ready":{"fonts":true,"images":true},"writing_mode":"vertical-rl"
        });
        let old: NavigatorEvent = serde_json::from_value(payload.clone()).unwrap();
        assert!(!cef_reader::navigator_event_is_current(
            &old,
            Revision(13),
            1
        ));
        assert!(
            !cef_reader::navigator_event_is_current(&old, Revision(12), 1),
            "same-revision {kind} from a previous chapter must not move the reused pane"
        );
        payload["chapter"] = 1.into();
        payload["location"]["chapter"] = 1.into();
        let current: NavigatorEvent = serde_json::from_value(payload).unwrap();
        assert!(cef_reader::navigator_event_is_current(
            &current,
            Revision(12),
            1
        ));
    }
    let error: NavigatorEvent = serde_json::from_value(serde_json::json!({
        "type":"error","revision":12,"message":"Invalid fixed page viewport"
    }))
    .unwrap();
    assert!(!cef_reader::navigator_event_is_current(
        &error,
        Revision(13),
        1
    ));
    assert!(
        cef_reader::navigator_event_is_current(&error, Revision(12), 1),
        "current real renderer errors must not be hidden by stale-event filtering"
    );
}

#[test]
fn chromium_speech_boundaries_leave_page_authority_to_the_dom() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    fixture.app.follow_playback = true;
    fixture.app.settings.tts.auto_follow = true;
    fixture.app.page = 0;
    let boundaries = fixture
        .app
        .layout
        .pages
        .iter()
        .enumerate()
        .skip(2)
        .take(4)
        .map(|(page, content)| (page, content.hits.first().unwrap().clone()))
        .collect::<Vec<_>>();
    assert!(boundaries.len() >= 3);
    // A native hit/page number is not an acknowledgement from the Chromium
    // document, even when that sparse compatibility anchor matches the word.
    for (_, hit) in &boundaries {
        fixture.app.apply_spoken_range(hit.locator, hit.end, true);
        assert_eq!(
            fixture.app.page, 0,
            "speech must not overwrite the last DOM-confirmed page"
        );
        assert_eq!(fixture.app.resume_locator, Some(hit.locator));
        assert_eq!(fixture.app.spoken_range, Some((hit.locator, hit.end)));
    }
    let (native_page, hit) = boundaries.last().unwrap();
    fixture.app.apply_spoken_range(hit.locator, hit.end, false);
    assert_eq!(
        fixture.app.page, *native_page,
        "the native fallback keeps its own hit-based follow behavior"
    );
    assert!(fixture.app.speech.is_none());
}

#[test]
fn spoken_chapter_transition_keeps_the_latest_source_until_the_dom_is_ready() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let chapter = reader_core::parse_html(
        "<html><body><p>次の章を読み続けます。</p></body></html>",
        1,
        "next.xhtml",
        "Next",
    )
    .unwrap();
    let atoms = chapter.atoms();
    let first = atoms[0].locator_at(0).unwrap();
    let next = atoms[0].locator_at(3).unwrap();
    let end = atoms[0].locator_at(6).unwrap();
    fixture.app.book.chapters.push(chapter);
    fixture.app.follow_playback = true;
    fixture.app.settings.tts.auto_follow = true;
    fixture.app.apply_spoken_range(first, next, true);
    assert_eq!((fixture.app.chapter, fixture.app.page), (1, 0));
    assert!(fixture.app.layout.pages.is_empty());
    assert_eq!(fixture.app.pending_restore, Some(first));
    fixture.app.apply_spoken_range(next, end, true);
    assert_eq!(
        fixture.app.pending_restore,
        Some(next),
        "loading a chapter must not retain an obsolete spoken word"
    );
    assert_eq!(fixture.app.resume_locator, Some(next));
    assert_eq!(fixture.app.spoken_range, Some((next, end)));
    assert!(fixture.app.speech.is_none());
}

#[test]
fn disabled_speech_follow_does_not_change_the_readers_chapter_or_page() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let chapter = reader_core::parse_html(
        "<html><body><p>別の章です。</p></body></html>",
        1,
        "other.xhtml",
        "Other",
    )
    .unwrap();
    let atoms = chapter.atoms();
    let start = atoms[0].locator_at(0).unwrap();
    let end = atoms[0].locator_at(3).unwrap();
    fixture.app.book.chapters.push(chapter);
    let position = (fixture.app.chapter, fixture.app.page);
    for (auto, following) in [(false, true), (true, false)] {
        fixture.app.settings.tts.auto_follow = auto;
        fixture.app.follow_playback = following;
        fixture.app.apply_spoken_range(start, end, true);
        assert_eq!((fixture.app.chapter, fixture.app.page), position);
        assert_eq!(fixture.app.resume_locator, Some(start));
        assert!(fixture.app.pending_restore.is_none());
    }
}

#[test]
fn composed_reader_pages_reuse_idle_geometry_and_invalidate_on_navigation_and_resize() {
    let mut fixture = TestApp::new();
    let chapter = reader_core::parse_html(
        &format!(
            "<html><body>{}</body></html>",
            "<p>文字と<ruby>漢字<rt>かんじ</rt></ruby>の組版を確認します。</p>".repeat(160)
        ),
        0,
        "chapter.xhtml",
        "",
    )
    .unwrap();
    fixture.use_chapter(chapter);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    let extent = vec2(820., 640.);
    fixture.render_reader(extent);
    let first = Arc::clone(&fixture.app.composed_frame.as_ref().unwrap().page);
    let source = first.first_locator();
    fixture.render_reader(extent);
    assert!(Arc::ptr_eq(
        &first,
        &fixture.app.composed_frame.as_ref().unwrap().page
    ));
    assert_eq!(first.first_locator(), source);
    fixture.app.turn_page(true);
    fixture.render_reader(extent);
    let second = Arc::clone(&fixture.app.composed_frame.as_ref().unwrap().page);
    assert!(!Arc::ptr_eq(&first, &second));
    assert_ne!(first.first_locator(), second.first_locator());
    fixture.render_reader(vec2(950., 700.));
    assert!(!Arc::ptr_eq(
        &second,
        &fixture.app.composed_frame.as_ref().unwrap().page
    ));
    let resized = Arc::clone(&fixture.app.composed_frame.as_ref().unwrap().page);
    fixture.app.settings.font_size += 4.;
    fixture.render_reader(vec2(950., 700.));
    assert!(!Arc::ptr_eq(
        &resized,
        &fixture.app.composed_frame.as_ref().unwrap().page
    ));
    let resized_font = Arc::clone(&fixture.app.composed_frame.as_ref().unwrap().page);
    fixture.app.settings.reading_mode = reader_library::ReadingMode::Vertical;
    fixture.render_reader(vec2(950., 700.));
    assert!(!Arc::ptr_eq(
        &resized_font,
        &fixture.app.composed_frame.as_ref().unwrap().page
    ));
}

#[test]
#[ignore = "Manual native CPU frame timing; synthetic Japanese fixture, no model or book data"]
fn warm_reader_frame_timing_probe() {
    let mut fixture = TestApp::new();
    let chapter = reader_core::parse_html(
        &format!(
            "<html><body>{}</body></html>",
            "<p>文章の<ruby>漢字<rt>かんじ</rt></ruby>と<a href='#note'>参照</a>を表示します。</p>"
                .repeat(1600)
        ),
        0,
        "chapter.xhtml",
        "",
    )
    .unwrap();
    fixture.use_chapter(chapter);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    let extent = vec2(1280., 760.);
    fixture.render_reader(extent);
    while fixture.app.pagination_pending {
        let work = fixture.app.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        fixture.app.tx.send(work).unwrap();
        fixture.app.handle_work();
    }
    fixture.render_reader(extent);
    for rebuild in [true, false] {
        let start = Instant::now();
        for _ in 0..200 {
            if rebuild {
                fixture.app.composed_frame = None;
            }
            fixture.render_reader(extent);
        }
        eprintln!(
            "native-warm-frame forced_recomposition={rebuild} frames=200 elapsed_us={}",
            start.elapsed().as_micros()
        );
    }
}

/// All workers, state and book fixtures stay inside this directory. Startup
/// scans complete before a test runs, so no worker outlives its temporary files.
struct TestApp {
    app: ReaderApp,
    context: egui::Context,
    directory: TempDir,
}

impl TestApp {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let mut store = LibraryStore::open(&data).unwrap();
        let mut settings = Settings {
            calibre_path: Some(directory.path().join("no-calibre-library")),
            ..Settings::default()
        };
        settings.tts.pre_cache = false;
        store.save_settings(&settings).unwrap();
        drop(store);
        let context = egui::Context::default();
        let creation = eframe::CreationContext::_new_kittest(context.clone());
        let mut app = ReaderApp::new(
            &creation,
            &["--data-dir".into(), data.to_string_lossy().into_owned()],
        )
        .unwrap();
        let initial: Vec<_> = (0..2)
            .map(|_| {
                app.rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("isolated startup scans must finish")
            })
            .collect();
        for work in initial {
            app.tx.send(work).unwrap();
        }
        app.handle_work();
        app.notice = None;
        assert!(app.speech.is_none());
        Self {
            app,
            context,
            directory,
        }
    }

    fn source(&self, name: &str, text: &str) -> (Book, PathBuf) {
        let path = self.directory.path().join(name);
        std::fs::write(&path, text).unwrap();
        (reader_core::open(&path).unwrap(), path)
    }

    fn render_reader(&mut self, size: egui::Vec2) {
        let palette = Palette::for_settings(&self.app.settings);
        let mut output = self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| self.app.reader(ui, palette),
        );
        let painted = !output.shapes.is_empty();
        output.textures_delta.clear();
        assert!(painted, "native reader must paint the fixture");
        assert!(
            self.app.speech.is_none(),
            "rendering must not start a speech engine"
        );
    }

    fn use_chapter(&mut self, chapter: reader_core::Chapter) {
        self.app.book.id = "source-restore-fixture".into();
        self.app.book.chapters = vec![chapter];
        self.app.book.toc.clear();
        self.app.book.resources.clear();
        self.app.layout = BookLayout::default();
        self.app.layout_key.clear();
        self.app.chapter = 0;
        self.app.page = 0;
        self.app.selected = None;
        self.app.pending_restore = None;
        self.app.resume_locator = None;
    }

    /// Runs the real application surface, including foreground chrome and
    /// shortcuts. Advancing egui time prevents separate test clicks from being
    /// interpreted as a double click (which would launch the dictionary helper).
    fn frame(&mut self, size: egui::Vec2, events: Vec<egui::Event>) -> egui::FullOutput {
        let time = self.context.input(|input| input.time) + 1.0;
        let mut frame = eframe::Frame::_new_kittest();
        let mut output = self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| eframe::App::ui(&mut self.app, ui, &mut frame),
        );
        output.textures_delta.clear();
        assert!(
            self.app.speech.is_none(),
            "UI input must not synthesize audio"
        );
        output
    }

    fn reading_fixture(&mut self) {
        let chapter = reader_core::parse_html(
            &format!(
                "<html><body>{}</body></html>",
                "<p>日本語の本を読みます。<ruby>東京<rt>とうきょう</rt></ruby>へ向かいます。</p>"
                    .repeat(180)
            ),
            0,
            "interactions.xhtml",
            "",
        )
        .unwrap();
        self.use_chapter(chapter);
        self.app.screen = Screen::Reader;
        self.app.settings.spread = reader_library::SpreadMode::Single;
        self.app.focus = true;
        self.frame(vec2(1280.0, 760.0), vec![]);
        assert!(self.app.layout.pages.len() > 3);
        self.app.page = 1;
    }

    fn flush_tap(&mut self, size: egui::Vec2) {
        if let Some(tap) = &mut self.app.pending_center_tap {
            *tap = Instant::now() - Duration::from_secs(2);
        }
        if let Some((tap, _)) = &mut self.app.pending_page_turn {
            *tap = Instant::now() - Duration::from_secs(2);
        }
        self.frame(size, vec![]);
    }

    fn panel(
        &mut self,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        render: impl FnMut(&mut ReaderApp, &mut Ui, Palette),
    ) -> egui::FullOutput {
        self.panel_after(size, events, 1., render)
    }

    fn panel_after(
        &mut self,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        seconds: f64,
        mut render: impl FnMut(&mut ReaderApp, &mut Ui, Palette),
    ) -> egui::FullOutput {
        let palette = Palette::for_settings(&self.app.settings);
        palette.apply(&self.context, self.app.settings.theme == Theme::Dark);
        let time = self.context.input(|input| input.time) + seconds;
        let mut output = self.context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| render(&mut self.app, ui, palette),
        );
        output.textures_delta.clear();
        output
    }

    fn image_fixture(&mut self, count: usize) {
        let mut chapters: Vec<_> = (0..count)
            .map(|index| reader_core::parse_html(
                &format!("<html><body><img src='plate-{index}.svg' alt='挿絵{}' style='display:block;width:100%;height:100%'/></body></html>", index + 1),
                index as u32,
                &format!("plate-{index}.xhtml"),
                "",
            ).unwrap())
            .collect();
        for chapter in &mut chapters {
            chapter.presentation.layout = Some(reader_core::RenditionLayout::PrePaginated);
            chapter.presentation.viewport = Some([600., 900.]);
        }
        let first = chapters.remove(0);
        self.use_chapter(first);
        self.app.book.chapters.extend(chapters);
        for index in 0..count {
            self.app.book.resources.insert(
                format!("plate-{index}.svg"),
                b"<svg xmlns='http://www.w3.org/2000/svg' width='600' height='900'><rect width='600' height='900' fill='#2583eb'/></svg>".to_vec(),
            );
        }
        self.app.book.page_progression = reader_core::PageProgression::Rtl;
        self.app.screen = Screen::Reader;
        self.app.focus = true;
        self.app.settings.spread = reader_library::SpreadMode::Auto;
        self.app.settings.text_spread = false;
        self.frame(vec2(1280.0, 760.0), vec![]);
        assert_eq!(self.app.layout.pages.len(), 1);
    }

    fn drag(&mut self, size: egui::Vec2, from: Pos2, to: Pos2) -> [egui::FullOutput; 3] {
        self.frame(size, vec![egui::Event::PointerMoved(from)]);
        let pressed = self.frame(
            size,
            vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let moved = self.frame(size, vec![egui::Event::PointerMoved(to)]);
        let released = self.frame(
            size,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        [pressed, moved, released]
    }

    fn wheel_fixture(&mut self) -> egui::Vec2 {
        self.reading_fixture();
        let size = vec2(640.0, 420.0);
        self.frame(size, vec![]);
        self.app.page = 0;
        self.frame(size, vec![egui::Event::PointerMoved(size.to_pos2() * 0.5)]);
        assert!(self.app.layout.pages.len() > 8);
        assert!(self.app.pending_wheel_turns.is_empty());
        size
    }
}

fn click(point: Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(point),
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

fn wheel(unit: egui::MouseWheelUnit, y: f32) -> egui::Event {
    egui::Event::MouseWheel {
        unit,
        delta: vec2(0.0, y),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    }
}

fn text_bounds(output: &egui::FullOutput, prefix: &str) -> Rect {
    fn find(shape: &egui::Shape, prefix: &str) -> Option<Rect> {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text.starts_with(prefix) => {
                assert_eq!(text.galley.rows.len(), 1, "compact controls must not wrap");
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, prefix)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, prefix))
        .unwrap_or_else(|| panic!("UI did not paint text starting with {prefix:?}"))
}

fn paints_text(output: &egui::FullOutput, prefix: &str) -> bool {
    fn contains(shape: &egui::Shape, prefix: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.job.text.starts_with(prefix),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, prefix)),
            _ => false,
        }
    }
    output
        .shapes
        .iter()
        .any(|shape| contains(&shape.shape, prefix))
}

#[test]
fn latest_book_open_wins_even_when_an_older_worker_finishes_last() {
    let mut fixture = TestApp::new();
    let (older, old_path) = fixture.source("older.txt", "古い要求の本");
    let old_id = older.id.clone();
    let (latest, latest_path) = fixture.source("latest.txt", "新しい要求の本");
    let latest_id = latest.id.clone();
    fixture.app.open_generation = 12;
    fixture.app.busy = true;
    fixture
        .app
        .tx
        .send(Work::Opened {
            generation: 12,
            result: Ok((latest, latest_path.clone())),
        })
        .unwrap();
    fixture
        .app
        .tx
        .send(Work::Opened {
            generation: 11,
            result: Ok((older, old_path)),
        })
        .unwrap();
    fixture
        .app
        .tx
        .send(Work::Opened {
            generation: 11,
            result: Err("stale worker error".into()),
        })
        .unwrap();
    fixture.app.handle_work();
    assert_eq!(fixture.app.book.id, latest_id);
    assert_eq!(fixture.app.path.as_ref(), Some(&latest_path));
    assert!(!fixture.app.busy);
    assert!(
        !fixture
            .app
            .store
            .books()
            .iter()
            .any(|book| book.id == old_id)
    );
    assert!(
        !fixture
            .app
            .notice
            .as_ref()
            .is_some_and(|(message, _)| message.contains("stale worker"))
    );
}

#[test]
fn replacing_a_book_discards_old_geometry_selection_and_delayed_taps() {
    let mut fixture = TestApp::new();
    fixture.render_reader(vec2(400.0, 360.0));
    let old_geometry = Arc::downgrade(&fixture.app.composed_frame.as_ref().unwrap().page);
    let old_notes = Arc::downgrade(&fixture.app.annotations);
    let old_hit = fixture.app.layout.pages[0].hits[5].clone();
    fixture.app.selected = Some(old_hit.clone());
    fixture.app.selection_anchor = Some(old_hit.clone());
    fixture.app.pending_restore = Some(old_hit.locator);
    fixture.app.resume_locator = Some(old_hit.locator);
    fixture.app.pending_center_tap = Some(Instant::now());
    fixture.app.pending_book_seek = Some(0.7);
    fixture.app.pending_page_index = Some(12);
    fixture.app.drawer_open = true;
    fixture.app.drawer = Drawer::Search;
    fixture.app.jump_marks = vec![JumpMark {
        locator: old_hit.locator,
    }];
    fixture.app.pictures.insert("old-asset".into(), None);
    let (book, path) = fixture.source("replacement.txt", "次の本を最初から読みます。");
    fixture.app.open_generation = 4;
    fixture
        .app
        .tx
        .send(Work::Opened {
            generation: 4,
            result: Ok((book, path)),
        })
        .unwrap();
    fixture.app.handle_work();
    assert!(
        old_geometry.upgrade().is_none(),
        "book replacement releases cached geometry"
    );
    assert!(
        old_notes.upgrade().is_none(),
        "book replacement releases cached annotations"
    );
    assert!(fixture.app.layout.pages.is_empty());
    assert!(fixture.app.layout_key.is_empty());
    assert!(fixture.app.pictures.is_empty());
    assert!(fixture.app.selected.is_none());
    assert!(
        fixture.app.selection_anchor.is_none(),
        "an old drag anchor must not cross into the new book"
    );
    assert!(fixture.app.pending_restore.is_none());
    assert!(fixture.app.resume_locator.is_none());
    assert!(fixture.app.pending_center_tap.is_none());
    assert!(fixture.app.pending_book_seek.is_none());
    assert!(fixture.app.pending_page_index.is_none());
    assert!(
        !fixture.app.drawer_open,
        "a replacement book must not restore the old book's sidebar"
    );
    assert!(fixture.app.drawer == Drawer::Toc);
    assert!(
        fixture.app.jump_marks.is_empty(),
        "jump markers belong only to the book that created them"
    );
    fixture.render_reader(vec2(400.0, 360.0));
    assert_eq!(fixture.app.page, 0);
    assert!(
        fixture.app.layout.pages[0]
            .hits
            .iter()
            .all(|hit| hit.locator.chapter == 0)
    );
}

#[test]
fn restored_interior_ruby_and_image_anchor_use_the_real_containing_page() {
    let mut fixture = TestApp::new();
    let html = format!(
        "<html><body>{}<p><ruby>東京<rt>とうきょう</rt></ruby>へ。</p><img src='diagram.png' alt='図'/><p>図の後です。</p></body></html>",
        "<p>本文を読みます。</p>".repeat(35)
    );
    let chapter = reader_core::parse_html(&html, 0, "restore.xhtml", "").unwrap();
    let atoms = chapter.atoms();
    let ruby_index = atoms.iter().position(|atom| atom.ruby.is_some()).unwrap();
    let inside = atoms[ruby_index].locator_at("東".len()).unwrap();
    let image_block = chapter
        .blocks
        .iter()
        .find(|block| block.image.is_some())
        .unwrap();
    let image_anchor = image_block
        .image
        .as_ref()
        .unwrap()
        .source
        .unwrap()
        .start_locator();
    fixture.use_chapter(chapter);
    fixture.render_reader(vec2(320.0, 240.0));
    fixture.app.restore_locator(inside);
    assert!(
        fixture.app.selected.is_none(),
        "restoration is not a fabricated glyph selection"
    );
    fixture.render_reader(vec2(320.0, 240.0));
    assert!(fixture.app.page > 0);
    assert!(fixture.app.layout.pages[fixture.app.page].contains_locator(inside));
    let start = fixture.app.speech_start_hit().unwrap();
    assert_eq!(start.text, "東京");
    assert_eq!(start.unit_id, layout::unit_id(0, ruby_index));
    assert_eq!(start.offset, 0, "ruby reading remains atomic");
    fixture.app.restore_locator(image_anchor);
    fixture.render_reader(vec2(320.0, 240.0));
    assert!(fixture.app.layout.pages[fixture.app.page].contains_locator(image_anchor));
    assert!(fixture.app.selected.is_none());
    assert!(fixture.app.pending_restore.is_none());
}

#[test]
fn default_narration_start_tracks_the_visible_page_and_exact_restored_hit() {
    let mut fixture = TestApp::new();
    let chapter = reader_core::parse_html(
        &format!(
            "<html><body>{}</body></html>",
            "<p>日本語の本を読みます。</p>".repeat(35)
        ),
        0,
        "speech-start.xhtml",
        "",
    )
    .unwrap();
    fixture.use_chapter(chapter);
    fixture.render_reader(vec2(320.0, 240.0));
    fixture.app.turn_page(true);
    fixture.render_reader(vec2(320.0, 240.0));
    let expected = fixture.app.layout.pages[fixture.app.page]
        .hits
        .first()
        .unwrap()
        .clone();
    assert_ne!(fixture.app.page, 0);
    let start = fixture.app.speech_start_hit().unwrap();
    assert_eq!(
        (start.locator, start.unit_id, start.offset),
        (expected.locator, expected.unit_id, expected.offset)
    );
    let exact = fixture.app.layout.pages[fixture.app.page]
        .hits
        .iter()
        .find(|hit| hit.locator > expected.locator)
        .unwrap()
        .clone();
    fixture.app.restore_locator(exact.locator);
    fixture.render_reader(vec2(320.0, 240.0));
    let start = fixture.app.speech_start_hit().unwrap();
    assert_eq!(
        (start.locator, start.unit_id, start.offset),
        (exact.locator, exact.unit_id, exact.offset)
    );
    assert!(fixture.app.speech.is_none());
}

#[test]
fn calibre_uuid_rename_keeps_local_identity_position_and_reopen_target() {
    let mut fixture = TestApp::new();
    let old_path = fixture.directory.path().join("old.epub");
    let new_path = fixture.directory.path().join("renamed.epub");
    std::fs::write(&old_path, b"read-only old source").unwrap();
    std::fs::write(&new_path, b"read-only renamed source").unwrap();
    let local_id = "local-document-identity";
    let old = BookEntry {
        id: local_id.into(),
        uuid: Some("calibre-book-uuid".into()),
        calibre_id: Some(42),
        title: "以前の題名".into(),
        authors: vec!["著者".into()],
        tags: vec![],
        epub_path: old_path.clone(),
        cover_path: None,
        source: BookSource::Calibre,
        added_at: 1,
    };
    fixture.app.store.save_book(&old).unwrap();
    fixture.app.shelf = vec![old.clone()];
    let locator = fixture.app.book.chapters[0].atoms()[3]
        .locator_at(0)
        .unwrap();
    fixture
        .app
        .store
        .save_position(&ReadingPosition {
            book_id: local_id.into(),
            locator,
            progress: 0.4,
            updated_at: 1,
        })
        .unwrap();
    let renamed = BookEntry {
        id: "calibre:library:calibre-book-uuid".into(),
        title: "変更後の題名".into(),
        epub_path: new_path.clone(),
        ..old
    };
    fixture.app.tx.send(Work::Shelf(Ok(vec![renamed]))).unwrap();
    fixture.app.handle_work();
    assert_eq!(fixture.app.shelf.len(), 1);
    assert_eq!(fixture.app.shelf[0].id, local_id);
    assert_eq!(fixture.app.shelf[0].epub_path, new_path);
    assert_eq!(fixture.app.store.books()[0].epub_path, new_path);
    assert_eq!(
        fixture
            .app
            .store
            .position(local_id)
            .unwrap()
            .unwrap()
            .locator,
        locator
    );
    let mut document = Book::demo();
    document.id = "different-current-content-hash".into();
    fixture.app.open_generation = 5;
    fixture
        .app
        .tx
        .send(Work::Opened {
            generation: 5,
            result: Ok((document, new_path.clone())),
        })
        .unwrap();
    fixture.app.handle_work();
    assert_eq!(fixture.app.book.id, local_id);
    assert_eq!(fixture.app.path.as_ref(), Some(&new_path));
    assert_eq!(fixture.app.pending_restore, Some(locator));
    assert_eq!(std::fs::read(old_path).unwrap(), b"read-only old source");
    assert_eq!(
        std::fs::read(new_path).unwrap(),
        b"read-only renamed source"
    );
}

#[test]
fn cli_book_argument_never_treats_option_values_as_books() {
    let directory = tempfile::tempdir().unwrap();
    let screenshot = directory.path().join("existing.png");
    let book = directory.path().join("book.epub");
    std::fs::write(&screenshot, b"existing screenshot").unwrap();
    std::fs::write(&book, b"book fixture").unwrap();
    let options = vec![
        "--demo".into(),
        "--data-dir".into(),
        directory.path().to_string_lossy().into_owned(),
        "--screenshot".into(),
        screenshot.to_string_lossy().into_owned(),
    ];
    assert_eq!(book_argument(&options), None);
    let mut args = options;
    args.push(book.to_string_lossy().into_owned());
    assert_eq!(book_argument(&args), Some(book));
    assert_eq!(book_argument(&["--screenshot".into()]), None);
}

#[test]
fn minimum_window_settings_render_all_tabs_without_horizontal_overflow() {
    let mut fixture = TestApp::new();
    fixture.app.screen = Screen::Settings;
    for theme in Theme::ALL {
        fixture.app.settings.theme = theme;
        let palette = Palette::for_settings(&fixture.app.settings);
        palette.apply(&fixture.context, theme == Theme::Dark);
        for tab in [
            SettingTab::Appearance,
            SettingTab::Reading,
            SettingTab::Speech,
            SettingTab::Dictionaries,
            SettingTab::Library,
            SettingTab::Keys,
        ] {
            fixture.app.setting_tab = tab;
            let mut right = 0.0_f32;
            let mut output = fixture.context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(820.0, 580.0))),
                    ..Default::default()
                },
                |ui| {
                    fixture.app.settings_view(ui, palette);
                    right = ui.min_rect().right();
                },
            );
            let painted = !output.shapes.is_empty();
            output.textures_delta.clear();
            assert!(painted);
            assert!(
                right <= 821.0,
                "settings exceeded minimum window width: {right}"
            );
        }
    }
    assert!(fixture.app.speech.is_none());
}

#[test]
fn center_taps_toggle_overlay_without_reflow_even_with_an_open_drawer() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280.0, 760.0);
    let extent = fixture.app.reading_extent;
    let key = fixture.app.layout_key.clone();
    let locator = fixture.app.layout.pages[fixture.app.page].first_locator();
    let geometry: Vec<_> = fixture.app.layout.pages[fixture.app.page]
        .glyphs
        .iter()
        .map(|glyph| (glyph.origin, glyph.size, glyph.id, glyph.rotated))
        .collect();
    fixture.frame(size, click(pos2(640.0, 380.0)));
    assert!(fixture.app.pending_center_tap.is_some());
    fixture.flush_tap(size);
    assert!(!fixture.app.focus, "center click reveals reading controls");
    fixture.app.drawer_open = true;
    fixture.frame(size, vec![]);
    fixture.frame(size, click(pos2(640.0, 380.0)));
    assert!(
        fixture.app.pending_center_tap.is_some(),
        "drawer must not swallow a click outside its bounds"
    );
    fixture.flush_tap(size);
    assert!(fixture.app.focus);
    assert!(
        fixture.app.drawer_open,
        "hidden chrome preserves the sidebar choice"
    );
    assert_eq!(fixture.app.reading_extent, extent);
    assert_eq!(fixture.app.layout_key, key);
    assert_eq!(
        fixture.app.layout.pages[fixture.app.page].first_locator(),
        locator
    );
    assert_eq!(
        fixture.app.layout.pages[fixture.app.page]
            .glyphs
            .iter()
            .map(|glyph| (glyph.origin, glyph.size, glyph.id, glyph.rotated))
            .collect::<Vec<_>>(),
        geometry,
        "toggling reading chrome must not change glyph positions"
    );
}

#[test]
fn center_hide_and_show_restores_each_sidebar_without_painting_it_while_hidden() {
    use crate::actions::Action;
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280., 760.);
    let loc = fixture.app.layout.pages[1].first_locator().unwrap();
    fixture.app.book.toc = vec![reader_core::TocEntry {
        target: None,
        title: "復元する目次".into(),
        chapter: 0,
        fragment: None,
        children: vec![],
    }];
    fixture
        .app
        .store
        .save_bookmark(&Bookmark::new(
            &fixture.app.book.id,
            loc,
            "復元するブックマーク",
        ))
        .unwrap();
    fixture
        .app
        .store
        .save_annotation(&reader_library::Annotation {
            id: "restore-sidebar-note".into(),
            book_id: fixture.app.book.id.clone(),
            start: loc,
            end: loc,
            quote: "復元する注釈".into(),
            note: "保持したメモ".into(),
            color: [43, 112, 235],
            created_at: 0,
        })
        .unwrap();
    fixture.app.search_results = vec![reader_core::SearchHit {
        chapter: 0,
        block: 0,
        start: loc,
        end: loc,
        snippet: "復元する検索結果".into(),
    }];
    for (action, tab, label) in [
        (Action::Toc, Drawer::Toc, "復元する目次"),
        (Action::Bookmarks, Drawer::Bookmarks, "復元するブックマーク"),
        (Action::Notes, Drawer::Notes, "復元する注釈"),
        (Action::Search, Drawer::Search, "復元する検索結果"),
    ] {
        fixture.app.execute_action(action, &fixture.context);
        fixture.frame(size, vec![]);
        assert!(paints_text(&fixture.frame(size, vec![]), label));
        fixture.frame(size, click(pos2(640., 380.)));
        fixture.flush_tap(size);
        assert!(fixture.app.focus);
        assert!(fixture.app.drawer_open && fixture.app.drawer == tab);
        assert!(
            !paints_text(&fixture.frame(size, vec![]), label),
            "sidebar must not be painted under hidden chrome"
        );
        fixture.frame(size, click(pos2(640., 380.)));
        fixture.flush_tap(size);
        assert!(!fixture.app.focus);
        assert!(fixture.app.drawer_open && fixture.app.drawer == tab);
        assert!(
            paints_text(&fixture.frame(size, vec![]), label),
            "show restores the same sidebar contents"
        );
    }
}

#[test]
fn chrome_shortcut_preserves_sidebar_but_escape_and_explicit_close_do_not_restore_it() {
    use crate::actions::Action;
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280., 760.);
    fixture
        .app
        .settings
        .key_bindings
        .insert(Action::ToggleChrome.id().into(), "F8".into());
    fixture
        .app
        .execute_action(Action::Bookmarks, &fixture.context);
    fixture.frame(size, key(egui::Key::F8, egui::Modifiers::NONE));
    assert!(fixture.app.focus && fixture.app.drawer_open);
    fixture.frame(size, key(egui::Key::F8, egui::Modifiers::NONE));
    assert!(!fixture.app.focus && fixture.app.drawer_open);
    assert!(fixture.app.drawer == Drawer::Bookmarks);
    fixture.frame(size, key(egui::Key::Escape, egui::Modifiers::NONE));
    assert!(fixture.app.focus && !fixture.app.drawer_open);
    fixture.frame(size, key(egui::Key::F8, egui::Modifiers::NONE));
    assert!(
        !fixture.app.focus && !fixture.app.drawer_open,
        "Escape explicitly closes the sidebar"
    );
    fixture.app.execute_action(Action::Toc, &fixture.context);
    fixture.panel(vec2(1280., 52.), vec![], ReaderApp::top_bar);
    fixture.panel(vec2(1280., 52.), click(pos2(18., 26.)), ReaderApp::top_bar);
    assert!(
        !fixture.app.drawer_open,
        "menu button explicitly closes the sidebar"
    );
    fixture
        .app
        .execute_action(Action::ToggleChrome, &fixture.context);
    fixture
        .app
        .execute_action(Action::ToggleChrome, &fixture.context);
    assert!(
        !fixture.app.drawer_open,
        "explicitly closed sidebars stay closed on chrome re-show"
    );
}

#[test]
fn broad_page_click_zones_and_arrows_follow_writing_direction_with_drawer_open() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280.0, 760.0);
    for (mode, left_is_next) in [
        (reader_library::ReadingMode::Vertical, true),
        (reader_library::ReadingMode::Horizontal, false),
    ] {
        fixture.app.settings.reading_mode = mode;
        fixture.app.focus = false;
        fixture.app.drawer_open = true;
        fixture.frame(size, vec![]);
        fixture.frame(size, vec![]);
        for (x, forward) in [(307.0, left_is_next), (973.0, !left_is_next)] {
            fixture.app.page = 1;
            fixture.frame(size, click(pos2(x, 380.0)));
            assert_eq!(
                fixture.app.page,
                if forward { 2 } else { 0 },
                "wide page click outside the drawer turns immediately for {mode:?}, x={x}"
            );
            assert!(
                fixture.app.pending_page_turn.is_none(),
                "page clicks must not wait for the double-click timeout"
            );
        }
        for (arrow, forward) in [
            (egui::Key::ArrowLeft, left_is_next),
            (egui::Key::ArrowRight, !left_is_next),
        ] {
            fixture.app.page = 1;
            fixture.frame(size, key(arrow, egui::Modifiers::NONE));
            assert_eq!(
                fixture.app.page,
                if forward { 2 } else { 0 },
                "physical arrows must follow {mode:?}"
            );
        }
    }
}

#[test]
fn key_capture_overrides_defaults_and_dispatches_custom_shelves_without_f11_default() {
    use crate::actions::{self, Action};
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280.0, 760.0);
    fixture.frame(size, key(egui::Key::F11, egui::Modifiers::NONE));
    assert!(fixture.app.focus, "F11 is unassigned by default");
    fixture.app.binding_capture = Some(Action::ToggleChrome.id().into());
    fixture.frame(size, key(egui::Key::F11, egui::Modifiers::NONE));
    assert!(fixture.app.binding_capture.is_none());
    assert!(
        fixture.app.focus,
        "a captured key must not execute its action immediately"
    );
    assert_eq!(
        actions::binding(Action::ToggleChrome, &fixture.app.settings.key_bindings),
        "F11"
    );
    fixture.frame(size, key(egui::Key::F11, egui::Modifiers::NONE));
    assert!(!fixture.app.focus, "a user may explicitly assign F11");
    fixture.app.binding_capture = Some(Action::NextPage.id().into());
    fixture.frame(size, key(egui::Key::ArrowLeft, egui::Modifiers::NONE));
    assert_eq!(
        actions::binding(Action::PageLeft, &fixture.app.settings.key_bindings),
        ""
    );
    fixture.app.settings.reading_mode = reader_library::ReadingMode::Horizontal;
    fixture.frame(size, vec![]);
    fixture.app.page = 1;
    fixture.frame(size, key(egui::Key::ArrowLeft, egui::Modifiers::NONE));
    assert_eq!(
        fixture.app.page, 2,
        "logical next-page assignment replaces the physical left-arrow default"
    );
    let shelf = "日本語の本";
    fixture
        .app
        .settings
        .custom_shelves
        .insert(shelf.into(), vec![fixture.app.book.id.clone()]);
    fixture.app.binding_capture = Some(format!("Shelf:{shelf}"));
    fixture.frame(size, key(egui::Key::J, egui::Modifiers::ALT));
    assert!(
        fixture.app.screen != Screen::Library,
        "capture must not switch windows"
    );
    fixture.frame(size, key(egui::Key::J, egui::Modifiers::ALT));
    assert!(fixture.app.screen == Screen::Library);
    assert_eq!(fixture.app.selected_shelf.as_deref(), Some(shelf));
    let reopened = LibraryStore::open(fixture.app.store.data_dir()).unwrap();
    assert_eq!(
        reopened.settings().key_bindings[&format!("Shelf:{shelf}")],
        "Alt+J"
    );
    assert_eq!(reopened.settings().key_bindings[Action::PageLeft.id()], "");
}

#[test]
fn long_bookmark_titles_keep_delete_in_the_same_compact_row() {
    let mut fixture = TestApp::new();
    fixture.app.drawer = Drawer::Bookmarks;
    let first = Bookmark::new(
        &fixture.app.book.id,
        Locator::default(),
        format!("長い見出し{}", "あいうえお".repeat(60)),
    );
    let second = Bookmark::new(&fixture.app.book.id, Locator::default(), "残すブックマーク");
    fixture.app.store.save_bookmark(&first).unwrap();
    fixture.app.store.save_bookmark(&second).unwrap();
    // The actual drawer leaves 272 points after its outer padding. Long labels
    // must truncate there while keeping a 36-point delete target on the right.
    let size = vec2(272.0, 400.0);
    fixture.panel(size, vec![], ReaderApp::drawer);
    let output = fixture.panel(size, vec![], ReaderApp::drawer);
    let title = text_bounds(&output, "長い見出し");
    let next = text_bounds(&output, "残すブックマーク");
    assert!(
        (title.left() - next.left()).abs() < 1.0,
        "bookmark labels share a left edge regardless of title length"
    );
    assert!(
        (next.center().y - title.center().y) <= 44.0,
        "bookmark rows should stay compact"
    );
    fixture.panel(
        size,
        click(pos2(254.0, title.center().y)),
        ReaderApp::drawer,
    );
    let remaining = fixture.app.store.bookmarks(&fixture.app.book.id).unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "delete remains clickable beside a long title"
    );
    assert_eq!(remaining[0].id, second.id);
    assert!(
        fixture.app.pending_restore.is_none(),
        "delete must not navigate to the bookmark"
    );
}

#[test]
fn editing_voice_instruction_releases_fixed_reference_without_synthesizing() {
    let mut fixture = TestApp::new();
    fixture.app.speech_provider = "irodori".into();
    fixture.app.speech_instruction = "穏やかな声".into();
    fixture.app.speech_reference = Some(fixture.directory.path().join("accepted-sample.wav"));
    let size = vec2(680.0, 760.0);
    fixture.panel(size, vec![], ReaderApp::speech_settings);
    let output = fixture.panel(size, vec![], ReaderApp::speech_settings);
    let field = text_bounds(&output, "穏やかな声");
    assert!(fixture.app.speech_reference.is_some());
    fixture.panel(size, click(field.center()), ReaderApp::speech_settings);
    fixture.panel(
        size,
        vec![egui::Event::Text("明るい".into())],
        ReaderApp::speech_settings,
    );
    assert!(fixture.app.speech_instruction.contains("明るい"));
    assert!(
        fixture.app.speech_reference.is_none(),
        "a changed voice instruction must not keep conditioning on the previous fixed sample"
    );
    let ProviderConfig::Irodori {
        reference_audio,
        voice_instruction,
        ..
    } = fixture.app.provider_config()
    else {
        panic!("voice settings unexpectedly changed provider");
    };
    assert!(reference_audio.is_none());
    assert!(voice_instruction.contains("明るい"));
    assert!(
        fixture.app.speech.is_none(),
        "editing settings must not initialize a synthesizer"
    );
}

#[test]
fn settings_switch_labels_are_left_aligned_and_toggle_from_label_or_keyboard() {
    let mut fixture = TestApp::new();
    let mut enabled = false;
    let mut row = Rect::NOTHING;
    let mut changed = false;
    let size = vec2(540.0, 100.0);
    let output = fixture.panel(size, vec![], |_, ui, _| {
        let response = crate::settings_controls::check(ui, &mut enabled, "ルビを表示する");
        row = response.rect;
    });
    let label = text_bounds(&output, "ルビを表示する");
    assert!(
        (label.left() - row.left()).abs() < 1.0,
        "setting labels stay at the left edge"
    );
    let thumb = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Circle(circle) if (circle.radius - 7.0).abs() < 0.1 => Some(circle.center),
            _ => None,
        })
        .expect("switch must paint its thumb");
    assert!(
        thumb.x >= row.right() - 32.0 && thumb.x <= row.right() - 10.0,
        "switch stays at the right edge"
    );
    assert_eq!(row.width(), size.x, "the whole setting row is clickable");
    let output = fixture.panel(size, click(label.center()), |_, ui, _| {
        changed = crate::settings_controls::check(ui, &mut enabled, "ルビを表示する").changed();
    });
    assert!(enabled && changed, "the label activates its switch");
    assert!(
        output.platform_output.events.iter().any(|event| matches!(
            event,
            egui::output::OutputEvent::ValueChanged(info) | egui::output::OutputEvent::Clicked(info)
                if info.role == egui::Role::Switch
        )),
        "assistive technology must receive a switch change"
    );
    fixture.panel(
        size,
        key(egui::Key::Space, egui::Modifiers::NONE),
        |_, ui, _| {
            crate::settings_controls::check(ui, &mut enabled, "ルビを表示する");
        },
    );
    assert!(!enabled, "a focused switch is keyboard accessible");
    fixture.panel(
        size,
        click(pos2(row.right() - 20.0, row.center().y)),
        |_, ui, _| {
            crate::settings_controls::check(ui, &mut enabled, "ルビを表示する");
        },
    );
    assert!(enabled, "the switch at the right edge is also clickable");
    fixture.panel(size, click(label.center()), |_, ui, _| {
        ui.add_enabled_ui(false, |ui| {
            crate::settings_controls::check(ui, &mut enabled, "ルビを表示する");
        });
    });
    assert!(enabled, "a disabled switch cannot change");
}

#[test]
fn shortcut_rows_stay_compact_and_right_aligned_at_narrow_widths() {
    use crate::actions::Action;
    let mut fixture = TestApp::new();
    for width in [320.0, 542.0, 894.0] {
        fixture.app.binding_capture = None;
        for (action, binding) in [
            (Action::Open, "Primary+Shift+Alt+O"),
            (Action::Library, "Primary+Shift+Alt+L"),
            (Action::CloseWindow, "Primary+Shift+Alt+W"),
        ] {
            fixture
                .app
                .settings
                .key_bindings
                .insert(action.id().into(), binding.into());
        }
        let size = vec2(width, 760.0);
        fixture.panel(size, vec![], ReaderApp::key_settings);
        let output = fixture.panel(size, vec![], ReaderApp::key_settings);
        let primary = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl"
        };
        let first = text_bounds(&output, &format!("{primary}+Shift+Alt+O"));
        let second = text_bounds(&output, &format!("{primary}+Shift+Alt+L"));
        let third = text_bounds(&output, &format!("{primary}+Shift+Alt+W"));
        let label_left = text_bounds(&output, Action::Open.label()).left();
        assert!(
            (label_left - 16.).abs() < 1.0,
            "action names share the section's inset content edge"
        );
        for label in [Action::Library.label(), Action::CloseWindow.label()] {
            assert!(
                (text_bounds(&output, label).left() - label_left).abs() < 1.0,
                "label {label:?} must not be centered within its available width"
            );
        }
        assert!(
            text_bounds(&output, "操作を検索").left().abs() < 1.,
            "the search label stays at the top-level content edge"
        );
        assert!((second.center().y - first.center().y - 48.0).abs() < 1.0);
        assert!((third.center().y - second.center().y - 48.0).abs() < 1.0);
        assert!((second.center().x - first.center().x).abs() < 1.0);
        assert!(first.left() >= width - 244.0 && first.right() <= width - 96.0);
        fixture.panel(
            size,
            click(pos2(width - 171.0, first.center().y)),
            ReaderApp::key_settings,
        );
        assert_eq!(
            fixture.app.binding_capture.as_deref(),
            Some(Action::Open.id())
        );
        let output = fixture.panel(size, vec![], ReaderApp::key_settings);
        let capturing = text_bounds(&output, "キーを押してください");
        assert!(
            (capturing.center().y - first.center().y).abs() < 1.0,
            "capture must not move its row"
        );
        assert!(capturing.right() <= width - 96.0);
        let cancel = text_bounds(&output, "取消");
        fixture.panel(size, click(cancel.center()), ReaderApp::key_settings);
        assert!(fixture.app.binding_capture.is_none());
        let output = fixture.panel(size, vec![], ReaderApp::key_settings);
        let clear = text_bounds(&output, "×");
        assert!(
            (clear.center().y - first.center().y).abs() < 1.0,
            "clear must remain on its action row: {width}, first={first:?}, clear={clear:?}"
        );
        assert!((clear.center().x - (width - 75.0)).abs() < 1.0);
        fixture.panel(size, click(clear.center()), ReaderApp::key_settings);
        assert_eq!(fixture.app.settings.key_bindings[Action::Open.id()], "");
        assert_eq!(
            fixture.app.settings.key_bindings[Action::Library.id()],
            "Primary+Shift+Alt+L"
        );
        fixture.panel(
            size,
            click(pos2(width - 26.0, first.center().y)),
            ReaderApp::key_settings,
        );
        assert!(
            !fixture
                .app
                .settings
                .key_bindings
                .contains_key(Action::Open.id())
        );
    }
}

#[test]
fn prose_uses_the_whole_surface_until_text_spread_is_enabled() {
    let mut fixture = TestApp::new();
    let text = "中央に隙間を空けず、文章を続けて読みます。".repeat(300);
    fixture.use_chapter(
        reader_core::parse_html(&format!("<p>{text}</p>"), 0, "prose.xhtml", "").unwrap(),
    );
    fixture.app.screen = Screen::Reader;
    fixture.app.settings.reading_mode = reader_library::ReadingMode::Horizontal;
    fixture.app.settings.spread = reader_library::SpreadMode::Auto;
    let size = vec2(1280.0, 760.0);
    fixture.frame(size, vec![]);
    assert!(!fixture.app.settings.text_spread);
    assert_eq!(fixture.app.spread_pages, 1);
    assert!(!fixture.app.cross_chapter_spread);
    let full_key = fixture.app.layout_key.clone();
    let anchor = fixture.app.layout.pages[0].first_locator();
    assert!(
        fixture.app.layout.pages[0]
            .hits
            .iter()
            .any(|hit| hit.rect.center().x > 1000.0)
    );
    fixture.app.settings.text_spread = true;
    fixture.frame(size, vec![]);
    assert_eq!(fixture.app.spread_pages, 2);
    assert_ne!(
        fixture.app.layout_key, full_key,
        "changing text spread must invalidate pagination"
    );
    assert!(
        fixture.app.layout.pages[0]
            .hits
            .iter()
            .all(|hit| hit.rect.right() < size.x * 0.5)
    );
    fixture.app.settings.text_spread = false;
    fixture.frame(size, vec![]);
    assert_eq!(fixture.app.spread_pages, 1);
    assert_eq!(fixture.app.layout_key, full_key);
    assert_eq!(fixture.app.layout.pages[0].first_locator(), anchor);
}

#[test]
fn image_spreads_cross_spine_chapters_without_repeating_the_odd_or_even_end() {
    let mut fixture = TestApp::new();
    let size = vec2(1280.0, 760.0);
    for count in [3, 4] {
        fixture.image_fixture(count);
        assert!(fixture.app.cross_chapter_spread);
        assert_eq!(fixture.app.spread_pages, 2);
        assert_eq!(fixture.app.sibling_layout.pages[0].pictures.len(), 1);
        assert_eq!(
            fixture.app.sibling_layout.pages[0]
                .first_locator()
                .unwrap()
                .chapter,
            1
        );
        fixture.app.turn_page(true);
        fixture.frame(size, vec![]);
        assert_eq!(
            fixture.app.chapter, 2,
            "the next spread starts after both visible images"
        );
        assert_eq!(
            fixture.app.layout.pages[0].first_locator().unwrap().chapter,
            2
        );
        if count == 4 {
            assert_eq!(
                fixture.app.sibling_layout.pages[0]
                    .first_locator()
                    .unwrap()
                    .chapter,
                3
            );
        } else {
            assert_ne!(
                fixture.app.sibling_key,
                format!("{}:{}", fixture.app.layout_key, 3),
                "a cached previous image must not fill an odd last spread"
            );
        }
        fixture.app.turn_page(true);
        fixture.frame(size, vec![]);
        assert_eq!(
            fixture.app.chapter, 2,
            "at the end, do not clamp to an overlapping spread"
        );
        fixture.app.turn_page(false);
        fixture.frame(size, vec![]);
        assert_eq!(fixture.app.chapter, 0);
        fixture.app.turn_page(false);
        fixture.frame(size, vec![]);
        assert_eq!(fixture.app.chapter, 0);
        fixture.app.settings.spread = reader_library::SpreadMode::Single;
        fixture.frame(size, vec![]);
        assert!(!fixture.app.cross_chapter_spread);
        fixture.app.turn_page(true);
        fixture.frame(size, vec![]);
        assert_eq!(
            fixture.app.chapter, 1,
            "single-image mode advances by one chapter"
        );
    }
}

#[test]
fn several_images_in_one_spine_chapter_are_not_skipped_by_cross_chapter_pairing() {
    let mut fixture = TestApp::new();
    let size = vec2(1280.0, 760.0);
    for count in [2, 3] {
        fixture.image_fixture(2);
        fixture.app.book.chapters[0] = reader_core::parse_html(
            &format!("<html><body>{}</body></html>", (0..count).map(|index| format!("<img src='plate-{}.svg' alt='同じ章の画像{index}' style='display:block;width:100%;height:100%'/>", index % 2)).collect::<String>()),
            0,
            "plates.xhtml",
            "",
        ).unwrap();
        fixture.app.layout_key.clear();
        fixture.app.layout = BookLayout::default();
        fixture.app.page = 0;
        fixture.frame(size, vec![]);
        assert_eq!(fixture.app.layout.pages.len(), count);
        assert_eq!(fixture.app.spread_pages, 2);
        assert!(
            !fixture.app.cross_chapter_spread,
            "a chapter containing several native pages must show its own consecutive images"
        );
        fixture.app.turn_page(true);
        fixture.frame(size, vec![]);
        if count == 3 {
            assert_eq!((fixture.app.chapter, fixture.app.page), (0, 2));
            fixture.app.turn_page(true);
            fixture.frame(size, vec![]);
        }
        assert_eq!(
            fixture.app.chapter, 1,
            "all images in the first chapter must be visited before moving on"
        );
    }
}

#[test]
fn top_and_bottom_resize_edges_work_over_title_drawer_and_reading_bar_overlays() {
    use egui::viewport::ResizeDirection;
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280.0, 760.0);
    fixture.app.focus = false;
    fixture.app.drawer_open = true;
    fixture.frame(size, vec![]);
    fixture.frame(size, vec![]);
    for (point, delta, expected) in [
        (pos2(120.0, 2.0), vec2(0.0, 16.0), ResizeDirection::North),
        (pos2(120.0, 758.0), vec2(0.0, -16.0), ResizeDirection::South),
    ] {
        fixture.frame(size, vec![egui::Event::PointerMoved(point)]);
        let pressed = fixture.frame(
            size,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let dragged = fixture.frame(size, vec![egui::Event::PointerMoved(point + delta)]);
        assert!([&pressed, &dragged].into_iter().flat_map(|output| output.viewport_output.values()).flat_map(|output| &output.commands).any(|command| matches!(command, egui::ViewportCommand::BeginResize(direction) if *direction == expected)), "edge {expected:?} must reach the native resize handler despite reading overlays: pressed={:?}, dragged={:?}", pressed.viewport_output.values().map(|output| &output.commands).collect::<Vec<_>>(), dragged.viewport_output.values().map(|output| &output.commands).collect::<Vec<_>>());
        fixture.frame(
            size,
            vec![egui::Event::PointerButton {
                pos: point + delta,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(fixture.app.pending_center_tap.is_none());
        assert!(fixture.app.pending_page_turn.is_none());
    }
}

#[test]
fn full_width_seek_slider_and_centered_arrow_buttons_follow_book_direction() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(4);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    let size = vec2(1280.0, 760.0);
    let bar = vec2(1280.0, 100.0);
    for (mode, rtl) in [
        (reader_library::ReadingMode::Vertical, true),
        (reader_library::ReadingMode::Horizontal, false),
    ] {
        fixture.app.settings.reading_mode = mode;
        for (x, expected) in [
            (1250.0, if rtl { 0 } else { 3 }),
            (124.0, if rtl { 3 } else { 0 }),
        ] {
            fixture.frame(size, vec![]);
            fixture.app.jump_marks.clear();
            fixture.panel(bar, vec![], ReaderApp::reading_seek);
            fixture.panel(bar, vec![], ReaderApp::reading_seek);
            let point = pos2(x, READING_FOOTER_HEIGHT * 0.5);
            fixture.panel(
                bar,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ReaderApp::reading_seek,
            );
            fixture.panel(
                bar,
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ReaderApp::reading_seek,
            );
            assert_eq!(
                fixture.app.chapter,
                expected,
                "the whole seek rail must follow {mode:?}, x={x}, drag={:?}, pointerlayer={:?}",
                fixture.app.seek_drag,
                fixture
                    .context
                    .layer_id_at(pos2(x, READING_FOOTER_HEIGHT * 0.5))
            );
        }
        for (x, forward) in [(348.0, rtl), (392.0, !rtl)] {
            fixture.app.jump_to_chapter(1, None);
            fixture.frame(size, vec![]);
            fixture.panel(bar, vec![], ReaderApp::reading_bar);
            fixture.panel(bar, vec![], ReaderApp::reading_bar);
            fixture.panel(bar, click(pos2(x, 20.0)), ReaderApp::reading_bar);
            assert_eq!(
                fixture.app.chapter,
                if forward { 2 } else { 0 },
                "centered toolbar arrows must have physical left/right semantics for {mode:?}"
            );
        }
    }
}

#[test]
fn book_title_and_progress_remain_visible_with_reading_chrome_hidden() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    fixture.app.book.title = "メニューを閉じた本の題名".into();
    let output = fixture.frame(vec2(1280.0, 760.0), vec![]);
    assert!(fixture.app.focus);
    let title = text_bounds(&output, "メニューを閉じた本の題名");
    let (start, _, total) = fixture.app.page_numbers().unwrap();
    let progress = text_bounds(&output, &format!("{start} / {total}"));
    assert!(title.top() >= 0.0 && title.bottom() <= 32.0);
    assert!(progress.left() >= 0. && progress.right() <= 108.);
    assert!(progress.top() >= 760. - READING_FOOTER_HEIGHT && progress.bottom() <= 760.);
}

#[test]
fn image_chapter_seek_rail_uses_whole_book_page_weights() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(3);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    fixture.frame(vec2(1280., 760.), vec![]);
    fixture.app.pagination_index =
        Some(crate::pagination::PageIndex::new(vec![41, 1, 20]).unwrap());
    fixture.app.chapter = 1;
    fixture.app.page = 0;
    assert_eq!(fixture.app.page_numbers(), Some((42, 42, 62)));
    assert!(
        (fixture.app.reading_fraction() - 41. / 61.).abs() < 0.000001,
        "a one-page illustration must not receive a whole chapter's progress weight"
    );
}

#[test]
fn queued_whole_book_seek_waits_for_the_current_page_index() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(3);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    fixture.frame(vec2(1280., 760.), vec![]);
    let key = fixture.app.pagination_key.clone();
    let generation = fixture
        .app
        .pagination_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    fixture.app.pagination_index = None;
    fixture.app.pagination_pending = true;
    fixture.app.seek_book(41. / 61.);
    assert_eq!(fixture.app.chapter, 0);
    assert_eq!(fixture.app.pending_book_seek, Some(41. / 61.));
    fixture
        .app
        .tx
        .send(Work::Pagination {
            key: key.clone(),
            generation: generation.wrapping_sub(1),
            result: Ok(crate::pagination::PageIndex::new(vec![1, 1, 1]).unwrap()),
        })
        .unwrap();
    fixture.app.handle_work();
    assert_eq!(fixture.app.chapter, 0);
    assert!(fixture.app.pending_book_seek.is_some());
    fixture
        .app
        .tx
        .send(Work::Pagination {
            key,
            generation,
            result: Ok(crate::pagination::PageIndex::new(vec![41, 1, 20]).unwrap()),
        })
        .unwrap();
    fixture.app.handle_work();
    assert_eq!(fixture.app.chapter, 1);
    assert_eq!(fixture.app.pending_page_index, Some(0));
    assert!(fixture.app.pending_book_seek.is_none());
}

#[test]
fn pagination_results_from_old_fonts_or_window_sizes_cannot_replace_current_index() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let original_key = fixture.app.pagination_key.clone();
    fixture.app.settings.font_size += 4.;
    fixture.frame(vec2(1280., 760.), vec![]);
    let font_key = fixture.app.pagination_key.clone();
    let font_epoch = fixture
        .app
        .pagination_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_ne!(original_key, font_key);
    fixture.frame(vec2(900., 760.), vec![]);
    let window_key = fixture.app.pagination_key.clone();
    let window_epoch = fixture
        .app
        .pagination_epoch
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_ne!(font_key, window_key);
    assert!(window_epoch > font_epoch);
    fixture.app.pagination_index = None;
    fixture.app.pagination_pending = true;
    for (key, generation) in [(font_key, window_epoch), (window_key.clone(), font_epoch)] {
        fixture
            .app
            .tx
            .send(Work::Pagination {
                key,
                generation,
                result: Err("obsolete pagination failure".into()),
            })
            .unwrap();
        fixture.app.handle_work();
        assert!(fixture.app.pagination_pending);
        assert!(fixture.app.pagination_index.is_none());
        assert!(fixture.app.pagination_error.is_none());
    }
    fixture
        .app
        .tx
        .send(Work::Pagination {
            key: window_key,
            generation: window_epoch,
            result: Ok(crate::pagination::PageIndex::new(vec![43]).unwrap()),
        })
        .unwrap();
    fixture.app.handle_work();
    assert!(!fixture.app.pagination_pending);
    assert_eq!(fixture.app.pagination_index.as_ref().unwrap().counts, [43]);
}

#[test]
fn whole_book_seek_reaches_the_requested_native_page_after_chapter_change() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(3);
    for (chapter, repeats) in [(0, 200), (2, 120)] {
        fixture.app.book.chapters[chapter] = reader_core::parse_html(
            &format!(
                "<html><body>{}</body></html>",
                "<p>日本語の本文です。ページごとの正しい移動を確かめます。</p>".repeat(repeats)
            ),
            chapter as u32,
            &format!("text-{chapter}.xhtml"),
            "",
        )
        .unwrap();
    }
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    fixture.app.layout_key.clear();
    let size = vec2(1280., 760.);
    fixture.frame(size, vec![]);
    let index = crate::pagination::compute(
        &fixture.app.book,
        fixture.app.reading_extent,
        &fixture.app.settings,
        &std::sync::atomic::AtomicU64::new(0),
        0,
    )
    .unwrap();
    assert_eq!(index.counts[1], 1);
    assert!(index.counts[0] > 3 && index.counts[2] > 3);
    let target = index.counts[2] - 2;
    let fraction = index.fraction(2, target);
    fixture.app.pagination_index = Some(index.clone());
    fixture.app.seek_book(fraction);
    assert_eq!(fixture.app.chapter, 2);
    assert_eq!(fixture.app.pending_page_index, Some(target));
    fixture.frame(size, vec![]);
    assert_eq!(fixture.app.page, target);
    assert!(fixture.app.pending_page_index.is_none());
    assert_eq!(
        fixture.app.layout.pages[target]
            .first_locator()
            .unwrap()
            .chapter,
        2
    );
    assert_eq!(
        fixture.app.page_numbers().unwrap().0,
        index.offsets[2] + target + 1
    );
    assert!((fixture.app.reading_fraction() - fraction).abs() < 0.000001);
}

#[test]
fn reference_audio_clear_button_stays_separate_and_clickable_at_narrow_widths() {
    let mut fixture = TestApp::new();
    fixture.app.speech_provider = "irodori".into();
    fixture.app.speech_reference = Some(fixture.directory.path().join("voice.wav"));
    let first = fixture.panel(vec2(800., 760.), vec![], ReaderApp::speech_settings);
    let header = text_bounds(&first, "声の参照・バリエーション");
    fixture.panel(
        vec2(800., 760.),
        click(header.center()),
        ReaderApp::speech_settings,
    );
    for (width, name) in [
        (800., "voice.wav".to_owned()),
        (
            420.,
            format!("長い参照音声{}声.wav", "日本語のファイル名".repeat(16)),
        ),
    ] {
        fixture.app.speech_reference = Some(fixture.directory.path().join(name));
        let size = vec2(width, 760.);
        fixture.panel(size, vec![], ReaderApp::speech_settings);
        let output = fixture.panel(size, vec![], ReaderApp::speech_settings);
        let pick = text_bounds(&output, "WAVを選ぶ");
        let clear = text_bounds(&output, "×");
        assert!(pick.right() + 4. < clear.left());
        assert!(clear.right() <= width && clear.left() > 0.);
        assert!((pick.center().y - clear.center().y).abs() < 1.);
        fixture.panel(size, click(clear.center()), ReaderApp::speech_settings);
        assert!(
            fixture.app.speech_reference.is_none(),
            "the visible clear action must not be hidden beneath the filename"
        );
        assert!(fixture.app.speech.is_none());
    }
}

#[test]
fn aivis_settings_select_actual_signed_style_ids_without_starting_audio() {
    use std::io::Read;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for (route, body) in [
            ("/engine_manifest", r#"{"name":"AivisSpeech Engine"}"#),
            (
                "/speakers",
                r#"[{"name":"Fixture Voice","speaker_uuid":"fixture-uuid","styles":[{"id":-123,"name":"Normal"},{"id":456,"name":"Calm"}]}]"#,
            ),
        ] {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "Aivis settings did not request {route}"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("catalog fixture failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with(&format!("GET {route} ")));
            write!(stream,"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let mut fixture = TestApp::new();
    fixture.app.speech_provider = "aivis".into();
    fixture.app.speech_endpoint = endpoint.clone();
    fixture.app.speech_voice = "-123".into();
    fixture.app.speech_profiles.set(
        "aivis",
        ProviderConfig::AivisSpeech {
            base_url: endpoint,
            speaker: -123,
            engine_path: None,
            auto_start: false,
        },
    );
    let size = vec2(800., 760.);
    fixture.panel(size, vec![], ReaderApp::speech_settings);
    server.join().unwrap();
    fn contains(shape: &egui::Shape, prefix: &str) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.job.text.starts_with(prefix),
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| contains(shape, prefix)),
            _ => false,
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let output = loop {
        let output = fixture.panel(size, vec![], ReaderApp::speech_settings);
        if output
            .shapes
            .iter()
            .any(|shape| contains(&shape.shape, "Fixture Voice · Normal"))
        {
            break output;
        }
        assert!(
            Instant::now() < deadline,
            "the actual catalog was not published to settings"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let selected = text_bounds(&output, "Fixture Voice · Normal");
    fixture.panel(size, click(selected.center()), ReaderApp::speech_settings);
    let menu = fixture.panel(size, vec![], ReaderApp::speech_settings);
    let calm = text_bounds(&menu, "Fixture Voice · Calm");
    fixture.panel(size, click(calm.center()), ReaderApp::speech_settings);
    assert_eq!(fixture.app.speech_voice, "456");
    let output = fixture.panel(size, vec![], ReaderApp::speech_settings);
    text_bounds(&output, "Fixture Voice · Calm");
    assert!(matches!(
        fixture.app.provider_config(),
        ProviderConfig::AivisSpeech { speaker: 456, .. }
    ));
    assert!(fixture.app.speech.is_none());
}

#[test]
fn scrolled_dictionary_rows_keep_the_settings_content_clip() {
    let mut fixture = TestApp::new();
    fixture.app.setting_tab = SettingTab::Dictionaries;
    fixture.app.dictionary_infos = (0..40)
        .map(|index| DictionaryInfo {
            id: format!("clip-{index}"),
            path: fixture.directory.path().join(format!("{index}.mdx")),
            title: format!("辞書クリップ試験 {index:02}"),
            mdd_paths: vec![],
            enabled: true,
            entry_count: 10,
            format_version: "2.0".into(),
            status: reader_library::DictionaryStatus::Ready,
        })
        .collect();
    let size = vec2(800., 620.);
    fixture.panel(size, vec![], ReaderApp::settings_view);
    let initial = fixture.panel(size, vec![], ReaderApp::settings_view);
    let row = text_bounds(&initial, "辞書クリップ試験 00");
    let output = fixture.panel(
        size,
        vec![
            egui::Event::PointerMoved(row.center()),
            wheel(egui::MouseWheelUnit::Point, -600.),
        ],
        ReaderApp::settings_view,
    );
    let header = text_bounds(&output, "設定");
    fn dictionary_clips(shape: &egui::Shape, clip: Rect, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text.starts_with("辞書クリップ試験") => {
                out.push(clip)
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    dictionary_clips(shape, clip, out);
                }
            }
            _ => {}
        }
    }
    let mut clips = Vec::new();
    for shape in &output.shapes {
        dictionary_clips(&shape.shape, shape.clip_rect, &mut clips);
    }
    assert!(!clips.is_empty());
    assert!(
        clips
            .iter()
            .all(|clip| clip.top() > header.bottom() && clip.bottom() < size.y),
        "scrolling rows must never override their ancestor scroll/header clipping"
    );
}

fn linked_epub_fixture(directory: &std::path::Path) -> PathBuf {
    let path = directory.join("asynchronous-links.epub");
    let entries = [
        (
            "META-INF/container.xml",
            r#"<container><rootfiles><rootfile full-path="OPS/package.opf"/></rootfiles></container>"#,
        ),
        (
            "OPS/package.opf",
            r#"<package><metadata><title>Async link fixture</title></metadata><manifest><item id="main" href="Text/main.xhtml" media-type="application/xhtml+xml"/><item id="second" href="Text/second.xhtml" media-type="application/xhtml+xml"/><item id="aux" href="Notes/extra.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="main"/><itemref idref="second"/></spine></package>"#,
        ),
        (
            "OPS/Text/main.xhtml",
            r##"<html xmlns:epub="http://www.idpf.org/2007/ops"><body><p id="start">本文です。<a epub:type="noteref" href="#note">注を見る</a><a href="../Notes/extra.xhtml#detail">補足を見る</a><a href="second.xhtml#destination">次の章へ</a></p><aside id="note"><p>本文中の注釈です。</p></aside></body></html>"##,
        ),
        (
            "OPS/Text/second.xhtml",
            r#"<html><body><p id="destination">二章の目的地です。</p></body></html>"#,
        ),
        (
            "OPS/Notes/extra.xhtml",
            r#"<html><body><p id="detail">本編外の補足説明です。</p></body></html>"#,
        ),
    ];
    let file = std::fs::File::create(&path).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, text) in entries {
        archive.start_file(name, options).unwrap();
        archive.write_all(text.as_bytes()).unwrap();
    }
    archive.finish().unwrap();
    path
}

fn link_hit(chapter: &reader_core::Chapter, label: &str) -> Hit {
    let (index, atom) = chapter
        .atoms()
        .into_iter()
        .enumerate()
        .find(|(_, atom)| atom.link.is_some() && atom.text.contains(label))
        .unwrap();
    Hit {
        rect: Rect::from_min_size(Pos2::ZERO, vec2(20., 20.)),
        locator: atom.locator_at(0).unwrap(),
        end: atom.locator_at(atom.text.len()).unwrap(),
        unit_id: layout::unit_id(chapter.id, index),
        offset: 0,
        text: atom.text.clone(),
    }
}

fn finish_link_worker(fixture: &mut TestApp) {
    let mut replies = Vec::new();
    loop {
        let work = fixture
            .app
            .rx
            .recv_timeout(Duration::from_secs(5))
            .expect("local link resolver must finish");
        let linked = matches!(work, Work::Link { .. });
        // Do not let handle_work consume a Link which this helper has not yet
        // observed: it drains the whole channel, including concurrent arrivals.
        replies.push(work);
        if linked {
            break;
        }
    }
    // Replay the observed prefix before any newer arrivals. Requeueing it onto
    // the live channel's tail could let a later reply run before this Link.
    let (replay_tx, replay_rx) = std::sync::mpsc::channel();
    for work in replies {
        replay_tx.send(work).unwrap();
    }
    let incoming = std::mem::replace(&mut fixture.app.rx, replay_rx);
    fixture.app.handle_work();
    fixture.app.rx = incoming;
}

#[test]
fn link_worker_wait_preserves_a_link_queued_behind_unrelated_work() {
    let mut fixture = TestApp::new();
    fixture.app.book = reader_core::open(linked_epub_fixture(fixture.directory.path())).unwrap();
    let target = fixture.app.book.load_link_target(0, "#note").unwrap();
    assert!(target.is_some());
    let anchor = pos2(80., 120.);
    // Make the losing interleaving deterministic: handle_work would drain both
    // replies if the helper requeued the first reply before observing the Link.
    fixture
        .app
        .tx
        .send(Work::Opened {
            generation: fixture.app.open_generation.wrapping_add(1),
            result: Err("ignored stale fixture reply".into()),
        })
        .unwrap();
    fixture
        .app
        .tx
        .send(Work::Link {
            book_generation: fixture.app.open_generation,
            request: fixture.app.link_request,
            result: Ok(target),
            note: true,
            anchor,
        })
        .unwrap();
    finish_link_worker(&mut fixture);
    let (text, locator, actual_anchor) = fixture.app.footnote.as_ref().unwrap();
    assert!(text.contains("本文中の注釈です"));
    assert_eq!(locator.chapter, 0);
    assert_eq!(*actual_anchor, anchor);
}

#[test]
fn async_links_open_notes_auxiliary_documents_and_spine_targets_without_source_writes() {
    let mut fixture = TestApp::new();
    let path = linked_epub_fixture(fixture.directory.path());
    let original = std::fs::read(&path).unwrap();
    fixture.app.book = reader_core::open(&path).unwrap();
    fixture.app.open_generation = 27;
    fixture.app.chapter = 0;
    fixture.render_reader(vec2(640., 640.));
    let origin = fixture.app.current_book_locator().unwrap();
    let note = link_hit(&fixture.app.book.chapters[0], "注を見る");
    let auxiliary = link_hit(&fixture.app.book.chapters[0], "補足を見る");
    let next = link_hit(&fixture.app.book.chapters[0], "次の章へ");
    let anchor = pos2(200., 240.);
    let context = fixture.context.clone();
    assert!(fixture.app.follow_link(&note, anchor, &context));
    finish_link_worker(&mut fixture);
    assert!(
        fixture.app.jump_marks.is_empty(),
        "opening a note preview is not a reading-position jump"
    );
    let (text, locator, popup_anchor) = fixture.app.footnote.as_ref().unwrap();
    assert!(text.contains("本文中の注釈です"));
    assert_eq!(locator.chapter, 0);
    assert_eq!(*popup_anchor, anchor);
    assert_eq!(fixture.app.chapter, 0);
    assert!(fixture.app.follow_link(&auxiliary, anchor, &context));
    finish_link_worker(&mut fixture);
    assert_eq!(
        fixture.app.jump_marks.first().map(|mark| mark.locator),
        Some(origin)
    );
    let auxiliary = fixture.app.auxiliary_document.as_ref().unwrap();
    assert_eq!(auxiliary.spine_index, None);
    assert!(
        auxiliary
            .chapter
            .text_at_anchor(auxiliary.locator)
            .unwrap()
            .contains("本編外の補足説明です")
    );
    assert_eq!(fixture.app.auxiliary_scroll_target, Some(auxiliary.locator));
    assert!(fixture.app.footnote.is_none());
    assert_eq!(fixture.app.book.chapters.len(), 2);
    assert_eq!(
        fixture.app.chapter, 0,
        "non-spine documents must preserve the main reading position"
    );
    assert!(fixture.app.follow_link(&next, anchor, &context));
    finish_link_worker(&mut fixture);
    assert_eq!(fixture.app.chapter, 1);
    assert_eq!(
        fixture.app.pending_restore,
        fixture.app.book.chapters[1]
            .anchors
            .get("destination")
            .copied()
    );
    assert!(fixture.app.book.chapters[1].loaded);
    assert!(fixture.app.auxiliary_document.is_none());
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn stale_async_links_cannot_open_a_popup_in_a_new_book_or_after_a_newer_click() {
    let mut fixture = TestApp::new();
    fixture.app.book = reader_core::open(linked_epub_fixture(fixture.directory.path())).unwrap();
    let note = link_hit(&fixture.app.book.chapters[0], "注を見る");
    let context = fixture.context.clone();
    for supersede_book in [false, true] {
        assert!(fixture.app.follow_link(&note, pos2(20., 30.), &context));
        if supersede_book {
            fixture.app.open_generation += 1;
        } else {
            fixture.app.link_request += 1;
        }
        finish_link_worker(&mut fixture);
        assert!(fixture.app.footnote.is_none());
        assert!(fixture.app.auxiliary_document.is_none());
        assert!(fixture.app.notice.is_none());
        assert_eq!(fixture.app.chapter, 0);
    }
}

#[test]
fn blank_book_and_library_drags_move_the_window_while_text_drags_select_source() {
    let start_drag = |outputs: &[egui::FullOutput]| {
        outputs
            .iter()
            .flat_map(|output| output.viewport_output.values())
            .flat_map(|output| &output.commands)
            .any(|command| matches!(command, egui::ViewportCommand::StartDrag))
    };
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280.0, 760.0);
    let margin = pos2(24.0, 380.0);
    assert!(
        fixture.app.layout.pages[fixture.app.page]
            .hit(margin)
            .is_none()
    );
    let output = fixture.drag(size, margin, margin + vec2(24.0, 0.0));
    assert!(
        start_drag(&output),
        "dragging the book margin starts a native window drag"
    );
    assert!(fixture.app.selected.is_none());
    assert!(fixture.app.pending_page_turn.is_none());
    let first = fixture.app.layout.pages[fixture.app.page].hits[4].clone();
    let last = fixture.app.layout.pages[fixture.app.page].hits[10].clone();
    let output = fixture.drag(size, first.rect.center(), last.rect.center());
    assert!(
        !start_drag(&output),
        "dragging actual text must never move the window"
    );
    let selection = fixture
        .app
        .selected
        .as_ref()
        .expect("dragging text makes a source-backed selection");
    assert_eq!(selection.locator, first.locator);
    assert_eq!(selection.end, last.end);
    assert!(!selection.text.is_empty());
    fixture.app.screen = Screen::Library;
    fixture.frame(size, vec![]);
    fixture.frame(size, vec![]);
    let blank = pos2(1000.0, 600.0);
    let output = fixture.drag(size, blank, blank + vec2(24.0, 16.0));
    assert!(
        start_drag(&output),
        "the empty library surface also starts a native window drag"
    );
}

#[test]
fn five_wheel_notches_in_one_input_frame_paint_five_successive_source_pages() {
    let mut fixture = TestApp::new();
    let size = fixture.wheel_fixture();
    let expected: Vec<_> = fixture
        .app
        .layout
        .pages
        .iter()
        .take(6)
        .map(|page| page.first_locator().unwrap())
        .collect();
    for (unit, values) in [
        (egui::MouseWheelUnit::Line, vec![-5.0]),
        (egui::MouseWheelUnit::Line, vec![-1.0; 5]),
        (egui::MouseWheelUnit::Page, vec![-5.0]),
        (egui::MouseWheelUnit::Point, vec![-400.0]),
    ] {
        fixture.app.page = 0;
        fixture.frame(size, vec![]);
        let mut events = vec![egui::Event::PointerMoved(size.to_pos2() * 0.5)];
        events.extend(values.into_iter().map(|value| wheel(unit, value)));
        fixture.frame(size, events);
        assert_eq!(
            fixture.app.page, 0,
            "input queues turns for subsequent rendered frames"
        );
        assert_eq!(
            fixture.app.pending_wheel_turns.len(),
            5,
            "{unit:?} must preserve all five notches"
        );
        for (page, &locator) in expected.iter().enumerate().skip(1) {
            let output = fixture.frame(size, vec![]);
            assert_eq!(
                fixture.app.page, page,
                "exactly one queued notch is rendered per frame"
            );
            assert_eq!(
                fixture.app.layout.pages[fixture.app.page].first_locator(),
                Some(locator)
            );
            text_bounds(&output, &format!("{} /", page + 1));
            assert_eq!(fixture.app.pending_wheel_turns.len(), 5 - page);
            if page < 5 {
                assert!(
                    output
                        .viewport_output
                        .values()
                        .any(|viewport| viewport.repaint_delay <= Duration::from_millis(16)),
                    "the next queued page must schedule its own repaint"
                );
            }
        }
        fixture.frame(size, vec![]);
        assert_eq!(
            fixture.app.page, 5,
            "an input burst must not cause extra turns afterward"
        );
    }
}

#[test]
fn one_wheel_notch_each_frame_is_not_dropped_in_either_direction() {
    let mut fixture = TestApp::new();
    let size = fixture.wheel_fixture();
    let expected: Vec<_> = fixture
        .app
        .layout
        .pages
        .iter()
        .take(8)
        .map(|page| page.first_locator().unwrap())
        .collect();
    for forward in [true, false] {
        let start = if forward { 0 } else { 6 };
        fixture.app.page = start;
        fixture.frame(size, vec![]);
        for index in 0..5 {
            fixture.frame(
                size,
                vec![wheel(
                    egui::MouseWheelUnit::Line,
                    if forward { -1.0 } else { 1.0 },
                )],
            );
            let page = if forward {
                start + index
            } else {
                start - index
            };
            assert_eq!(fixture.app.page, page);
            assert_eq!(
                fixture.app.layout.pages[page].first_locator(),
                Some(expected[page])
            );
            assert_eq!(
                fixture.app.pending_wheel_turns.len(),
                1,
                "the newest notch remains queued while the previous notch is displayed"
            );
        }
        fixture.frame(size, vec![]);
        let page = if forward { start + 5 } else { start - 5 };
        assert_eq!(fixture.app.page, page);
        assert_eq!(
            fixture.app.layout.pages[page].first_locator(),
            Some(expected[page])
        );
        assert!(fixture.app.pending_wheel_turns.is_empty());
    }
}

#[test]
fn wheel_direction_changes_keep_the_original_notch_order() {
    let mut fixture = TestApp::new();
    let size = fixture.wheel_fixture();
    fixture.app.page = 3;
    fixture.frame(size, vec![]);
    fixture.frame(
        size,
        [-1.0, -1.0, 1.0, 1.0, -1.0]
            .into_iter()
            .map(|value| wheel(egui::MouseWheelUnit::Line, value))
            .collect(),
    );
    assert_eq!(
        fixture
            .app
            .pending_wheel_turns
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [true, true, false, false, true]
    );
    for page in [4, 5, 4, 3, 4] {
        fixture.frame(size, vec![]);
        assert_eq!(
            fixture.app.page, page,
            "opposite wheel events must not cancel queued visible turns"
        );
    }
    assert!(fixture.app.pending_wheel_turns.is_empty());
}

#[test]
fn queued_wheel_pages_survive_chapter_loading_and_the_first_layout_frame() {
    let mut fixture = TestApp::new();
    let size = vec2(640.0, 420.0);
    let first = reader_core::parse_html(
        "<html><body><p>短い最初の章。</p></body></html>",
        0,
        "first.xhtml",
        "",
    )
    .unwrap();
    let next = reader_core::parse_html(
        &format!(
            "<html><body>{}</body></html>",
            "<p>次の章でも一段ずつページを表示します。</p>".repeat(160)
        ),
        1,
        "next.xhtml",
        "",
    )
    .unwrap();
    fixture.use_chapter(first);
    let mut placeholder = next.clone();
    placeholder.loaded = false;
    placeholder.blocks.clear();
    placeholder.anchors.clear();
    fixture.app.book.chapters.push(placeholder);
    // Suppress speculative loading so the actual chapter request has a
    // deterministic completion that this fixture can hold until released.
    fixture.app.chapter_prefetch.insert(1);
    fixture.app.screen = Screen::Reader;
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    fixture.app.focus = true;
    fixture.frame(size, vec![egui::Event::PointerMoved(size.to_pos2() * 0.5)]);
    fixture.frame(size, vec![wheel(egui::MouseWheelUnit::Line, -5.0)]);
    assert_eq!(fixture.app.pending_wheel_turns.len(), 5);
    fixture.frame(size, vec![]);
    assert_eq!(fixture.app.chapter, 1);
    assert_eq!(fixture.app.chapter_loading, Some(1));
    assert_eq!(fixture.app.pending_wheel_turns.len(), 4);
    let request = fixture.app.chapter_request;
    // This in-memory fixture has no EPUB loader. Intercept its worker's
    // expected error, then inject the successful source at a controlled time.
    let completion = fixture.app.rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(completion, Work::Chapter { request: returned, index: 1, result: Err(_), .. } if returned == request)
    );
    for additional in 1..=3 {
        let output = fixture.frame(size, vec![wheel(egui::MouseWheelUnit::Line, -1.0)]);
        text_bounds(&output, "章を読み込んでいます");
        assert_eq!(
            fixture.app.pending_wheel_turns.len(),
            4 + additional,
            "new notches arriving during chapter loading must also be retained"
        );
        assert!(fixture.app.layout.pages.is_empty());
    }
    fixture
        .app
        .tx
        .send(Work::Chapter {
            book_generation: fixture.app.open_generation,
            request,
            index: 1,
            result: Ok(next),
        })
        .unwrap();
    fixture.frame(size, vec![]);
    assert!(fixture.app.chapter_loading.is_none());
    assert!(!fixture.app.layout_key.is_empty());
    assert!(fixture.app.layout.pages.len() > 7);
    assert_eq!(
        fixture.app.page, 0,
        "the first frame after loading must paint the chapter before consuming another notch"
    );
    assert_eq!(fixture.app.pending_wheel_turns.len(), 7);
    let expected: Vec<_> = fixture
        .app
        .layout
        .pages
        .iter()
        .take(8)
        .map(|page| page.first_locator().unwrap())
        .collect();
    assert_eq!(expected[0].chapter, 1);
    for (page, &locator) in expected.iter().enumerate().skip(1) {
        fixture.frame(size, vec![]);
        assert_eq!(fixture.app.page, page);
        assert_eq!(
            fixture.app.layout.pages[page].first_locator(),
            Some(locator)
        );
        assert_eq!(fixture.app.pending_wheel_turns.len(), 7 - page);
    }
}

#[test]
fn library_list_titles_and_navigation_labels_share_fixed_left_edges() {
    let mut fixture = TestApp::new();
    fixture.app.settings.library_list = true;
    fixture.app.shelf = [
        "短い書籍名".to_owned(),
        format!("とても長い書籍名{}", "本の題名".repeat(30)),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, title)| BookEntry {
        id: format!("left-edge-book-{index}"),
        uuid: None,
        calibre_id: None,
        title,
        authors: vec![format!("著者{index}")],
        tags: vec![],
        epub_path: fixture.directory.path().join(format!("book-{index}.txt")),
        cover_path: None,
        source: BookSource::Imported,
        added_at: 0,
    })
    .collect();
    for width in [820.0, 1280.0] {
        let size = vec2(width, 760.0);
        fixture.panel(size, vec![], ReaderApp::library);
        let output = fixture.panel(size, vec![], ReaderApp::library);
        let short = text_bounds(&output, "短い書籍名");
        let long = text_bounds(&output, "とても長い書籍名");
        assert!((short.left() - long.left()).abs() < 1.0);
        assert!(
            (short.left() - 294.0).abs() < 1.0,
            "row titles start directly after the cover column"
        );
        assert!(
            long.top() > short.bottom(),
            "single-line titles stay on separate list rows"
        );
        for label in ["すべての本", "読書中", "読了"] {
            assert!(
                (text_bounds(&output, label).left() - 54.0).abs() < 1.0,
                "navigation label {label:?} keeps its own left column"
            );
        }
    }
}

#[test]
fn toc_titles_align_by_depth_instead_of_centering_by_text_width() {
    use reader_core::TocEntry;
    let mut fixture = TestApp::new();
    fixture.app.drawer = Drawer::Toc;
    fixture.app.book.toc = vec![
        TocEntry {
            target: None,
            title: "短い親見出し".into(),
            chapter: 0,
            fragment: None,
            children: vec![
                TocEntry {
                    target: None,
                    title: format!("長い子見出し{}", "階層付きの目次".repeat(20)),
                    chapter: 0,
                    fragment: Some("child".into()),
                    children: vec![TocEntry {
                        target: None,
                        title: "孫見出し".into(),
                        chapter: 0,
                        fragment: Some("grandchild".into()),
                        children: vec![],
                    }],
                },
                TocEntry {
                    target: None,
                    title: "短い子見出し".into(),
                    chapter: 0,
                    fragment: Some("child-2".into()),
                    children: vec![],
                },
            ],
        },
        TocEntry {
            target: None,
            title: format!("長い親見出し{}", "章の題名".repeat(20)),
            chapter: 1,
            fragment: None,
            children: vec![],
        },
    ];
    let size = vec2(272.0, 420.0);
    fixture.panel(size, vec![], ReaderApp::drawer);
    let output = fixture.panel(size, vec![], ReaderApp::drawer);
    let parent = text_bounds(&output, "短い親見出し");
    let child = text_bounds(&output, "長い子見出し");
    let grandchild = text_bounds(&output, "孫見出し");
    assert!((text_bounds(&output, "長い親見出し").left() - parent.left()).abs() < 1.0);
    assert!((text_bounds(&output, "短い子見出し").left() - child.left()).abs() < 1.0);
    assert!((child.left() - parent.left() - 12.0).abs() < 1.0);
    assert!((grandchild.left() - child.left() - 12.0).abs() < 1.0);
}

#[test]
fn deeply_nested_toc_rows_scroll_into_view_and_navigate_to_their_fragment() {
    use reader_core::TocEntry;
    let mut fixture = TestApp::new();
    let html = format!(
        "<html><body>{}<h2 id='deep-section'>深い本文の見出し</h2><p>ここへ移動します。</p></body></html>",
        "<p>前方の本文を読みます。</p>".repeat(70)
    );
    let chapter = reader_core::parse_html(&html, 0, "deep.xhtml", "").unwrap();
    let target = chapter.anchors["deep-section"];
    fixture.use_chapter(chapter);
    fixture.app.drawer = Drawer::Toc;
    let mut nested = Vec::new();
    for depth in (0..7).rev() {
        nested = vec![TocEntry {
            target: None,
            title: format!("階層{depth}の見出し{}", "長い章名".repeat(12)),
            chapter: 0,
            fragment: Some(if depth == 6 {
                "deep-section".into()
            } else {
                format!("level-{depth}")
            }),
            children: nested,
        }];
    }
    fixture.app.book.toc = (0..12)
        .map(|n| TocEntry {
            target: None,
            title: format!("前方の章{n}"),
            chapter: 0,
            fragment: None,
            children: vec![],
        })
        .chain(nested)
        .collect();
    let size = vec2(272., 190.);
    fixture.panel(size, vec![], ReaderApp::drawer);
    fixture.panel(size, vec![], ReaderApp::drawer);
    fixture.panel(
        size,
        vec![
            egui::Event::PointerMoved(pos2(220., 140.)),
            wheel(egui::MouseWheelUnit::Point, -1600.),
        ],
        ReaderApp::drawer,
    );
    for _ in 0..5 {
        fixture.panel(size, vec![], ReaderApp::drawer);
    }
    let output = fixture.panel(size, vec![], ReaderApp::drawer);
    let deepest = text_bounds(&output, "階層6の見出し");
    assert!(
        (deepest.left() - 72.).abs() < 1.,
        "seven levels preserve 12px indentation"
    );
    assert!(
        deepest.top() >= 0. && deepest.bottom() <= size.y,
        "deep entries must become visible by scrolling: {deepest:?}"
    );
    assert!(
        deepest.right() <= size.x + 1.,
        "deep long titles truncate inside the drawer"
    );
    fixture.panel(size, click(deepest.center()), ReaderApp::drawer);
    assert_eq!(
        fixture.app.resume_locator,
        Some(target),
        "deep row click navigates to its real source fragment"
    );
}

#[test]
fn persistent_seek_footer_is_clickable_with_chrome_hidden_and_does_not_reflow_on_toggle() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(4);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    fixture.app.settings.reading_mode = reader_library::ReadingMode::Horizontal;
    let size = vec2(1280., 760.);
    fixture.frame(size, vec![]);
    let extent = fixture.app.reading_extent;
    let key = fixture.app.layout_key.clone();
    assert_eq!(
        extent,
        vec2(1280., 760.),
        "footer lives inside the book's vertical margin, not outside the reading extent"
    );
    assert!(fixture.app.focus);
    let point = pos2(1250., 747.);
    fixture.frame(size, vec![egui::Event::PointerMoved(point)]);
    fixture.frame(
        size,
        vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    fixture.frame(
        size,
        vec![egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    fixture.frame(size, vec![]);
    assert_eq!(
        fixture.app.chapter, 3,
        "the full-width footer slider remains active with menus hidden"
    );
    assert!(
        fixture.app.focus,
        "seek must not reveal chrome or dispatch a book-center tap"
    );
    assert!(fixture.app.pending_center_tap.is_none());
    assert_eq!(
        fixture.app.jump_marks.len(),
        1,
        "a slider jump records its origin once"
    );
    fixture
        .app
        .execute_action(crate::actions::Action::ToggleChrome, &fixture.context);
    fixture.frame(size, vec![]);
    assert_eq!(fixture.app.reading_extent, extent);
    let visible_key = fixture.app.layout_key.clone();
    fixture
        .app
        .execute_action(crate::actions::Action::ToggleChrome, &fixture.context);
    fixture.frame(size, vec![]);
    assert_eq!(fixture.app.layout_key, visible_key);
    assert!(!key.is_empty());
}

#[test]
fn reading_metadata_occupies_the_configured_margin_once_and_keeps_images_uncovered() {
    let size = vec2(1280., 760.);
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    fixture.app.book.title = "metadata-inset-regression".into();
    for margin in [0., 48., 96.] {
        fixture.app.settings.margin_vertical = Some(margin);
        let output = fixture.frame(size, vec![]);
        assert_eq!(fixture.app.reading_extent, size);
        let effective = margin.max(READING_FOOTER_HEIGHT);
        // Source cells may include natural bearing/leading beyond painted ink.
        // Measure the real outline, including ruby and rotated vertical forms.
        let glyphs = fixture.app.layout.pages[fixture.app.page].glyphs.clone();
        let mut ink = Rect::NOTHING;
        for glyph in glyphs {
            if let Some(outline) = fixture.app.fonts.outline_bounds(glyph.face, glyph.id) {
                for point in [
                    outline.left_top(),
                    outline.right_top(),
                    outline.left_bottom(),
                    outline.right_bottom(),
                ] {
                    let mut offset = point.to_vec2() * glyph.size;
                    if glyph.rotated {
                        offset = vec2(-offset.y, offset.x);
                    }
                    ink.extend_with(glyph.origin + offset);
                }
            }
        }
        let title = text_bounds(&output, "metadata-inset-regression");
        assert!(
            ink.is_finite()
                && ink.top() > title.bottom()
                && ink.bottom() < size.y - READING_FOOTER_HEIGHT,
            "painted text and ruby must not overlap top/bottom metadata ({margin}); ink={ink:?}, title={title:?}"
        );
        let mut expected_settings = fixture.app.settings.clone();
        expected_settings.margin_vertical = Some(effective);
        let expected = layout::chapter_layout(
            &mut fixture.app.fonts,
            &fixture.app.book.chapters[0],
            size,
            &expected_settings,
        );
        assert_eq!(fixture.app.layout.pages.len(), expected.pages.len());
        assert_eq!(
            fixture.app.layout.pages[0].glyphs.len(),
            expected.pages[0].glyphs.len(),
            "the caller must not subtract another footer from the text layout's full viewport"
        );
        let (start, end, total) = fixture.app.page_numbers().unwrap();
        assert_eq!(start, end);
        let label = text_bounds(&output, &format!("{start} / {total}"));
        assert!(label.top() >= size.y - READING_FOOTER_HEIGHT && label.bottom() <= size.y);
    }
    fixture.image_fixture(2);
    fixture.app.settings.spread = reader_library::SpreadMode::Single;
    fixture.app.settings.margin_horizontal = Some(0.);
    fixture.app.settings.margin_vertical = Some(0.);
    fixture.frame(size, vec![]);
    let content = crate::reading_surface::content_rect(
        Rect::from_min_size(Pos2::ZERO, size),
        &fixture.app.book.chapters[0],
        &fixture.app.settings,
    );
    assert_eq!(content.top(), READING_FOOTER_HEIGHT);
    assert_eq!(content.bottom(), size.y - READING_FOOTER_HEIGHT);
    let picture = &fixture.app.layout.pages[0].pictures[0];
    assert!(picture.rect.top() >= -0.1 && picture.rect.bottom() <= content.height() + 0.1);
    assert!(
        picture.rect.bottom() + content.top() <= size.y - READING_FOOTER_HEIGHT + 0.1,
        "author image canvas must fit completely above the persistent footer"
    );
}

#[test]
fn jump_marks_ignore_normal_page_turns_keep_five_sources_and_reproject_after_reflow() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280., 760.);
    fixture.app.turn_page(true);
    fixture.frame(size, vec![]);
    fixture.app.turn_page(false);
    fixture.frame(size, vec![]);
    fixture.frame(
        size,
        vec![
            egui::Event::PointerMoved(pos2(640., 380.)),
            wheel(egui::MouseWheelUnit::Line, -1.),
        ],
    );
    fixture.frame(size, vec![]);
    assert!(
        fixture.app.jump_marks.is_empty(),
        "ordinary forward/backward/wheel reading is not jump history"
    );
    for fraction in [0.8, 0.1, 0.7, 0.2, 0.9, 0.3, 0.6] {
        fixture.app.seek_book(fraction);
        fixture.frame(size, vec![]);
    }
    assert_eq!(fixture.app.jump_marks.len(), MAX_JUMP_MARKS);
    let marks = fixture.app.jump_marks.clone();
    let target = marks[0].locator;
    fixture.app.settings.font_size = 36.;
    fixture.app.settings.margin_horizontal = Some(84.);
    fixture.app.settings.margin_vertical = Some(64.);
    let output = fixture.frame(size, vec![]);
    assert_eq!(
        fixture.app.jump_marks, marks,
        "typography changes preserve source locators rather than stale page indices"
    );
    let index = fixture.app.pagination_index.as_ref().unwrap();
    let fraction = index.locator_fraction(target).unwrap();
    let (chapter, page) = index.locate(fraction);
    assert_eq!(chapter, 0);
    assert!(
        fixture.app.layout.pages[page].contains_locator(target),
        "dot position uses the new page containing the source"
    );
    let rail = Rect::from_min_max(pos2(0., size.y - READING_FOOTER_HEIGHT), size.to_pos2());
    let target_point = seek_rail_position(seek_slider_rect(rail), fraction, true);
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Circle(circle) if (circle.radius-3.5).abs()<0.1 && circle.center.distance(target_point)<0.1)),"RTL jump dot lies on the actual slider rail center and uses its handle inset");
    fixture.frame(size, click(target_point));
    fixture.frame(size, vec![]);
    assert!(
        fixture.app.layout.pages[fixture.app.page].contains_locator(target),
        "clicking a reprojected dot restores its real source after reflow"
    );
}

#[test]
fn jump_dots_share_the_real_slider_handle_line_and_consume_only_their_own_clicks() {
    for (mode, rtl) in [
        (reader_library::ReadingMode::Horizontal, false),
        (reader_library::ReadingMode::Vertical, true),
    ] {
        let mut fixture = TestApp::new();
        fixture.image_fixture(6);
        fixture.app.settings.spread = reader_library::SpreadMode::Single;
        fixture.app.settings.reading_mode = mode;
        let size = vec2(1280., 760.);
        fixture.app.jump_to_chapter(5, None);
        fixture.frame(size, vec![]);
        let target = fixture.app.book.chapters[1].start_locator().unwrap();
        fixture.app.jump_marks = vec![JumpMark { locator: target }];
        let fraction = fixture
            .app
            .pagination_index
            .as_ref()
            .unwrap()
            .locator_fraction(target)
            .unwrap();
        let bar = vec2(size.x, READING_FOOTER_HEIGHT);
        fixture.app.seek_drag = Some(fraction);
        fixture.panel(bar, vec![], ReaderApp::reading_seek);
        let output = fixture.panel(bar, vec![], ReaderApp::reading_seek);
        let circles = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Circle(circle) = &shape.shape {
                    Some(circle)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let dot = circles
            .iter()
            .find(|c| (c.radius - 3.5).abs() < 0.1)
            .unwrap();
        let handle = circles.iter().find(|c| c.radius > 8.).unwrap();
        assert!(
            dot.center.distance(handle.center) < 0.1,
            "history marker must use egui's actual slider center travel and vertical alignment in {mode:?}"
        );
        let dot_point = dot.center;
        fixture.app.seek_drag = None;
        fixture.panel(bar, vec![], ReaderApp::reading_seek);
        fixture.panel(
            bar,
            vec![
                egui::Event::PointerMoved(dot_point),
                egui::Event::PointerButton {
                    pos: dot_point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ReaderApp::reading_seek,
        );
        assert_eq!(
            fixture.app.chapter, 5,
            "pressing a dot must not apply a slider seek underneath it"
        );
        assert!(fixture.app.pending_page_index.is_none());
        assert!(fixture.app.seek_drag.is_none());
        fixture.panel_after(
            bar,
            vec![egui::Event::PointerButton {
                pos: dot_point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            0.03,
            ReaderApp::reading_seek,
        );
        assert_eq!(fixture.app.chapter, 1);
        assert_eq!(fixture.app.resume_locator, Some(target));
        assert!(
            fixture.app.pending_page_index.is_none(),
            "dot navigation restores its locator, not a simultaneous approximate slider page"
        );
        assert_eq!(
            fixture.app.jump_marks.len(),
            2,
            "only the origin of this explicit jump is added"
        );

        fixture.frame(size, vec![]);
        let rail = seek_slider_rect(Rect::from_min_size(Pos2::ZERO, bar));
        let start = seek_rail_position(rail, 0.05, rtl);
        let end = seek_rail_position(rail, 0.8, rtl);
        fixture.panel(
            bar,
            vec![egui::Event::PointerMoved(start)],
            ReaderApp::reading_seek,
        );
        fixture.panel(
            bar,
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ReaderApp::reading_seek,
        );
        fixture.panel(
            bar,
            vec![egui::Event::PointerMoved(dot_point)],
            ReaderApp::reading_seek,
        );
        fixture.panel(
            bar,
            vec![egui::Event::PointerMoved(end)],
            ReaderApp::reading_seek,
        );
        fixture.panel(
            bar,
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ReaderApp::reading_seek,
        );
        assert_eq!(
            fixture.app.chapter, 4,
            "a rail drag must continue across a history marker in {mode:?}"
        );
        assert!(fixture.app.seek_drag.is_none());
    }
}

#[test]
fn pagination_status_stays_on_the_single_seek_rail_without_inventing_progress() {
    let mut fixture = TestApp::new();
    fixture.app.pagination_index = None;
    fixture.app.pagination_pending = true;
    let bar = vec2(1280., READING_FOOTER_HEIGHT);
    fixture.panel(bar, vec![], ReaderApp::reading_seek);
    let output = fixture.panel(bar, vec![], ReaderApp::reading_seek);
    let status = text_bounds(&output, "全体のページ数を計算中…");
    let unknown = text_bounds(&output, "— / —");
    assert!((status.center().y - READING_FOOTER_HEIGHT * 0.5).abs() < 1.);
    assert!((status.center().y - unknown.center().y).abs() < 1.);
    assert!(status.top() >= 0. && status.bottom() <= bar.y);
    fixture.app.pagination_pending = false;
    fixture.app.pagination_error = Some("isolated pagination error".into());
    let output = fixture.panel(bar, vec![], ReaderApp::reading_seek);
    let error = text_bounds(&output, "ページ数を取得できません");
    assert!((error.center().y - READING_FOOTER_HEIGHT * 0.5).abs() < 1.);
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("計算中"))));
}

#[test]
fn toc_group_rows_keep_children_and_nonspine_entries_open_the_original_document() {
    use reader_core::{TocEntry, TocTarget};
    let mut fixture = TestApp::new();
    let path = linked_epub_fixture(fixture.directory.path());
    let original = std::fs::read(&path).unwrap();
    fixture.app.book = reader_core::open(&path).unwrap();
    fixture.app.chapter = 0;
    fixture.app.layout_key.clear();
    fixture.app.layout = BookLayout::default();
    fixture.app.drawer = Drawer::Toc;
    fixture.app.book.toc = vec![TocEntry {
        title: "リンクのない親見出し".into(),
        chapter: 0,
        fragment: None,
        target: Some(TocTarget::Group),
        children: vec![TocEntry {
            title: "本編外の補足章".into(),
            chapter: 2,
            fragment: Some("detail".into()),
            target: Some(TocTarget::Document {
                href: "/OPS/Notes/extra.xhtml#detail".into(),
            }),
            children: vec![],
        }],
    }];
    let size = vec2(272., 300.);
    fixture.panel(size, vec![], ReaderApp::drawer);
    let output = fixture.panel(size, vec![], ReaderApp::drawer);
    let group = text_bounds(&output, "リンクのない親見出し");
    let child = text_bounds(&output, "本編外の補足章");
    assert!((child.left() - group.left() - 12.).abs() < 1.);
    let request = fixture.app.link_request;
    fixture.panel(size, click(group.center()), ReaderApp::drawer);
    assert_eq!(
        fixture.app.link_request, request,
        "an unlinked grouping heading must not jump to chapter zero"
    );
    fixture.panel(size, click(child.center()), ReaderApp::drawer);
    assert_ne!(
        fixture.app.link_request, request,
        "non-spine TOC targets use the async document resolver"
    );
    finish_link_worker(&mut fixture);
    let document = fixture.app.auxiliary_document.as_ref().unwrap();
    assert_eq!(document.chapter.href, "OPS/Notes/extra.xhtml");
    assert!(
        document
            .chapter
            .text_at_anchor(document.locator)
            .unwrap()
            .contains("本編外の補足説明です")
    );
    assert_eq!(fixture.app.chapter, 0);
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn bookmark_search_and_toc_clicks_record_only_their_jump_origins() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let size = vec2(1280., 760.);
    let panel = vec2(272., 420.);
    let origin = fixture.app.current_book_locator().unwrap();
    let target = fixture.app.layout.pages[fixture.app.layout.pages.len() - 2]
        .first_locator()
        .unwrap();
    fixture
        .app
        .store
        .save_bookmark(&Bookmark::new(
            &fixture.app.book.id,
            target,
            "履歴を確認するブックマーク",
        ))
        .unwrap();
    fixture.app.drawer = Drawer::Bookmarks;
    fixture.panel(panel, vec![], ReaderApp::drawer);
    let output = fixture.panel(panel, vec![], ReaderApp::drawer);
    fixture.panel(
        panel,
        click(text_bounds(&output, "履歴を確認するブックマーク").center()),
        ReaderApp::drawer,
    );
    assert_eq!(fixture.app.pending_restore, Some(target));
    assert_eq!(
        fixture.app.jump_marks.last().map(|mark| mark.locator),
        Some(origin)
    );
    fixture.frame(size, vec![]);
    let bookmark_origin = fixture.app.current_book_locator().unwrap();
    let search_target = fixture.app.layout.pages[3].first_locator().unwrap();
    fixture.app.search_results = vec![reader_core::SearchHit {
        chapter: 0,
        block: 0,
        start: search_target,
        end: search_target,
        snippet: "履歴を確認する検索結果".into(),
    }];
    fixture.app.drawer = Drawer::Search;
    fixture.panel(panel, vec![], ReaderApp::drawer);
    let output = fixture.panel(panel, vec![], ReaderApp::drawer);
    fixture.panel(
        panel,
        click(text_bounds(&output, "履歴を確認する検索結果").center()),
        ReaderApp::drawer,
    );
    assert_eq!(fixture.app.pending_restore, Some(search_target));
    assert_eq!(
        fixture.app.jump_marks.last().map(|mark| mark.locator),
        Some(bookmark_origin)
    );
    fixture.frame(size, vec![]);
    let search_origin = fixture.app.current_book_locator().unwrap();
    fixture.app.book.toc = vec![reader_core::TocEntry {
        title: "履歴を確認する目次".into(),
        chapter: 0,
        fragment: None,
        target: None,
        children: vec![],
    }];
    fixture.app.drawer = Drawer::Toc;
    fixture.panel(panel, vec![], ReaderApp::drawer);
    let output = fixture.panel(panel, vec![], ReaderApp::drawer);
    fixture.panel(
        panel,
        click(text_bounds(&output, "履歴を確認する目次").center()),
        ReaderApp::drawer,
    );
    assert_eq!(fixture.app.page, 0);
    assert_eq!(
        fixture.app.jump_marks.last().map(|mark| mark.locator),
        Some(search_origin)
    );
}

#[test]
fn numeric_seek_within_a_chapter_discards_stale_source_navigation() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let count = fixture.app.layout.pages.len();
    let index = crate::pagination::PageIndex::new(vec![count]).unwrap();
    let stale = fixture.app.book.chapters[0].start_locator().unwrap();
    for target in [2, 3] {
        fixture.app.pagination_index = Some(index.clone());
        fixture.app.pending_restore = Some(stale);
        fixture.app.resume_locator = Some(stale);
        fixture.app.pending_fragment = Some("old-section".into());
        fixture.app.seek_book(index.fraction(0, target));
        assert_eq!(fixture.app.pending_page_index, Some(target));
        assert!(fixture.app.pending_restore.is_none());
        assert!(fixture.app.resume_locator.is_none());
        assert!(fixture.app.pending_fragment.is_none());
        fixture.frame(vec2(1280., 760.), vec![]);
        assert_eq!(fixture.app.page, target);
    }
}

#[test]
fn numeric_seek_survives_asynchronous_target_chapter_completion() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(2);
    let chapter = fixture.app.book.chapters[1].clone();
    fixture.app.book.chapters[1].loaded = false;
    let index = crate::pagination::PageIndex::new(vec![1, 8]).unwrap();
    fixture.app.pagination_index = Some(index.clone());
    fixture.app.seek_book(index.fraction(1, 5));
    assert_eq!(fixture.app.chapter, 1);
    fixture.app.chapter_request = 77;
    fixture.app.chapter_loading = Some(1);
    fixture
        .app
        .tx
        .send(Work::Chapter {
            book_generation: fixture.app.open_generation,
            request: 77,
            index: 1,
            result: Ok(chapter),
        })
        .unwrap();
    fixture.app.handle_work();
    assert!(fixture.app.chapter_loading.is_none());
    assert_eq!(fixture.app.pending_page_index, Some(5));
    assert!(fixture.app.pending_restore.is_none());
    assert!(fixture.app.resume_locator.is_none());
    assert!(fixture.app.pending_fragment.is_none());
}

#[test]
fn later_toc_and_bookmark_navigation_replace_pending_numeric_seeks() {
    let mut fixture = TestApp::new();
    fixture.image_fixture(3);
    fixture.app.pagination_index = Some(crate::pagination::PageIndex::new(vec![6, 6, 6]).unwrap());
    fixture.app.seek_book(0.5);
    assert!(fixture.app.pending_page_index.is_some());
    fixture.app.jump_to_chapter(2, None);
    assert_eq!(fixture.app.chapter, 2);
    assert!(fixture.app.pending_page_index.is_none());
    assert!(fixture.app.pending_page_fraction.is_none());
    assert_eq!(fixture.app.pending_restore.unwrap().chapter, 2);
    fixture.app.pagination_index = None;
    fixture.app.seek_book(0.2);
    assert_eq!(fixture.app.pending_book_seek, Some(0.2));
    let bookmark = fixture.app.book.chapters[0].start_locator().unwrap();
    fixture.app.restore_locator(bookmark);
    assert!(fixture.app.pending_book_seek.is_none());
    assert!(fixture.app.pending_page_index.is_none());
    assert!(fixture.app.pending_page_fraction.is_none());
    assert_eq!(fixture.app.pending_restore, Some(bookmark));
    assert_eq!(fixture.app.resume_locator, Some(bookmark));
}

#[test]
fn repeated_seeks_before_a_target_is_painted_do_not_record_provisional_locations() {
    let mut fixture = TestApp::new();
    fixture.reading_fixture();
    let source = fixture.app.current_book_locator().unwrap();
    fixture.app.seek_book(0.9);
    assert!(fixture.app.pending_page_index.is_some());
    fixture.app.seek_book(0.2);
    assert_eq!(
        fixture.app.jump_marks,
        vec![JumpMark { locator: source }],
        "unpainted seek targets are not positions the user read"
    );
    fixture.frame(vec2(1280., 760.), vec![]);
    assert!(fixture.app.pending_page_index.is_none());
}

#[test]
fn custom_pronunciations_keep_cross_atom_spelling_but_preserve_unregistered_ruby() {
    let chapter=reader_core::parse_html("<p><ruby>東京<rt>とうきょう</rt></ruby>大学へ。<ruby>京都<rt>きょうと</rt></ruby>も読む。</p>",0,"ruby-custom.xhtml","").unwrap();
    let original = narration_texts(&chapter, &Default::default()).concat();
    assert!(original.contains("とうきょう大学") && original.contains("きょうと"));
    let rules = [("東京大学".into(), "とうだい".into())]
        .into_iter()
        .collect();
    let registered = narration_texts(&chapter, &rules).concat();
    assert!(
        registered.contains("東京大学"),
        "the engine must receive a registered phrase's original spelling across ruby/normal atoms"
    );
    assert!(
        registered.contains("きょうと"),
        "unregistered ruby keeps its authored reading"
    );
    assert!(!registered.contains("とうきょう大学"));
}

#[test]
fn narration_plan_keeps_canonical_chapter_units_when_starting_inside_a_page() {
    let mut fixture = TestApp::new();
    let chapter = reader_core::parse_html(
        &format!(
            "<body><p>{}</p><p><ruby>東京<rt>とうきょう</rt></ruby>大学へ。</p></body>",
            "長い段落の途中から読み始めます。".repeat(60)
        ),
        0,
        "speech-plan.xhtml",
        "",
    )
    .unwrap();
    fixture.use_chapter(chapter);
    fixture.render_reader(vec2(320., 240.));
    fixture.app.turn_page(true);
    fixture.render_reader(vec2(320., 240.));
    let hit = fixture.app.speech_start_hit().unwrap();
    assert!(
        hit.offset > 0,
        "the page begins inside the original paragraph"
    );
    let chapter = &fixture.app.book.chapters[0];
    let rules = [("東京大学".into(), "とうだい".into())]
        .into_iter()
        .collect();
    let complete = chapter_narration(chapter, &rules, None);
    let selected = chapter_narration(chapter, &rules, Some(&hit));
    assert_eq!(
        complete.units, selected.units,
        "prefetch and playback must share full chapter chunk boundaries"
    );
    assert_eq!(selected.units[selected.start_index].id, hit.unit_id);
    assert_eq!(selected.byte_offset, hit.offset);
    assert!(selected.units.iter().any(|unit| unit.text == "東京"));
    assert!(!selected.units.iter().any(|unit| unit.text == "とうきょう"));
    assert!(
        fixture.app.speech.is_none(),
        "planning never starts a provider or playback"
    );
}

#[test]
#[ignore = "Manual operation latency probe; native egui CPU frames, no hardware presentation or TTS"]
fn operation_latency_probe() {
    let initialize = Instant::now();
    let mut fixture = TestApp::new();
    eprintln!("operation,initialize,{}", initialize.elapsed().as_micros());
    let size = fixture.wheel_fixture();
    for _ in 0..3 {
        fixture.frame(size, vec![]);
    }
    for turn in 0..24 {
        let start = Instant::now();
        fixture.app.turn_page(true);
        let dispatch = start.elapsed().as_micros();
        fixture.frame(size, vec![]);
        eprintln!(
            "operation,page_turn,{turn},{dispatch},{}",
            start.elapsed().as_micros()
        );
    }
}

#[test]
fn page_turn_persists_the_new_page_and_keeps_a_real_origin_while_loading_next_chapter() {
    let mut fixture = TestApp::new();
    fixture.wheel_fixture();
    fixture.app.turn_page(true);
    let next = fixture.app.layout.pages[fixture.app.page]
        .first_locator()
        .unwrap();
    let saved = fixture
        .app
        .store
        .position(&fixture.app.book.id)
        .unwrap()
        .unwrap();
    assert_eq!(saved.locator, next);
    fixture.app.page = fixture.app.layout.pages.len() - 1;
    let origin = fixture.app.layout.pages[fixture.app.page]
        .first_locator()
        .unwrap();
    let mut following = fixture.app.book.chapters[0].clone();
    following.id = 1;
    following.loaded = false;
    following.blocks.clear();
    fixture.app.book.chapters.push(following);
    fixture.app.turn_page(true);
    assert_eq!(fixture.app.chapter, 1);
    assert!(fixture.app.layout.pages.is_empty());
    let saved = fixture
        .app
        .store
        .position(&fixture.app.book.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.locator, origin,
        "unlaid chapters never save a fabricated source position"
    );
}

#[test]
fn nvidia_permission_is_explicit_persistent_revocable_and_does_not_start_audio() {
    let mut fixture = TestApp::new();
    fixture.app.speech_provider = "kokoro".into();
    fixture.app.load_profile_fields();
    fixture.app.speech_voice = "jm_kumo".into();
    let before = serde_json::to_string(&fixture.app.provider_config()).unwrap();
    assert!(matches!(
        fixture.app.provider_config(),
        ProviderConfig::Kokoro {
            allow_nvidia_download: false,
            ..
        }
    ));
    let size = vec2(780., 920.);
    fixture.panel(size, vec![], ReaderApp::speech_settings);
    let output = fixture.panel(size, vec![], ReaderApp::speech_settings);
    let label = text_bounds(&output, "利用条件に同意してGPUランタイムの取得を許可する");
    assert!(label.left() >= 0. && label.right() <= size.x);
    fixture.panel(size, click(label.center()), ReaderApp::speech_settings);
    assert!(
        matches!(fixture.app.provider_config(), ProviderConfig::Kokoro { allow_nvidia_download:true, voice, .. } if voice == "jm_kumo")
    );
    let after = serde_json::to_string(&fixture.app.provider_config()).unwrap();
    assert_ne!(
        before, after,
        "the serialized provider key must invalidate synthesis and prefetch state"
    );
    fixture
        .app
        .store
        .save_settings(&fixture.app.settings)
        .unwrap();
    let reloaded = LibraryStore::open(fixture.app.store.data_dir()).unwrap();
    let profiles = crate::speech_profiles::SpeechProfiles::load(reloaded.settings());
    assert!(matches!(
        profiles.get("kokoro"),
        ProviderConfig::Kokoro {
            allow_nvidia_download: true,
            ..
        }
    ));
    assert!(
        fixture.app.speech.is_none(),
        "giving permission must not start audio or download workers"
    );
    // Creating idle command workers opens neither an audio device nor a model.
    // Revocation must invalidate a controller already holding the old config.
    assert!(fixture.app.ensure_speech());
    assert!(fixture.app.speech.is_some());
    fixture.app.prefetched_audio = Some("previous-permission-prefetch".into());
    fixture.panel(size, click(label.center()), ReaderApp::speech_settings);
    assert!(matches!(
        fixture.app.provider_config(),
        ProviderConfig::Kokoro {
            allow_nvidia_download: false,
            ..
        }
    ));
    assert!(
        fixture.app.speech.is_none(),
        "revocation must discard the controller holding the previous permission"
    );
    assert!(fixture.app.speech_config_key.is_empty());
    assert!(fixture.app.prefetched_audio.is_none());
}

#[test]
fn appearance_hex_colors_persist_and_a_preset_clears_overrides_in_the_live_preview() {
    let mut fixture = TestApp::new();
    fixture.app.setting_tab = SettingTab::Appearance;
    fixture.app.screen = Screen::Settings;
    let size = vec2(820., 1800.);
    fixture.panel(size, vec![], ReaderApp::settings_view);
    let output = fixture.panel(size, vec![], ReaderApp::settings_view);
    let hex = text_bounds(&output, "#FAFAF7");
    fixture.panel(size, click(hex.center()), ReaderApp::settings_view);
    let select_all = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    fixture.panel(
        size,
        key(egui::Key::A, select_all),
        ReaderApp::settings_view,
    );
    fixture.panel(
        size,
        vec![egui::Event::Text("#102030".into())],
        ReaderApp::settings_view,
    );
    assert_eq!(fixture.app.settings.custom_background, Some([16, 32, 48]));
    let stored = LibraryStore::open(fixture.app.store.data_dir()).unwrap();
    assert_eq!(stored.settings().custom_background, Some([16, 32, 48]));
    fixture.app.settings.custom_foreground = Some([220, 225, 230]);
    let output = fixture.panel(size, vec![], ReaderApp::settings_view);
    let nord = text_bounds(&output, "Nord");
    fixture.panel(size, click(nord.center()), ReaderApp::settings_view);
    assert_eq!(fixture.app.settings.theme, Theme::Nord);
    assert!(
        fixture.app.settings.custom_background.is_none()
            && fixture.app.settings.custom_foreground.is_none()
    );
    let palette = Palette::for_settings(&fixture.app.settings);
    assert_eq!(palette.paper, egui::Color32::from_rgb(46, 52, 64));
    let output = fixture.panel(size, vec![], |app, ui, p| app.settings_view(ui, p));
    assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Rect(rect) if rect.fill==palette.paper && rect.rect.width()>300.)),"live native surfaces use the selected preset paper");
}

/// Metadata-only AIVMX: the real catalog reads this bounded protobuf without
/// loading an ONNX session, neural weights, a runtime, or an audio device.
fn native_aivis_layout_fixture() -> (TestApp, String, String, String) {
    fn varint(mut value: usize, out: &mut Vec<u8>) {
        while value >= 128 {
            out.push((value as u8) | 128);
            value >>= 7;
        }
        out.push(value as u8);
    }
    fn message(field: usize, bytes: &[u8], out: &mut Vec<u8>) {
        varint((field << 3) | 2, out);
        varint(bytes.len(), out);
        out.extend_from_slice(bytes);
    }
    let mut fixture = TestApp::new();
    let name = format!("UI catalog model · {}", "長い日本語モデル名".repeat(18));
    let speaker_a = format!(
        "UI speaker alpha · {}",
        "落ち着いた日本語の話者名".repeat(16)
    );
    let speaker_b = format!("UI speaker beta · {}", "別の長い日本語の話者名".repeat(16));
    let manifest = serde_json::json!({
        "manifest_version":"1.0", "uuid":"12345678-1234-4234-9234-123456789abc",
        "name":name, "version":"1.0.0-ui-geometry-fixture", "model_architecture":"Style-Bert-VITS2 (JP-Extra)",
        "model_format":"ONNX", "license":"UI fixture terms; no real voice or model weights are included.",
        "speakers":[
            {"local_id":17,"name":speaker_a,"styles":[{"local_id":8,"name":"UI style alpha · 通常"},{"local_id":19,"name":"UI style alpha · 明瞭"}]},
            {"local_id":63,"name":speaker_b,"styles":[{"local_id":104,"name":format!("UI style beta calm · {}","長いスタイル名".repeat(20))},{"local_id":209,"name":format!("UI style beta bright · {}","もう一つの長いスタイル名".repeat(16))}]}
        ]
    });
    let mut entry = Vec::new();
    message(1, b"aivm_manifest", &mut entry);
    message(2, manifest.to_string().as_bytes(), &mut entry);
    let mut model = vec![8, 8]; // ONNX IR 8; a graph envelope sufficient for catalog metadata parsing.
    message(7, &[0], &mut model);
    message(14, &entry, &mut model);
    let path = fixture.directory.path().join("metadata-only-ui.aivmx");
    std::fs::write(&path, model).unwrap();
    fixture.app.speech_provider = "aivis_native".into();
    fixture.app.setting_tab = SettingTab::Speech;
    fixture.app.screen = Screen::Settings;
    fixture
        .app
        .set_aivis_native_config(ProviderConfig::AivisNative {
            runtime_dir: fixture.directory.path().join("unprepared-runtime"),
            model_path: path,
            bert_dir: fixture
                .directory
                .path()
                .join("very-long-japanese-bert-folder")
                .join("長いフォルダー名と空白を含む BERT data ".repeat(12)),
            speaker: 17,
            style: 8,
            backend: "cpu".into(),
            auto_download: false,
            allow_nvidia_download: false,
        });
    (fixture, name, speaker_a, speaker_b)
}

fn native_aivis_catalog_ui(fixture: &mut TestApp, size: egui::Vec2) -> egui::FullOutput {
    fn contains(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.job.text.starts_with("UI catalog model"),
            egui::Shape::Vec(shapes) => shapes.iter().any(contains),
            _ => false,
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = fixture.panel(size, vec![], ReaderApp::settings_view);
        if output.shapes.iter().any(|shape| contains(&shape.shape)) {
            return output;
        }
        assert!(
            Instant::now() < deadline,
            "local metadata catalog did not reach native settings"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn native_aivis_long_catalog_names_keep_model_speaker_and_style_selection_inside_settings() {
    fn matching_text_bounds(shape: &egui::Shape, prefix: &str, out: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.job.text.starts_with(prefix) => {
                assert_eq!(text.galley.rows.len(), 1);
                out.push(Rect::from_min_size(text.pos, text.galley.size()));
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    matching_text_bounds(shape, prefix, out);
                }
            }
            _ => (),
        }
    }
    for width in [820., 1280.] {
        let (mut fixture, _, _, _) = native_aivis_layout_fixture();
        let size = vec2(width, 1800.);
        let output = native_aivis_catalog_ui(&mut fixture, size);
        for prefix in ["UI catalog model", "UI speaker alpha", "UI style alpha"] {
            let bounds = text_bounds(&output, prefix);
            assert!(
                bounds.left() >= 228. && bounds.right() <= width - 24. + 0.5,
                "{prefix} escaped the native settings content at window width {width}: {bounds:?}"
            );
        }
        fixture.panel(
            size,
            click(text_bounds(&output, "UI catalog model").center()),
            ReaderApp::settings_view,
        );
        let menu = fixture.panel(size, vec![], ReaderApp::settings_view);
        let mut choices = Vec::new();
        for shape in &menu.shapes {
            matching_text_bounds(&shape.shape, "UI catalog model", &mut choices);
        }
        assert!(
            choices.len() >= 2,
            "the model popup must display the actual metadata entry"
        );
        let choice = choices.last().unwrap();
        assert!(
            choice.right() <= width + 0.5,
            "long model choices must remain inside the window"
        );
        fixture.panel(size, click(choice.center()), ReaderApp::settings_view);
        let output = fixture.panel(size, vec![], ReaderApp::settings_view);
        let selected = text_bounds(&output, "UI speaker alpha");
        fixture.panel(size, click(selected.center()), ReaderApp::settings_view);
        let menu = fixture.panel(size, vec![], ReaderApp::settings_view);
        let choice = text_bounds(&menu, "UI speaker beta");
        assert!(
            choice.right() <= width + 0.5,
            "long speaker choices must stay inside the window"
        );
        fixture.panel(size, click(choice.center()), ReaderApp::settings_view);
        assert!(matches!(
            fixture.app.provider_config(),
            ProviderConfig::AivisNative {
                speaker: 63,
                style: 104,
                ..
            }
        ));
        let output = fixture.panel(size, vec![], ReaderApp::settings_view);
        let selected = text_bounds(&output, "UI style beta calm");
        assert!(selected.right() <= width - 24. + 0.5);
        fixture.panel(size, click(selected.center()), ReaderApp::settings_view);
        let menu = fixture.panel(size, vec![], ReaderApp::settings_view);
        let choice = text_bounds(&menu, "UI style beta bright");
        assert!(
            choice.right() <= width + 0.5,
            "long style choices must stay inside the window"
        );
        fixture.panel(size, click(choice.center()), ReaderApp::settings_view);
        assert!(matches!(
            fixture.app.provider_config(),
            ProviderConfig::AivisNative {
                speaker: 63,
                style: 209,
                ..
            }
        ));
        assert!(
            fixture.app.speech.is_none(),
            "catalog selection must not synthesize or download models"
        );
        assert!(!fixture.directory.path().join("unprepared-runtime").exists());
    }
}

#[test]
fn native_aivis_runtime_path_and_consent_controls_stay_clickable_inside_settings() {
    for width in [820., 1280.] {
        let (mut fixture, _, _, _) = native_aivis_layout_fixture();
        let size = vec2(width, 1800.);
        let output = native_aivis_catalog_ui(&mut fixture, size);
        let header = text_bounds(&output, "実行設定");
        fixture.panel(size, click(header.center()), ReaderApp::settings_view);
        let output = fixture.panel(size, vec![], ReaderApp::settings_view);
        let pick = text_bounds(&output, "フォルダーを選択");
        assert!(
            pick.left() >= 228. && pick.right() <= width - 24. + 0.5,
            "the BERT picker must remain visible beside a long path at window width {width}: {pick:?}"
        );
        let ProviderConfig::AivisNative { bert_dir, .. } = fixture.app.provider_config() else {
            panic!("native provider changed")
        };
        let path = text_bounds(&output, &bert_dir.display().to_string());
        assert!(
            path.right() + 4. <= pick.left(),
            "the clipped path must not draw underneath the picker"
        );
        for (prefix, nvidia) in [
            ("不足するモデル・ランタイムを自動取得する", false),
            (
                "利用条件に同意してNVIDIA GPUランタイムの取得を許可する",
                true,
            ),
        ] {
            let output = fixture.panel(size, vec![], ReaderApp::settings_view);
            let control = text_bounds(&output, prefix);
            assert!(control.left() >= 228. && control.right() <= width - 24. + 0.5);
            fixture.panel(size, click(control.center()), ReaderApp::settings_view);
            let ProviderConfig::AivisNative {
                auto_download,
                allow_nvidia_download,
                ..
            } = fixture.app.provider_config()
            else {
                panic!("native provider changed")
            };
            if nvidia {
                assert!(allow_nvidia_download);
            } else {
                assert!(auto_download);
            }
        }
        assert!(fixture.app.speech.is_none());
        assert!(!fixture.directory.path().join("unprepared-runtime").exists());
    }
}
