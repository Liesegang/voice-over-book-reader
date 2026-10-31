use super::*;
use std::sync::Mutex;

#[test]
fn cancelling_prefetch_keeps_active_playback_and_its_commands_unchanged() {
    let (commands, received) = mpsc::channel();
    let (_, events) = mpsc::channel();
    let epoch = Arc::new(AtomicU64::new(7));
    let prefetch_epoch = Arc::new(AtomicU64::new(3));
    let prefetch = Cancellation::new(epoch.clone(), 7).with_supersession(prefetch_epoch.clone(), 3);
    let playback = Cancellation::new(epoch.clone(), 7);
    let controller = SpeechController {
        commands,
        events,
        epoch,
        prefetch_epoch,
    };
    controller.cancel_prefetch();
    assert!(prefetch.is_cancelled());
    assert!(!playback.is_cancelled());
    assert_eq!(controller.generation(), 7);
    assert!(
        received.try_recv().is_err(),
        "prefetch cancellation must not enqueue Stop"
    );
}

#[test]
#[ignore = "requires an explicitly provided private Japanese text corpus"]
fn japanese_frontend_private_corpus() {
    let path = std::env::var_os("VOBR_JAPANESE_CORPUS").expect("VOBR_JAPANESE_CORPUS");
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let units = rows
        .iter()
        .enumerate()
        .map(|(index, row)| SpeechUnit {
            id: index as u64,
            text: row["text"].as_str().unwrap().to_owned(),
        })
        .collect();
    let frontend = crate::aivis_japanese::JapaneseFrontend::new().unwrap();
    let mut failures = Vec::new();
    let mut count = 0;
    for (index, chunk) in chunks(units).into_iter().enumerate() {
        let text = chunk
            .units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect::<String>();
        count += 1;
        if let Err(error) = frontend.prepare(&text) {
            failures.push(serde_json::json!({"chunk":index,"characters":text.chars().count(),"error":format!("{error:#}")}));
        }
    }
    if let Some(path) = std::env::var_os("VOBR_JAPANESE_CORPUS_REPORT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({"chunks":count,"failures":failures}))
                .unwrap(),
        )
        .unwrap();
    }
    assert!(
        failures.is_empty(),
        "private corpus frontend failures: {failures:?}"
    );
}

struct FailureProvider {
    calls: Arc<Mutex<Vec<String>>>,
}
impl crate::TtsProvider for FailureProvider {
    fn name(&self) -> &str {
        "chapter failure fixture"
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        _: &Cancellation,
        _: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        let text = units
            .iter()
            .map(|unit| unit.text.as_str())
            .collect::<String>();
        self.calls.lock().unwrap().push(text.clone());
        ensure!(!text.contains("ERROR"), "Long vowel has no preceding vowel");
        Ok(SynthesizedAudio {
            samples: vec![0.1; 240],
            sample_rate: 24_000,
            boundaries: vec![TimedBoundary {
                start_sample: 0,
                end_sample: 240,
                ranges: crate::types::unit_ranges(units),
                precision: BoundaryPrecision::Word,
            }],
        })
    }
}

fn job(directory: &std::path::Path, units: Vec<SpeechUnit>, playback: bool) -> Job {
    Job {
        queued_at: Instant::now(),
        generation: 1,
        target: (!playback).then(|| (units[0].id, 0)),
        units,
        playback,
        prefetch_ticket: (!playback).then_some(1),
        config: SpeechConfig {
            cache_dir: directory.into(),
            ..SpeechConfig::default()
        },
    }
}

