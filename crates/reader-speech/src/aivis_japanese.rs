//! Native Japanese frontend for the JP-Extra model interface.
//! Uses BSD-licensed JPreprocess/Open JTalk labels; no SBV2 Python code is used.
use anyhow::{Context, Result, bail, ensure};
use jpreprocess::{
    DefaultTokenizer, JPreprocess, SystemDictionaryConfig, kind::JPreprocessDictionaryKind,
    normalize_text_for_naist_jdic,
};
use std::ops::Range;

// Trained model vocabulary: numeric token identities are an interoperability
// contract, independent of the phonemizer implementation. The Japanese subset
// uses these IDs in the public 112-token JP-Extra ONNX input alphabet.
const PHONE_IDS: &[(&str, i64)] = &[
    ("_", 0),
    ("N", 5),
    ("a", 8),
    ("a:", 9),
    ("b", 19),
    ("by", 20),
    ("ch", 22),
    ("d", 23),
    ("dy", 25),
    ("e", 26),
    ("e:", 27),
    ("f", 34),
    ("g", 35),
    ("gy", 36),
    ("h", 37),
    ("hy", 39),
    ("i", 40),
    ("i:", 42),
    ("j", 55),
    ("k", 57),
    ("ky", 58),
    ("m", 60),
    ("my", 61),
    ("n", 62),
    ("ny", 64),
    ("o", 65),
    ("o:", 66),
    ("p", 71),
    ("py", 72),
    ("q", 73),
    ("r", 74),
    ("ry", 75),
    ("s", 76),
    ("sh", 77),
    ("t", 78),
    ("ts", 80),
    ("ty", 81),
    ("u", 82),
    ("u:", 83),
    ("w", 97),
    ("y", 99),
    ("z", 100),
    ("zy", 102),
    ("v", 93),
    ("!", 103),
    ("?", 104),
    ("…", 105),
    (",", 106),
    (".", 107),
    ("'", 108),
    ("-", 109),
];
fn phone_id(phone: &str) -> Result<i64> {
    PHONE_IDS
        .iter()
        .find(|(p, _)| *p == phone)
        .map(|(_, id)| *id)
        .with_context(|| format!("JP-Extra model has no phoneme {phone}"))
}
fn canonical(phone: &str) -> String {
    match phone {
        "cl" => "q".into(),
        "A" => "a".into(),
        "I" => "i".into(),
        "U" => "u".into(),
        "E" => "e".into(),
        "O" => "o".into(),
        other => other.into(),
    }
}
fn punctuation(c: char) -> Option<&'static str> {
    Some(match c {
        '。' | '．' | '.' => ".",
        '、' | '，' | ',' | '：' | '；' | ':' | ';' => ",",
        '！' | '!' => "!",
        '？' | '?' => "?",
        '…' | '⋯' => "…",
        '「' | '」' | '『' | '』' | '（' | '）' | '(' | ')' | '“' | '”' | '‘' | '’' | '\''
        | '"' => "'",
        _ => return None,
    })
}

