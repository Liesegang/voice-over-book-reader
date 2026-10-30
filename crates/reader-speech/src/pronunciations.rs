//! Literal pronunciation overrides with provider-clock timings mapped to source UTF-8.
use crate::{
    BoundaryPrecision, Cancellation, SpeechUnit, SynthesizedAudio, TtsProvider, UnitRange,
};
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, ops::Range};

pub fn validate_pronunciations(rules: &BTreeMap<String, String>) -> Result<()> {
    for (word, reading) in rules {
        ensure!(!word.trim().is_empty(), "登録する表記を入力してください");
        ensure!(
            !reading.trim().is_empty(),
            "「{word}」の読みを入力してください"
        );
    }
    Ok(())
}

fn matches<'a>(text: &str, rules: &'a BTreeMap<String, String>) -> Vec<(Range<usize>, &'a str)> {
    let mut ordered: Vec<_> = rules.iter().filter(|(word, _)| !word.is_empty()).collect();
    ordered.sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    let mut indexed: BTreeMap<char, Vec<_>> = BTreeMap::new();
    for (word, reading) in ordered {
        indexed
            .entry(word.chars().next().unwrap())
            .or_default()
            .push((word, reading));
    }
    let mut found = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let character = text[offset..].chars().next().unwrap();
        if let Some((word, reading)) = indexed.get(&character).and_then(|candidates| {
            candidates
                .iter()
                .find(|(word, _)| text[offset..].starts_with(word.as_str()))
        }) {
            let end = offset + word.len();
            found.push((offset..end, reading.as_str()));
            offset = end;
        } else {
            offset += character.len_utf8();
        }
    }
    found
}

/// Preserve original spellings of ruby atoms touched by a registered phrase.
/// Matching can cross atom boundaries and uses the same longest-match order as synthesis.
pub fn pronunciation_source_mask<'a>(
    texts: impl IntoIterator<Item = &'a str>,
    rules: &BTreeMap<String, String>,
) -> Vec<bool> {
    let texts: Vec<_> = texts.into_iter().collect();
    if rules.is_empty() {
        return vec![false; texts.len()];
    }
    let full = texts.concat();
    let matches = matches(&full, rules);
    let mut offset = 0;
    texts
        .iter()
        .map(|text| {
            let end = offset + text.len();
            let touched = end > offset
                && matches
                    .iter()
                    .any(|(range, _)| range.start < end && range.end > offset);
            offset = end;
            touched
        })
        .collect()
}

pub(crate) fn pronunciation_target(
    units: &[SpeechUnit],
    rules: &BTreeMap<String, String>,
    index: usize,
    byte: usize,
) -> (usize, usize) {
    if rules.is_empty() {
        return (index, byte);
    }
    let text: String = units.iter().map(|unit| unit.text.as_str()).collect();
    let source = units[..index]
        .iter()
        .map(|unit| unit.text.len())
        .sum::<usize>()
        + byte;
    let Some((word, _)) = matches(&text, rules)
        .into_iter()
        .find(|(word, _)| word.contains(&source))
    else {
        return (index, byte);
    };
    let mut offset = 0;
    for (unit_index, unit) in units.iter().enumerate() {
        if word.start < offset + unit.text.len() {
            return (unit_index, word.start - offset);
        }
        offset += unit.text.len();
    }
    (index, byte)
}

struct SourcePart {
    spoken: Range<usize>,
    source: Vec<UnitRange>,
    replaced: bool,
}

struct PronouncedUnits {
    units: Vec<SpeechUnit>,
    parts: Vec<SourcePart>,
}

impl PronouncedUnits {
    fn new(units: &[SpeechUnit], rules: &BTreeMap<String, String>) -> Self {
        if rules.is_empty() {
            return Self {
                units: units.to_vec(),
                parts: Vec::new(),
            };
        }
        let text: String = units.iter().map(|unit| unit.text.as_str()).collect();
        let matches = matches(&text, rules);
        if matches.is_empty() {
            return Self {
                units: units.to_vec(),
                parts: Vec::new(),
            };
        }
        let mut spoken = String::new();
        let mut parts = Vec::new();
        let mut source_offset = 0;
        for (range, reading) in matches {
            Self::copy_literal(
                &text,
                source_offset..range.start,
                units,
                &mut spoken,
                &mut parts,
            );
            let from = spoken.len();
            spoken.push_str(reading);
            parts.push(SourcePart {
                spoken: from..spoken.len(),
                source: crate::types::ranges_for_text(units, range.start, range.end),
                replaced: true,
            });
            source_offset = range.end;
        }
        Self::copy_literal(
            &text,
            source_offset..text.len(),
            units,
            &mut spoken,
            &mut parts,
        );
        Self {
            units: vec![SpeechUnit {
                id: 0,
                text: spoken,
            }],
            parts,
        }
    }