#[test]
fn failure_after_first_paragraph_does_not_skip_into_next_chapter() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (jobs, input) = mpsc::channel();
    let (output, results) = mpsc::channel();
    jobs.send(job(
        directory.path(),
        vec![
            SpeechUnit {
                id: 8 << 32,
                text: format!("{}。", "あ".repeat(99)),
            },
            SpeechUnit {
                id: (8 << 32) + 1,
                text: "ERROR。".into(),
            },
            SpeechUnit {
                id: (8 << 32) + 2,
                text: "まだ第八章は続きます。".into(),
            },
        ],
        true,
    ))
    .unwrap();
    jobs.send(job(
        directory.path(),
        vec![SpeechUnit {
            id: 9 << 32,
            text: "第九章へ勝手に飛んではいけません。".into(),
        }],
        true,
    ))
    .unwrap();
    drop(jobs);
    let provider_calls = calls.clone();
    synthesize_jobs_with_factory(
        input,
        output,
        Arc::new(AtomicU64::new(1)),
        Arc::new(AtomicU64::new(1)),
        move |_| {
            Ok(Box::new(FailureProvider {
                calls: provider_calls.clone(),
            }))
        },
    );
    let messages = results.try_iter().collect::<Vec<_>>();
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Produced::Error(1, _)))
            .count(),
        1
    );
    let ready = messages
        .into_iter()
        .filter_map(|message| {
            if let Produced::Ready(1, audio) = message {
                Some(audio.load().unwrap())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ready.len(),
        1,
        "only the completed first paragraph may have audio"
    );
    assert!(
        ready[0]
            .boundaries
            .iter()
            .flat_map(|boundary| &boundary.ranges)
            .all(|range| range.id >> 32 == 8)
    );
    assert_eq!(
        calls.lock().unwrap().len(),
        2,
        "the next chapter must not even be synthesized after this failure"
    );
}

#[test]
fn failed_prefetch_does_not_report_a_playback_error_or_revoke_later_playback() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (jobs, input) = mpsc::channel();
    let (output, results) = mpsc::channel();
    jobs.send(job(
        directory.path(),
        vec![SpeechUnit {
            id: 8,
            text: "ERROR。".into(),
        }],
        false,
    ))
    .unwrap();
    jobs.send(job(
        directory.path(),
        vec![SpeechUnit {
            id: 9,
            text: "選び直した位置から読みます。".into(),
        }],
        true,
    ))
    .unwrap();
    drop(jobs);
    let provider_calls = calls.clone();
    synthesize_jobs_with_factory(
        input,
        output,
        Arc::new(AtomicU64::new(1)),
        Arc::new(AtomicU64::new(1)),
        move |_| {
            Ok(Box::new(FailureProvider {
                calls: provider_calls.clone(),
            }))
        },
    );
    let messages = results.try_iter().collect::<Vec<_>>();
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Produced::Error(..)))
    );
    assert!(!messages.iter().any(|message| matches!(message, Produced::Progress(_, text) if text == "音声の先読みが完了しました")));
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Produced::Ready(1, _)))
            .count(),
        1
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[test]
fn a_new_playback_generation_recovers_after_a_failed_chapter() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let epoch = Arc::new(AtomicU64::new(1));
    let worker_epoch = epoch.clone();
    let (jobs, input) = mpsc::channel();
    let (output, results) = mpsc::channel();
    let provider_calls = calls.clone();
    let worker = thread::spawn(move || {
        synthesize_jobs_with_factory(
            input,
            output,
            worker_epoch,
            Arc::new(AtomicU64::new(1)),
            move |_| {
                Ok(Box::new(FailureProvider {
                    calls: provider_calls.clone(),
                }))
            },
        )
    });
    jobs.send(job(
        directory.path(),
        vec![SpeechUnit {
            id: 8,
            text: "ERROR。".into(),
        }],
        true,
    ))
    .unwrap();
    loop {
        if matches!(
            results.recv_timeout(Duration::from_secs(5)).unwrap(),
            Produced::Error(1, _)
        ) {
            break;
        }
    }
    epoch.store(2, Ordering::Release);
    let mut retry = job(
        directory.path(),
        vec![SpeechUnit {
            id: 9,
            text: "同じ章から再試行できます。".into(),
        }],
        true,
    );
    retry.generation = 2;
    jobs.send(retry).unwrap();
    drop(jobs);
    worker.join().unwrap();
    assert!(
        results
            .try_iter()
            .any(|message| matches!(message, Produced::Ready(2, _)))
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[test]
fn a_superseded_in_flight_failure_cannot_abort_the_new_playback_generation() {
    struct GatedFailureProvider {
        inner: FailureProvider,
        entered: Sender<()>,
        release: Receiver<()>,
    }
    impl crate::TtsProvider for GatedFailureProvider {
        fn name(&self) -> &str {
            "superseded in-flight failure fixture"
        }

        fn synthesize(
            &mut self,
            units: &[SpeechUnit],
            cancel: &Cancellation,
            progress: &dyn Fn(String),
        ) -> Result<SynthesizedAudio> {
            if units.iter().any(|unit| unit.text.contains("ERROR")) {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            self.inner.synthesize(units, cancel, progress)
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let epoch = Arc::new(AtomicU64::new(1));
    let worker_epoch = epoch.clone();
    let (jobs, input) = mpsc::channel();
    let (output, results) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let provider_calls = calls.clone();
    let worker = thread::spawn(move || {
        let mut release = Some(release_rx);
        synthesize_jobs_with_factory(
            input,
            output,
            worker_epoch,
            Arc::new(AtomicU64::new(1)),
            move |_| {
                Ok(Box::new(GatedFailureProvider {
                    inner: FailureProvider {
                        calls: provider_calls.clone(),
                    },
                    entered: entered_tx.clone(),
                    release: release.take().unwrap(),
                }))
            },
        );
    });
    jobs.send(job(
        directory.path(),
        vec![SpeechUnit {
            id: 8,
            text: "OLD ERROR。".into(),
        }],
        true,
    ))
    .unwrap();

    // Advance the epoch only after the old provider is inside synthesize, then
    // release its failure after the replacement job has already been queued.
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    epoch.store(2, Ordering::Release);
    let mut retry = job(
        directory.path(),
        vec![SpeechUnit {
            id: 9,
            text: "選び直した新しい世代の本文を読みます。".into(),
        }],
        true,
    );
    retry.generation = 2;
    jobs.send(retry).unwrap();
    release_tx.send(()).unwrap();
    drop(jobs);
    worker.join().unwrap();

    let messages = results.try_iter().collect::<Vec<_>>();
    assert!(!messages.iter().any(|message| matches!(
        message,
        Produced::Ready(1, _) | Produced::Error(1, _) | Produced::Done(1)
    )));
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Produced::Ready(2, _)))
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Produced::Done(2)))
            .count(),
        1
    );
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Produced::Error(..)))
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}