#[derive(Clone, Debug)]
pub(crate) struct Phone {
    pub id: i64,
    pub tone: i64,
    pub bert_char: usize,
    pub source: Option<Range<usize>>,
}
#[derive(Clone, Debug)]
pub(crate) struct PreparedJapanese {
    pub text: String,
    pub phones: Vec<Phone>,
}
pub(crate) struct JapaneseFrontend {
    processor: JPreprocess<DefaultTokenizer>,
}
impl JapaneseFrontend {
    pub fn new() -> Result<Self> {
        let dictionary =
            SystemDictionaryConfig::Bundled(JPreprocessDictionaryKind::NaistJdic).load()?;
        Ok(Self {
            processor: JPreprocess::with_dictionaries(dictionary, None),
        })
    }
    pub fn prepare(&self, text: &str) -> Result<PreparedJapanese> {
        ensure!(
            !text.trim().is_empty() && text.chars().count() <= 1200,
            "Aivis synthesis requires 1–1200 text characters per chunk"
        );
        let normalized = normalize_with_sources(text);
        let mut njd = self.processor.text_to_njd(&normalized.text)?;
        njd.preprocess();
        let features = njd.nodes.iter().map(ToString::to_string).collect();
        let labels = self.processor.make_label(features);
        let label_phones = labels
            .iter()
            .filter(|label| !matches!(label.phoneme.c.as_deref(), None | Some("sil" | "pau")))
            .collect::<Vec<_>>();
        let rewritten = njd.nodes.iter().map(|n| n.get_string()).collect::<String>();
        let rewritten_map = align_rewrite(&normalized, &rewritten);
        let mut phones = vec![Phone {
            id: 0,
            tone: 0,
            bert_char: 0,
            source: None,
        }];
        let mut bert_text = String::new();
        let mut label_cursor = 0usize;
        let mut character_cursor = 0usize;
        let mut bert_char_cursor = 0usize;
        for node in &njd.nodes {
            let chars = node.get_string().chars().collect::<Vec<_>>();
            let from = character_cursor;
            character_cursor += chars.len();
            let source = union(
                rewritten_map[from..character_cursor]
                    .iter()
                    .flatten()
                    .cloned(),
            );
            let mut char_slots = Vec::new();
            for c in chars {
                if c.is_whitespace() {
                    continue;
                }
                let c = punctuation(c).and_then(|s| s.chars().next()).unwrap_or(c);
                bert_text.push(c);
                bert_char_cursor += 1;
                char_slots.push(bert_char_cursor);
            }
            if char_slots.is_empty() {
                continue;
            }
            let pron = node.get_pron();
            if pron.is_question() || pron.is_touten() {
                for (index, c) in node
                    .get_string()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .enumerate()
                {
                    if let Some(symbol) = punctuation(c) {
                        phones.push(Phone {
                            id: phone_id(symbol)?,
                            tone: 0,
                            bert_char: char_slots[index.min(char_slots.len() - 1)],
                            source: source.clone(),
                        });
                    }
                }
                continue;
            }
            let mut word_phones = Vec::new();
            let mut last_vowel: Option<String> = None;
            for mora in pron.moras() {
                let (consonant, vowel) = mora.phonemes();
                if let Some(consonant) = consonant {
                    let consonant = consonant.to_string();
                    if consonant == "-" {
                        if let Some(v) = &last_vowel {
                            word_phones.push(v.clone());
                        } else {
                            bail!("Long vowel has no preceding vowel");
                        }
                    } else {
                        word_phones.push(canonical(&consonant));
                    }
                }
                if let Some(vowel) = vowel {
                    let vowel = canonical(&vowel.to_string());
                    last_vowel = Some(vowel.clone());
                    word_phones.push(vowel);
                }
            }
            let count = word_phones.len();
            for (index, phone) in word_phones.iter().enumerate() {
                let label = label_phones
                    .get(label_cursor)
                    .context("NJD and full-context phoneme counts differ")?;
                ensure!(
                    canonical(label.phoneme.c.as_deref().unwrap()) == *phone,
                    "NJD/full-context phoneme disagreement"
                );
                label_cursor += 1;
                let mora = label.mora.as_ref().context("Missing mora position")?;
                let phrase = label
                    .accent_phrase_curr
                    .as_ref()
                    .context("Missing accent phrase")?;
                let accent = phrase.accent_position;
                let high = if mora.position_forward == 1 {
                    accent == 1
                } else {
                    accent == 0 || mora.position_forward <= accent
                };
                // BERT feature expansion follows the model's balanced character allocation.
                // This is feature association, never an estimate of speech timestamps.
                let slot = balanced_slot(index, count, char_slots.len());
                phones.push(Phone {
                    id: phone_id(phone)?,
                    tone: i64::from(high),
                    bert_char: char_slots[slot],
                    source: source.clone(),
                });
            }
        }
        ensure!(
            label_cursor == label_phones.len() && label_cursor > 0,
            "Text contains unresolved phonemes"
        );
        phones.push(Phone {
            id: 0,
            tone: 0,
            bert_char: bert_char_cursor + 1,
            source: None,
        });
        Ok(PreparedJapanese {
            text: bert_text,
            phones,
        })
    }
}
fn balanced_slot(index: usize, phones: usize, chars: usize) -> usize {
    let quotient = phones / chars;
    let extra = phones % chars;
    let wide = (quotient + 1) * extra;
    if index < wide {
        index / (quotient + 1)
    } else {
        extra + (index - wide) / quotient.max(1)
    }
}
fn union(ranges: impl Iterator<Item = Range<usize>>) -> Option<Range<usize>> {
    ranges.fold(None, |a, b| {
        Some(match a {
            None => b,
            Some(a) => a.start.min(b.start)..a.end.max(b.end),
        })
    })
}
struct Normalized {
    text: String,
    sources: Vec<Range<usize>>,
}
fn normalize_with_sources(text: &str) -> Normalized {
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut out = Normalized {
        text: String::new(),
        sources: Vec::new(),
    };
    let mut i = 0;
    while i < chars.len() {
        let (start, _) = chars[i];
        let mut end = i + 1;
        while end < chars.len()
            && matches!(
                chars[end].1,
                '\u{3099}' | '\u{309a}' | '\u{309b}' | '\u{309c}' | '\u{ff9e}' | '\u{ff9f}'
            )
        {
            end += 1;
        }
        let byte_end = chars.get(end).map(|(n, _)| *n).unwrap_or(text.len());
        let value = normalize_text_for_naist_jdic(&text[start..byte_end]);
        out.sources.extend(value.chars().map(|_| start..byte_end));
        out.text.push_str(&value);
        i = end;
    }
    out
}
/// Monotone character edit alignment. A rewritten number or contraction keeps
/// the entire original changed range; it never invents per-character precision.
fn align_rewrite(original: &Normalized, rewritten: &str) -> Vec<Option<Range<usize>>> {
    let a = original.text.chars().collect::<Vec<_>>();
    let b = rewritten.chars().collect::<Vec<_>>();
    let width = b.len() + 1;
    let mut lengths = vec![0u16; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lengths[i * width + j] = if a[i] == b[j] {
                1 + lengths[(i + 1) * width + j + 1]
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }
    let mut matched = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            matched.push((i, j));
            i += 1;
            j += 1;
        } else if lengths[(i + 1) * width + j] >= lengths[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut result = vec![None; b.len()];
    let (mut old, mut new) = (0, 0);
    for (i, j) in matched
        .into_iter()
        .chain(std::iter::once((a.len(), b.len())))
    {
        let changed = union(original.sources[old..i].iter().cloned());
        for target in &mut result[new..j] {
            *target = changed.clone();
        }
        if i < a.len() {
            result[j] = Some(original.sources[i].clone());
        }
        old = i + 1;
        new = j + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_rewrites_group_numbers_and_combining_kana() {
        let original = normalize_with_sources("2026年ｶﾞ");
        let mapped = align_rewrite(&original, "二千二十六年ガ");
        assert!(mapped[..5].iter().all(|r| *r == Some(0..4)));
        assert_eq!(mapped[6], Some(7..13));
    }
    #[test]
    fn balanced_features_do_not_claim_character_audio_times() {
        let slots = (0..9).map(|i| balanced_slot(i, 9, 5)).collect::<Vec<_>>();
        assert_eq!(slots, [0, 0, 1, 1, 2, 2, 3, 3, 4]);
    }
    #[test]
    fn full_context_frontend_handles_kanji_digits_long_vowels_quotes_and_halfwidth_kana() {
        let frontend = JapaneseFrontend::new().unwrap();
        for text in [
            "東京の図書館で本を読みます。",
            "2026年に3冊の本を読んだ。",
            "「ｶﾞｯｺｳへ行きます！」 Rustも読みます。",
            "コーヒーとケーキを買った。",
        ] {
            let prepared = frontend.prepare(text).unwrap();
            assert!(prepared.phones.len() > 3);
            for phone in prepared.phones {
                if let Some(source) = phone.source {
                    assert!(source.start < source.end && source.end <= text.len());
                    assert!(
                        text.is_char_boundary(source.start) && text.is_char_boundary(source.end)
                    );
                }
            }
        }
    }
    #[test]
    fn greeting_uses_full_context_accent_and_keeps_utf8_source_ranges() {
        let frontend = JapaneseFrontend::new().unwrap();
        let prepared = frontend.prepare("こんにちは。").unwrap();
        assert_eq!(
            prepared.phones.iter().map(|p| p.id).collect::<Vec<_>>(),
            [0, 57, 65, 5, 62, 40, 22, 40, 97, 8, 107, 0]
        );
        assert_eq!(prepared.phones[1].tone, 0);
        assert_eq!(prepared.phones[3].tone, 1);
        assert!(
            prepared.phones[1..10]
                .iter()
                .all(|p| p.source == Some(0..15))
        );
    }
}
