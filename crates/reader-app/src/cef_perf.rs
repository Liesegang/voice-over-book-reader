//! Opt-in, actual GPU screenshot/readback timing for the native book window.
//! Results contain counters and hashes only. No screenshots, book text or paths
//! are written. Callback completion is not a display-photon timestamp.
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerfAction {
    Turn(bool),
    WheelBurst(usize),
    ToggleChrome,
    Close,
}

#[derive(Clone, Copy)]
enum Clock {
    Local(Instant),
    #[cfg(windows)]
    ParentQpc {
        start: i64,
        frequency: i64,
    },
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(value: *mut i64) -> i32;
    fn QueryPerformanceFrequency(value: *mut i64) -> i32;
}

impl Clock {
    fn new() -> Self {
        #[cfg(windows)]
        {
            let start = std::env::var("VOBR_PERF_LAUNCH_QPC")
                .or_else(|_| std::env::var("BOOK_PROBE_LAUNCH_QPC"))
                .ok()
                .and_then(|value| value.parse::<i64>().ok());
            let mut frequency = 0;
            // SAFETY: the Windows API writes one i64 to this owned pointer.
            let queried = unsafe { QueryPerformanceFrequency(&mut frequency) };
            if let Some(start) = start.filter(|value| *value > 0)
                && queried != 0
                && frequency > 0
            {
                return Self::ParentQpc { start, frequency };
            }
        }
        Self::Local(Instant::now())
    }