    fn copy_literal(
        text: &str,
        range: Range<usize>,
        units: &[SpeechUnit],
        spoken: &mut String,
        parts: &mut Vec<SourcePart>,
    ) {
        for (relative, character) in text[range.clone()].char_indices() {
            let source = range.start + relative;
            let from = spoken.len();
            spoken.push(character);
            parts.push(SourcePart {
                spoken: from..spoken.len(),
                source: crate::types::ranges_for_text(units, source, source + character.len_utf8()),
                replaced: false,
            });
        }
    }

    fn remap(&self, audio: &mut SynthesizedAudio) -> Result<()> {
        if self.parts.is_empty() {
            return Ok(());
        }
        let spoken = &self.units[0].text;
        for boundary in &mut audio.boundaries {
            let mut source: Vec<UnitRange> = Vec::new();
            let mut replaced = false;
            for range in &boundary.ranges {
                ensure!(
                    range.id == 0
                        && range.start <= range.end
                        && range.end <= spoken.len()
                        && spoken.is_char_boundary(range.start)
                        && spoken.is_char_boundary(range.end),
                    "Provider returned an invalid pronunciation range"
                );
                let from = self
                    .parts
                    .partition_point(|part| part.spoken.end <= range.start);
                for part in self.parts[from..]
                    .iter()
                    .take_while(|part| range.end > part.spoken.start)
                {
                    replaced |= part.replaced;
                    for mapped in &part.source {
                        if let Some(last) = source.last_mut()
                            && last.id == mapped.id
                            && mapped.start <= last.end
                            && mapped.end >= last.start
                        {
                            last.start = last.start.min(mapped.start);
                            last.end = last.end.max(mapped.end);
                        } else {
                            source.push(mapped.clone());
                        }
                    }
                }
            }
            boundary.ranges = source;
            if replaced && boundary.precision != BoundaryPrecision::Chunk {
                boundary.precision = BoundaryPrecision::AlignedGroup;
            }
        }
        Ok(())
    }
}

