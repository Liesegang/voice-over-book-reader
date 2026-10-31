//! Opt-in SAPI follow check. Only counters and source IDs leave this module.
use eframe::egui;
use reader_core::Locator;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(crate) struct PaneSample {
    pub chapter: usize,
    pub page: usize,
    pub requested_page: usize,
    pub revision: u64,
    pub size: [u32; 2],
    pub settled: bool,
    pub marker_xy: [f32; 2],
    pub marker: [u8; 3],
}
pub(crate) struct Frame<'a> {
    pub ready: bool,
    pub total: Option<usize>,
    pub layout: &'a str,
    pub chapter: usize,
    pub page: usize,
    pub error: bool,
    pub panes: Vec<PaneSample>,
}
pub(crate) enum Action {
    Start,
    Close,
}
#[derive(Default, Serialize)]
struct Counts {
    boundaries: u64,
    native_page_overwrites: u64,
    committed_checks: u64,
    page_mismatches: u64,
    prose_pair_checks: u64,
    prose_pair_mismatches: u64,
    layout_changes: u64,
    error_frames: u64,
}
pub(crate) struct Driver {
    output: PathBuf,
    deadline: Instant,
    started: Option<Instant>,
    baseline: Option<(String, usize)>,
    layouts: BTreeMap<(usize, usize), (u64, [u32; 2])>,
    counts: Counts,
    sources: BTreeSet<Locator>,
    settled_positions: BTreeSet<[usize; 2]>,
    captured: Arc<Mutex<[u64; 4]>>,
    last_capture: Instant,
    done: bool,
}
impl Driver {
    #[rustfmt::skip]
    pub(crate) fn from_args(args: &[String]) -> anyhow::Result<Option<Self>> {
        let Some(at) = args.iter().position(|arg| arg == "--speech-smoke-output") else {
            return Ok(None);
        };
        let isolated = args.iter().position(|arg| arg == "--data-dir").and_then(|at| args.get(at+1));
        anyhow::ensure!(isolated.is_some(), "Speech smoke requires an explicit isolated --data-dir");
        anyhow::ensure!(!args.iter().any(|arg| arg == "--perf-output"), "Speech smoke cannot run with the performance driver");
        let output = PathBuf::from(args.get(at+1).ok_or_else(|| anyhow::anyhow!("Speech smoke output is required"))?);
        Ok(Some(Self {
            output,
            deadline: Instant::now() + Duration::from_secs(120),
            started: None,
            baseline: None,
            layouts: BTreeMap::new(),
            counts: Counts::default(),
            sources: BTreeSet::new(),
            settled_positions: BTreeSet::new(),
            captured: Arc::new(Mutex::new([0; 4])),
            last_capture: Instant::now(),
            done: false,
        }))
    }
    pub(crate) fn boundary(&mut self, source: Locator, before: [usize; 2], after: [usize; 2]) {
        if self.started.is_none() {
            return;
        }
        self.counts.boundaries += 1;
        self.counts.native_page_overwrites +=
            u64::from(before[0] == after[0] && before[1] != after[1]);
        if self.sources.len() < 256 {
            self.sources.insert(source);
        }
    }
    pub(crate) fn tick(&mut self, ctx: &egui::Context, frame: Frame<'_>) -> Option<Action> {
        if self.done {
            return None;
        }
        self.counts.error_frames += u64::from(frame.error);
        if self.started.is_none() {
            if self.counts.error_frames > 0 || Instant::now() >= self.deadline {
                return Some(self.finish());
            }
            if frame.ready
                && let Some(total) = frame.total
            {
                self.started = Some(Instant::now());
                self.baseline = Some((frame.layout.to_owned(), total));
                return Some(Action::Start);
            }
            ctx.request_repaint_after(Duration::from_millis(16));
            return None;
        }
        if let Some(primary) = frame
            .panes
            .first()
            .filter(|pane| frame.ready && pane.settled)
        {
            self.counts.committed_checks += 1;
            self.settled_positions
                .insert([primary.chapter, primary.page]);
            self.counts.page_mismatches += u64::from(
                (primary.chapter, primary.page) != (frame.chapter, frame.page)
                    || primary.page != primary.requested_page,
            );
            if self
                .baseline
                .as_ref()
                .is_some_and(|(key, total)| key != frame.layout || Some(*total) != frame.total)
            {
                self.counts.layout_changes += 1;
            }
            for (slot, pane) in frame.panes.iter().enumerate() {
                if !pane.settled {
                    continue;
                }
                let value = (pane.revision, pane.size);
                if let Some(old) = self.layouts.insert((pane.chapter, slot), value)
                    && old != value
                {
                    self.counts.layout_changes += 1;
                }
            }
        }
        let pair = frame.ready
            && frame.panes.len() == 2
            && frame.panes.iter().all(|pane| pane.settled)
            && frame.panes[0].chapter == frame.panes[1].chapter;
        if pair {
            self.counts.prose_pair_checks += 1;
            self.counts.prose_pair_mismatches +=
                u64::from(frame.panes[1].page != frame.panes[0].page + 1);
        }
        if frame.panes.iter().any(|pane| pane.settled)
            && self.last_capture.elapsed() >= Duration::from_millis(500)
        {
            self.last_capture = Instant::now();
            let scale = ctx.pixels_per_point();
            let targets = frame
                .panes
                .iter()
                .filter(|pane| pane.settled)
                .map(|pane| (pane.marker_xy, pane.marker))
                .collect::<Vec<_>>();
            let result = Arc::clone(&self.captured);
            ctx.request_screenshot(move |image| {
                let valid = targets
                    .iter()
                    .map(|(point, rgb)| {
                        let [x, y] = point.map(|v| (v * scale).round() as i32);
                        if x < 1
                            || y < 1
                            || x as usize + 1 >= image.width()
                            || y as usize + 1 >= image.height()
                        {
                            return false;
                        }
                        image
                            .region_by_pixels([x as usize - 1, y as usize - 1], [3, 3])
                            .pixels
                            .iter()
                            .filter(|pixel| {
                                pixel.to_array()[..3]
                                    .iter()
                                    .zip(rgb)
                                    .all(|(a, b)| a.abs_diff(*b) <= 8)
                            })
                            .count()
                            >= 5
                    })
                    .collect::<Vec<_>>();
                let mut counts = result.lock().unwrap();
                counts[0] += 1;
                counts[1] += valid.iter().filter(|yes| **yes).count() as u64;
                if pair {
                    counts[2] += 1;
                    counts[3] += u64::from(
                        valid.iter().all(|yes| *yes) && targets[0].1[..2] != targets[1].1[..2],
                    );
                }
            });
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        if self
            .started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(15))
        {
            Some(self.finish())
        } else {
            None
        }
    }
    #[rustfmt::skip]
    fn finish(&mut self) -> Action {
        self.done = true;
        let gpu = *self.captured.lock().unwrap();
        let c = &self.counts;
        let failures = c.native_page_overwrites+c.page_mismatches+c.prose_pair_mismatches+c.layout_changes+c.error_frames;
        let success = c.boundaries>=2 && c.committed_checks>0 && self.settled_positions.len()>=2 && failures==0
            && gpu[1]>0 && (c.prose_pair_checks==0 || gpu[3]>0);
        let report = serde_json::json!({"success":success,"system_provider":true,"speed":2.0,
            "counts":c,"source_ids":self.sources,"settled_positions":self.settled_positions,"gpu_readbacks":gpu[0],"gpu_valid_pane_markers":gpu[1],
            "gpu_pair_checks":gpu[2],"gpu_distinct_pairs":gpu[3],"total_pages":self.baseline.as_ref().map(|(_,total)|total)});
        let written = (|| -> anyhow::Result<()> {
            if let Some(parent) = self.output.parent().filter(|path| !path.as_os_str().is_empty()) { std::fs::create_dir_all(parent)?; }
            std::fs::write(&self.output, serde_json::to_vec_pretty(&report)?)?;
            Ok(())
        })();
        if written.is_err() {
            eprintln!("Speech smoke report could not be written");
        }
        Action::Close
    }
}