    fn ms(self) -> f64 {
        match self {
            Self::Local(start) => start.elapsed().as_secs_f64() * 1000.,
            #[cfg(windows)]
            Self::ParentQpc { start, frequency } => {
                let mut now = 0;
                // SAFETY: the Windows API writes one i64 to this owned pointer.
                unsafe { QueryPerformanceCounter(&mut now) };
                (now - start).max(0) as f64 * 1000. / frequency as f64
            }
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Local(_) => "driver creation; excludes process/bootstrap startup",
            #[cfg(windows)]
            Self::ParentQpc { .. } => {
                "parent launch QPC; includes bootstrap and native engine startup"
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Initial,
    WaitCount,
    Baseline,
    OverlayOn,
    OverlayOff,
    Turns,
    Burst,
    Finished,
}

struct Capture {
    chapter: usize,
    page: usize,
    callback_ms: f64,
    marker_matches: bool,
    expected_rgb: [u8; 3],
    sampled_rgb: [u8; 3],
    matching_pixels: usize,
    image_size: [usize; 2],
    right_book_hash: String,
    toolbar_hash: String,
}

struct PendingTurn {
    chapter: usize,
    target: usize,
    dispatched_ms: f64,
    dispatched_frame: u64,
}

struct BurstCapture {
    ordinal: u64,
    frame: u64,
    chapter: usize,
    requested_ms: f64,
    callback_ms: f64,
    observed_page: Option<usize>,
    sampled_rgb: [u8; 3],
    size: [usize; 2],
}

pub(crate) struct PerfDriver {
    output: PathBuf,
    requested_backend: &'static str,
    clock: Clock,
    deadline: Instant,
    phase: Phase,
    after_frame: u64,
    captured: Arc<Mutex<Option<Capture>>>,
    capture_pending: bool,
    initial_chapter: Option<usize>,
    initial_page: Option<usize>,
    first_frame_ms: Option<f64>,
    total_ready_ms: Option<f64>,
    total_pages: Option<usize>,
    initial_hash: Option<String>,
    initial_toolbar_hash: Option<String>,
    overlay_checks: Vec<Value>,
    sequence: VecDeque<usize>,
    pending_turn: Option<PendingTurn>,
    turns: Vec<Value>,
    rejected_captures: usize,
    marker_samples: Vec<Value>,
    count_only: bool,
    burst_started_ms: Option<f64>,
    burst_completed_ms: Option<f64>,
    burst_chapter: usize,
    burst_blue: u8,
    burst_captures: Arc<Mutex<Vec<BurstCapture>>>,
    burst_buffer: BTreeMap<u64, BurstCapture>,
    burst_next_request: u64,
    burst_next_result: u64,
    burst_last_frame: Option<u64>,
    burst_observed: Vec<usize>,
    burst_transitions: Vec<Value>,
    burst_frames: Vec<Value>,
}

impl PerfDriver {
    pub(crate) fn from_args(args: &[String]) -> Option<Self> {
        let at = args
            .iter()
            .position(|argument| argument == "--perf-output")?;
        let output = PathBuf::from(args.get(at + 1)?);
        Some(Self {
            output,
            requested_backend: if args.iter().any(|argument| argument == "--native-renderer") {
                "native"
            } else {
                "chromium"
            },
            clock: Clock::new(),
            deadline: Instant::now() + Duration::from_secs(120),
            phase: Phase::Initial,
            after_frame: 0,
            captured: Arc::new(Mutex::new(None)),
            capture_pending: false,
            initial_chapter: None,
            initial_page: None,
            first_frame_ms: None,
            total_ready_ms: None,
            total_pages: None,
            initial_hash: None,
            initial_toolbar_hash: None,
            overlay_checks: Vec::new(),
            sequence: (0..3)
                .flat_map(|_| [1, 2, 3, 4, 5, 4, 3, 2, 1, 0])
                .collect(),
            pending_turn: None,
            turns: Vec::new(),
            rejected_captures: 0,
            marker_samples: Vec::new(),
            count_only: args.iter().any(|argument| argument == "--perf-count-only"),
            burst_started_ms: None,
            burst_completed_ms: None,
            burst_chapter: 0,
            burst_blue: 128,
            burst_captures: Arc::new(Mutex::new(Vec::new())),
            burst_buffer: BTreeMap::new(),
            burst_next_request: 0,
            burst_next_result: 0,
            burst_last_frame: None,
            burst_observed: Vec::new(),
            burst_transitions: Vec::new(),
            burst_frames: Vec::new(),
        })
    }

    /// Call after the book and native chrome are painted for this UI frame.
    /// The driver returns actions for the host's existing action dispatcher.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tick(
        &mut self,
        ctx: &egui::Context,
        physical_size: [usize; 2],
        expected_marker: Option<[u8; 3]>,
        current_chapter: usize,
        current_page: usize,
        total_pages: Option<usize>,
        ready: bool,
    ) -> Option<PerfAction> {
        if self.phase == Phase::Finished {
            return None;
        }
        if Instant::now() >= self.deadline {
            self.finish(Some(
                "Timed out waiting for confirmed book pixels or total pages",
            ));
            return Some(PerfAction::Close);
        }
        if ready && let Some(total) = total_pages.filter(|total| *total > 0) {
            self.total_pages = Some(total);
            self.total_ready_ms.get_or_insert_with(|| self.clock.ms());
        }
        let capture = self
            .captured
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(capture) = capture {
            self.capture_pending = false;
            let same_book = self.initial_hash.as_ref() == Some(&capture.right_book_hash);
            let same_toolbar = self.initial_toolbar_hash.as_ref() == Some(&capture.toolbar_hash);
            // The native toolbar legitimately covers the diagnostic marker.
            // In that phase verify the visible book plus the changed toolbar;
            // the subsequent hidden-chrome capture must confirm the marker again.
            let marker_valid = capture.marker_matches
                || (self.phase == Phase::OverlayOn && same_book && !same_toolbar);
            let valid = marker_valid
                && (self.phase != Phase::OverlayOn || !same_toolbar)
                && (self.phase != Phase::OverlayOff || same_toolbar)
                && capture.image_size == physical_size
                && capture.chapter == current_chapter
                && capture.page == current_page;
            if !valid {
                self.rejected_captures += 1;
            } else {
                self.marker_samples.push(json!({
                    "chapter":capture.chapter, "page":capture.page,
                    "callback_ms":capture.callback_ms,
                    "expected_rgb":capture.expected_rgb, "sampled_rgb":capture.sampled_rgb,
                    "matching_pixels":capture.matching_pixels,
                    "native_chrome_occluded_marker":self.phase==Phase::OverlayOn && !capture.marker_matches,
                    "image_size":capture.image_size,
                }));
                match self.phase {
                    Phase::Initial => {
                        self.initial_chapter = Some(current_chapter);
                        self.initial_page = Some(current_page);
                        self.first_frame_ms = Some(capture.callback_ms);
                        self.initial_hash = Some(capture.right_book_hash);
                        self.initial_toolbar_hash = Some(capture.toolbar_hash);
                        self.phase = Phase::WaitCount;
                    }
                    Phase::Baseline => {
                        // Pagination can replace its status label between the
                        // first book frame and this point. Compare chrome with
                        // a fresh hidden baseline after that state is stable.
                        self.initial_hash = Some(capture.right_book_hash);
                        self.initial_toolbar_hash = Some(capture.toolbar_hash);
                        self.phase = Phase::OverlayOn;
                        self.after_frame = ctx.cumulative_frame_nr() + 1;
                        return Some(PerfAction::ToggleChrome);
                    }
                    Phase::OverlayOn | Phase::OverlayOff => {
                        self.overlay_checks.push(json!({
                            "chrome_visible":self.phase==Phase::OverlayOn,
                            "page":current_page,
                            "right_book_pixels_unchanged":self.initial_hash.as_ref()==Some(&capture.right_book_hash),
                            "toolbar_state_verified":if self.phase==Phase::OverlayOn{!same_toolbar}else{same_toolbar},
                            "right_book_hash":capture.right_book_hash,
                        }));
                        if self.phase == Phase::OverlayOn {
                            self.phase = Phase::OverlayOff;
                            self.after_frame = ctx.cumulative_frame_nr() + 1;
                            return Some(PerfAction::ToggleChrome);
                        }
                        self.phase = Phase::Turns;
                    }
                    Phase::Turns => {
                        if let Some(pending) = self.pending_turn.take() {
                            if capture.chapter != pending.chapter || capture.page != pending.target
                            {
                                self.pending_turn = Some(pending);
                            } else {
                                self.turns.push(json!({
                                    "chapter":capture.chapter, "page":capture.page,
                                    "dispatch_ms":pending.dispatched_ms,
                                    "screenshot_callback_ms":capture.callback_ms,
                                    "dispatch_to_readback_ms":capture.callback_ms-pending.dispatched_ms,
                                }));
                            }
                        }
                    }
                    Phase::WaitCount | Phase::Burst | Phase::Finished => {}
                }
            }
        }
        match self.phase {
            Phase::Initial => {
                if ready {
                    self.capture(
                        ctx,
                        physical_size,
                        expected_marker,
                        current_chapter,
                        current_page,
                    );
                }
            }
            Phase::WaitCount if self.total_ready_ms.is_some() && ready => {
                if self.count_only {
                    self.finish(None);
                    return Some(PerfAction::Close);
                }
                if current_page != 0 {
                    self.finish(Some("Turn fixture must start at chapter page zero"));
                    return Some(PerfAction::Close);
                }
                self.phase = Phase::Baseline;
                self.after_frame = ctx.cumulative_frame_nr() + 1;
            }
            Phase::Baseline | Phase::OverlayOn | Phase::OverlayOff => {
                if ready && ctx.cumulative_frame_nr() > self.after_frame {
                    self.capture(
                        ctx,
                        physical_size,
                        expected_marker,
                        current_chapter,
                        current_page,
                    );
                }
            }
            Phase::Turns => {
                if let Some(pending) = &self.pending_turn {
                    if current_chapter == pending.chapter
                        && current_page == pending.target
                        && ctx.cumulative_frame_nr() > pending.dispatched_frame
                    {
                        // Observe the first target pixels even while the host
                        // is awaiting its extra paint barrier. A pane's marker
                        // may still describe the old page, so only reuse its
                        // revision channel, never its page channels.
                        let marker =
                            expected_marker.map(|marker| page_marker(pending.target, marker[2]));
                        self.capture(ctx, physical_size, marker, current_chapter, current_page);
                    }
                } else if ready {
                    if let Some(target) = self.sequence.pop_front() {
                        self.pending_turn = Some(PendingTurn {
                            chapter: current_chapter,
                            target,
                            dispatched_ms: self.clock.ms(),
                            dispatched_frame: ctx.cumulative_frame_nr(),
                        });
                        return Some(PerfAction::Turn(target > current_page));
                    }
                    if let Some(marker) = expected_marker {
                        if current_page != 0 {
                            self.finish(Some(
                                "Rapid-wheel fixture must begin at chapter page zero",
                            ));
                            return Some(PerfAction::Close);
                        }
                        self.phase = Phase::Burst;
                        self.burst_started_ms = Some(self.clock.ms());
                        self.burst_chapter = current_chapter;
                        self.burst_blue = marker[2];
                        return Some(PerfAction::WheelBurst(5));
                    }
                }
            }
            Phase::Burst => {
                if let Err(error) = self.collect_burst(physical_size) {
                    self.finish(Some(error));
                    return Some(PerfAction::Close);
                }
                if self.burst_observed == [1, 2, 3, 4, 5]
                    && ready
                    && current_chapter == self.burst_chapter
                    && current_page == 5
                {
                    self.finish(None);
                    return Some(PerfAction::Close);
                }
                self.capture_burst(ctx, current_chapter);
            }
            _ => {}
        }
        ctx.request_repaint_after(Duration::from_millis(4));
        None
    }

    fn capture(
        &mut self,
        ctx: &egui::Context,
        size: [usize; 2],
        marker: Option<[u8; 3]>,
        chapter: usize,
        page: usize,
    ) {
        if self.capture_pending {
            return;
        }
        let Some(marker) = marker else {
            return;
        };
        if size[0] == 0 || size[1] == 0 {
            return;
        }
        self.capture_pending = true;
        let result = Arc::clone(&self.captured);
        let scale = ctx.pixels_per_point();
        let clock = self.clock;
        ctx.request_screenshot(move |image| {
            let (matches, sampled_rgb, matching_pixels) = sample_marker(&image, marker, scale);
            let capture = Capture {
                chapter,
                page,
                callback_ms: clock.ms(),
                marker_matches: matches,
                expected_rgb: marker,
                sampled_rgb,
                matching_pixels,
                image_size: image.size,
                right_book_hash: book_region_hash(&image),
                toolbar_hash: toolbar_region_hash(&image, scale),
            };
            *result.lock().unwrap_or_else(|error| error.into_inner()) = Some(capture);
        });
        ctx.request_repaint();
    }

    /// Capture every rendered UI frame without waiting for prior callbacks.
    /// This never delays or drains the host's own wheel input queue.
    fn capture_burst(&mut self, ctx: &egui::Context, chapter: usize) {
        let frame = ctx.cumulative_frame_nr();
        if self.burst_last_frame == Some(frame) {
            return;
        }
        self.burst_last_frame = Some(frame);
        let ordinal = self.burst_next_request;
        self.burst_next_request += 1;
        let captures = Arc::clone(&self.burst_captures);
        let clock = self.clock;
        let requested_ms = clock.ms();
        let scale = ctx.pixels_per_point();
        let blue = self.burst_blue;
        ctx.request_screenshot(move |image| {
            let (_, sampled_rgb, _) = sample_marker(&image, [0; 3], scale);
            // Decode actual GPU pixels independently of the host page number.
            let observed_page = (0..=5).find(|page| {
                let expected = [16 + ((*page & 7) * 32) as u8, 16, blue];
                sample_marker(&image, expected, scale).0
            });
            let capture = BurstCapture {
                ordinal,
                frame,
                chapter,
                requested_ms,
                callback_ms: clock.ms(),
                observed_page,
                sampled_rgb,
                size: image.size,
            };
            captures
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(capture);
        });
        ctx.request_repaint();
    }

    fn collect_burst(&mut self, size: [usize; 2]) -> Result<(), &'static str> {
        let captures = std::mem::take(
            &mut *self
                .burst_captures
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        for capture in captures {
            self.burst_buffer.insert(capture.ordinal, capture);
        }
        // Callback completion order is not presentation order. Validate the
        // requests in their native rendered-frame order instead.
        while let Some(capture) = self.burst_buffer.remove(&self.burst_next_result) {
            self.burst_next_result += 1;
            if capture.chapter != self.burst_chapter || capture.size != size {
                return Err("Rapid-wheel fixture changed chapter or viewport");
            }
            self.burst_frames.push(json!({
                "capture":capture.ordinal, "frame":capture.frame,
                "requested_ms":capture.requested_ms, "callback_ms":capture.callback_ms,
                "observed_page":capture.observed_page, "sampled_rgb":capture.sampled_rgb,
            }));
            if let Some(page) = capture.observed_page {
                let before = self.burst_observed.len();
                let completed = observe_burst(&mut self.burst_observed, page)?;
                if self.burst_observed.len() != before {
                    self.burst_transitions.push(json!({
                        "page":page, "capture":capture.ordinal, "frame":capture.frame,
                        "screenshot_callback_ms":capture.callback_ms,
                        "dispatch_to_readback_ms":capture.callback_ms-self.burst_started_ms.unwrap(),
                    }));
                }
                if completed {
                    self.burst_completed_ms.get_or_insert(capture.callback_ms);
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self, error: Option<&str>) {
        self.phase = Phase::Finished;
        let values: Vec<_> = self
            .turns
            .iter()
            .filter_map(|turn| turn["dispatch_to_readback_ms"].as_f64())
            .collect();
        let first_five: Vec<_> = self
            .turns
            .iter()
            .take(5)
            .filter_map(|turn| turn["page"].as_u64())
            .collect();
        let expected_order =
            self.count_only || (self.turns.len() == 30 && first_five == [1, 2, 3, 4, 5]);
        let overlay_stable = self.count_only
            || (self.overlay_checks.len() == 2
                && self.overlay_checks.iter().all(|check| {
                    check["right_book_pixels_unchanged"] == true
                        && check["toolbar_state_verified"] == true
                }));
        let burst_valid = self.count_only || self.burst_observed == [1, 2, 3, 4, 5];
        let report = json!({
            "measurement":"Native app GPU screenshot callback; includes GPU readback and callback delivery, excludes display-photon latency",
            "marker_validation":"3x3 marker-center pixels, at least five matches, maximum channel error 8; ordered page fixture uses pages 0 through 5",
            "turn_input":"30 sequential native actions followed by a separate five-notch wheel burst enqueued once; screenshot callbacks do not gate burst input dispatch",
            "turn_endpoint":"First confirmed target-page GPU screenshot readback, sampled before host readiness barriers; subsequent turn dispatch still awaits host readiness",
            "overlay_validation":"Hidden baseline is refreshed after pagination completes; uncovered right-book pixels stay identical while toolbar pixels change and return; the toolbar may occlude the marker while visible",
            "clock":self.clock.description(),
            "requested_backend":self.requested_backend,
            "count_only":self.count_only,
            "success":error.is_none() && expected_order && overlay_stable && burst_valid,
            "error":error,
            "first_book_readback_ms":self.first_frame_ms,
            "total_pages_ready_ms":self.total_ready_ms,
            "total_pages":self.total_pages,
            "initial_chapter":self.initial_chapter,
            "initial_page":self.initial_page,
            "turn_count":self.turns.len(),
            "turn_median_ms":percentile(&values,0.5),
            "turn_p95_ms":percentile(&values,0.95),
            "first_five_pages":first_five,
            "all_requested_pages_read_back":expected_order,
            "overlay_book_pixels_stable":overlay_stable,
            "overlay_checks":self.overlay_checks,
            "turns":self.turns,
            "marker_samples":self.marker_samples,
            "rejected_captures":self.rejected_captures,
            "rapid_wheel_burst":{
                "input_notches":if self.count_only {0} else {5},
                "enqueued_in_one_input_frame":self.burst_started_ms.is_some(),
                "dispatch_ms":self.burst_started_ms,
                "confirmed_page_sequence":self.burst_observed,
                "all_five_pages_read_back_in_order":burst_valid && !self.count_only,
                "dispatch_to_fifth_readback_ms":self.burst_started_ms.zip(self.burst_completed_ms).map(|(start,end)|end-start),
                "transitions":self.burst_transitions,
                "frame_readbacks":self.burst_frames,
                "ordering":"Screenshot requests follow rendered-frame order; asynchronous callbacks are reordered by request ordinal before page validation",
            },
        });
        let result = (|| -> std::io::Result<()> {
            if let Some(parent) = self.output.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            let temporary = self
                .output
                .with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&temporary, serde_json::to_vec_pretty(&report)?)?;
            std::fs::rename(temporary, &self.output)
        })();
        if let Err(error) = result {
            eprintln!("Performance report could not be written: {error}");
        }
    }
}

fn observe_burst(observed: &mut Vec<usize>, page: usize) -> Result<bool, &'static str> {
    // Repeated reads of the same displayed page are expected. Initial page zero
    // is also expected before Chromium consumes the first queued wheel notch.
    if observed.last().copied().unwrap_or(0) == page {
        return Ok(observed.len() == 5);
    }
    if page != observed.len() + 1 || observed.len() == 5 {
        return Err("Rapid-wheel GPU pixels skipped or reversed a requested page");
    }
    observed.push(page);
    Ok(observed.len() == 5)
}

fn page_marker(page: usize, revision_blue: u8) -> [u8; 3] {
    [
        16 + ((page & 7) * 32) as u8,
        16 + (((page >> 3) & 7) * 32) as u8,
        revision_blue,
    ]
}

fn sample_marker(
    image: &egui::ColorImage,
    expected: [u8; 3],
    scale: f32,
) -> (bool, [u8; 3], usize) {
    let x = (image.width() as f32 - 4. * scale).round() as isize;
    let y = (image.height() as f32 - 36. * scale).round() as isize;
    let mut pixels = Vec::with_capacity(9);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (x, y) = (x + dx, y + dy);
            if x >= 0 && y >= 0 && (x as usize) < image.width() && (y as usize) < image.height() {
                let color = image[(x as usize, y as usize)];
                pixels.push([color.r(), color.g(), color.b()]);
            }
        }
    }
    let matches = pixels
        .iter()
        .filter(|pixel| {
            pixel
                .iter()
                .zip(expected)
                .all(|(value, want)| value.abs_diff(want) <= 8)
        })
        .count();
    let sampled = if pixels.is_empty() {
        [0; 3]
    } else {
        std::array::from_fn(|channel| {
            let mut values: Vec<_> = pixels.iter().map(|pixel| pixel[channel]).collect();
            values.sort_unstable();
            values[values.len() / 2]
        })
    };
    (pixels.len() == 9 && matches >= 5, sampled, matches)
}

fn book_region_hash(image: &egui::ColorImage) -> String {
    region_hash(
        image,
        image.width() * 65 / 100..image.width() * 9 / 10,
        image.height() / 4..image.height() * 7 / 10,
    )
}

fn toolbar_region_hash(image: &egui::ColorImage, scale: f32) -> String {
    let top = (image.height() as f32 - 100. * scale).max(0.) as usize;
    let bottom = (image.height() as f32 - 28. * scale).max(0.) as usize;
    region_hash(
        image,
        image.width() * 4 / 10..image.width() * 9 / 10,
        top..bottom,
    )
}

fn region_hash(
    image: &egui::ColorImage,
    xs: std::ops::Range<usize>,
    ys: std::ops::Range<usize>,
) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for y in ys {
        for x in xs.clone() {
            let pixel = image[(x, y)];
            for value in [pixel.r(), pixel.g(), pixel.b(), pixel.a()] {
                hash = (hash ^ u64::from(value)).wrapping_mul(0x100000001b3);
            }
        }
    }
    format!("{hash:016x}")
}