pub(crate) struct PronunciationProvider<'a> {
    pub provider: &'a mut dyn TtsProvider,
    pub rules: &'a BTreeMap<String, String>,
}
impl TtsProvider for PronunciationProvider<'_> {
    fn name(&self) -> &str {
        self.provider.name()
    }
    fn synthesize(
        &mut self,
        units: &[SpeechUnit],
        cancel: &Cancellation,
        progress: &dyn Fn(String),
    ) -> Result<SynthesizedAudio> {
        validate_pronunciations(self.rules)?;
        let pronounced = PronouncedUnits::new(units, self.rules);
        let mut audio = self
            .provider
            .synthesize(&pronounced.units, cancel, progress)?;
        audio.validate()?;
        pronounced.remap(&mut audio)?;
        Ok(audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TimedBoundary;
    fn rules() -> BTreeMap<String, String> {
        [
            ("日本".into(), "にほん".into()),
            ("日本橋".into(), "にっぽんばし".into()),
        ]
        .into()
    }
    #[test]
    fn longest_literal_match_crosses_atoms_and_keeps_unregistered_ruby() {
        assert_eq!(
            pronunciation_source_mask(["a日本", "橋へ", "無関係"], &rules()),
            vec![true, true, false]
        );
        let units = vec![
            SpeechUnit {
                id: 4,
                text: "a日本".into(),
            },
            SpeechUnit {
                id: 8,
                text: "橋へ".into(),
            },
        ];
        let pronounced = PronouncedUnits::new(&units, &rules());
        assert_eq!(pronounced.units[0].text, "aにっぽんばしへ");
        assert_eq!(pronunciation_target(&units, &rules(), 1, 0), (0, 1));
        assert_eq!(pronunciation_target(&units, &rules(), 1, 3), (1, 3));
    }
    #[test]
    fn replacement_events_use_whole_original_phrase_without_changing_sample_clock() {
        let units = vec![
            SpeechUnit {
                id: 4,
                text: "a日本".into(),
            },
            SpeechUnit {
                id: 8,
                text: "橋へ😀".into(),
            },
        ];
        let pronounced = PronouncedUnits::new(&units, &rules());
        let mut audio = SynthesizedAudio {
            samples: vec![0.; 100],
            sample_rate: 24000,
            boundaries: vec![
                TimedBoundary {
                    start_sample: 7,
                    end_sample: 29,
                    ranges: vec![UnitRange {
                        id: 0,
                        start: 4,
                        end: 10,
                    }],
                    precision: BoundaryPrecision::Word,
                },
                TimedBoundary {
                    start_sample: 30,
                    end_sample: 90,
                    ranges: vec![UnitRange {
                        id: 0,
                        start: 19,
                        end: 26,
                    }],
                    precision: BoundaryPrecision::Word,
                },
            ],
        };
        pronounced.remap(&mut audio).unwrap();
        assert_eq!(
            audio.boundaries[0].ranges,
            vec![
                UnitRange {
                    id: 4,
                    start: 1,
                    end: 7
                },
                UnitRange {
                    id: 8,
                    start: 0,
                    end: 3
                }
            ]
        );
        assert_eq!(
            audio.boundaries[0].precision,
            BoundaryPrecision::AlignedGroup
        );
        assert_eq!(
            (
                audio.boundaries[0].start_sample,
                audio.boundaries[0].end_sample
            ),
            (7, 29)
        );
        assert_eq!(
            audio.boundaries[1].ranges,
            vec![UnitRange {
                id: 8,
                start: 3,
                end: 10
            }]
        );
    }
    #[test]
    fn unchanged_units_and_unknown_provider_ranges_are_handled_honestly() {
        let original = vec![SpeechUnit {
            id: 42,
            text: "東京".into(),
        }];
        assert_eq!(PronouncedUnits::new(&original, &rules()).units, original);
        let pronounced = PronouncedUnits::new(
            &[SpeechUnit {
                id: 0,
                text: "日本橋".into(),
            }],
            &rules(),
        );
        let mut audio = SynthesizedAudio {
            samples: vec![0.; 100],
            sample_rate: 24000,
            boundaries: vec![TimedBoundary {
                start_sample: 0,
                end_sample: 100,
                ranges: vec![UnitRange {
                    id: 99,
                    start: 0,
                    end: 3,
                }],
                precision: BoundaryPrecision::Word,
            }],
        };
        assert!(pronounced.remap(&mut audio).is_err());
        let invalid = [("".into(), "あ".into())].into();
        assert!(validate_pronunciations(&invalid).is_err());
        assert!(validate_pronunciations(&[("語".into(), " ".into())].into()).is_err());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Requires an installed Japanese Windows voice; actually synthesizes PCM"]
    fn system_voice_pronunciation_keeps_original_word_ranges() {
        let mut provider =
            crate::create_provider(&crate::ProviderConfig::System { voice: None }).unwrap();
        let rules = [("日本橋".into(), "にっぽんばし".into())].into();
        let original = SpeechUnit {
            id: 51,
            text: "日本橋で本を読みます。".into(),
        };
        let audio = PronunciationProvider {
            provider: provider.as_mut(),
            rules: &rules,
        }
        .synthesize(
            std::slice::from_ref(&original),
            &Cancellation::uncancelled(),
            &|_| {},
        )
        .unwrap();
        audio.validate().unwrap();
        assert!(audio.boundaries.iter().any(|boundary| {
            boundary.precision == BoundaryPrecision::AlignedGroup
                && boundary
                    .ranges
                    .iter()
                    .any(|range| range.id == 51 && range.start == 0 && range.end == 9)
        }));
        assert!(
            audio
                .boundaries
                .iter()
                .flat_map(|boundary| &boundary.ranges)
                .all(|range| range.id == 51
                    && range.end <= original.text.len()
                    && original.text.is_char_boundary(range.start)
                    && original.text.is_char_boundary(range.end))
        );
        eprintln!(
            "Actual SAPI override: rate={} samples={} marks={} first={:?}",
            audio.sample_rate,
            audio.samples.len(),
            audio.boundaries.len(),
            audio.boundaries.first()
        );
        if let Some(path) = std::env::var_os("VOBR_PRONUNCIATION_QA_WAV") {
            assert!(
                !std::path::Path::new(&path).exists(),
                "QA WAV must not overwrite an existing file"
            );
            let mut wav = hound::WavWriter::create(
                path,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: audio.sample_rate,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            for sample in audio.samples {
                wav.write_sample((sample.clamp(-1., 1.) * 32767.).round() as i16)
                    .unwrap();
            }
            wav.finalize().unwrap();
        }
    }
}