fn percentile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    if fraction == 0.5 && values.len().is_multiple_of(2) {
        let mid = values.len() / 2;
        Some((values[mid - 1] + values[mid]) / 2.)
    } else {
        Some(
            values[((values.len() as f64 * fraction).ceil() as usize)
                .saturating_sub(1)
                .min(values.len() - 1)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_turn_marker_cannot_accept_previous_page_pixels() {
        let old_marker = page_marker(0, 160);
        let expected = page_marker(9, old_marker[2]);
        assert_eq!(expected, [48, 48, 160]);
        let mut image = egui::ColorImage::new(
            [80, 80],
            vec![egui::Color32::from_rgb(old_marker[0], old_marker[1], old_marker[2]); 6400],
        );
        assert!(!sample_marker(&image, expected, 1.).0);
        for y in 43..46 {
            for x in 75..78 {
                image[(x, y)] = egui::Color32::from_rgb(expected[0], expected[1], expected[2]);
            }
        }
        assert!(sample_marker(&image, expected, 1.).0);
    }

    #[test]
    fn rapid_wheel_proof_ignores_duplicate_frames_but_rejects_skips_and_reversal() {
        let mut observed = Vec::new();
        for page in [0, 0, 1, 1, 2, 3, 3, 4, 5, 5] {
            assert_eq!(observe_burst(&mut observed, page), Ok(page == 5));
        }
        assert_eq!(observed, [1, 2, 3, 4, 5]);
        assert!(observe_burst(&mut vec![1], 3).is_err());
        assert!(observe_burst(&mut vec![1, 2], 1).is_err());
        assert!(observe_burst(&mut observed, 0).is_err());
    }
    #[test]
    fn gpu_marker_validation_rejects_previous_page_and_accepts_small_rounding_noise() {
        let mut image = egui::ColorImage::new([80, 80], vec![egui::Color32::BLACK; 6400]);
        for y in 43..46 {
            for x in 75..78 {
                image[(x, y)] = egui::Color32::from_rgb(31, 66, 127);
            }
        }
        assert_eq!(
            sample_marker(&image, [32, 64, 128], 1.),
            (true, [31, 66, 127], 9)
        );
        assert!(!sample_marker(&image, [52, 64, 128], 1.).0);
        assert!(!sample_marker(&image, [32, 64, 128], 2.).0);
    }
    #[test]
    fn percentile_uses_nearest_rank_and_even_median() {
        assert_eq!(percentile(&[], 0.95), None);
        assert_eq!(percentile(&[8., 2., 4., 6.], 0.5), Some(5.));
        assert_eq!(
            percentile(&(1..=30).rev().map(f64::from).collect::<Vec<_>>(), 0.95),
            Some(29.)
        );
    }

    #[test]
    fn overlay_validation_observes_toolbar_change_without_book_reflow() {
        let original = egui::ColorImage::new([160, 400], vec![egui::Color32::WHITE; 64000]);
        let mut overlay = original.clone();
        for y in 300..372 {
            for x in 0..160 {
                overlay[(x, y)] = egui::Color32::BLACK;
            }
        }
        assert_eq!(book_region_hash(&original), book_region_hash(&overlay));
        assert_ne!(
            toolbar_region_hash(&original, 1.),
            toolbar_region_hash(&overlay, 1.)
        );
        overlay[(110, 130)] = egui::Color32::BLACK;
        assert_ne!(book_region_hash(&original), book_region_hash(&overlay));
    }
}
