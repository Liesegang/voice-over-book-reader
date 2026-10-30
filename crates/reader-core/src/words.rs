//! Morphological click targets share the same source mapping as native glyphs.
use crate::{Atom, Block, Chapter, Locator};
use anyhow::{Result, anyhow};
use lindera::{dictionary::load_dictionary, mode::Mode, segmenter::Segmenter};
use std::{
    borrow::Cow,
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordSpan {
    pub text: String,
    pub start: Locator,
    pub end: Locator,
    /// Index into Chapter::atoms(); this is the speech unit's low 32 bits.
    pub atom_index: usize,
    /// UTF-8 byte offset inside that first atom, for point-to-speak.
    pub offset: usize,
    pub lemma: Option<String>,
}

static SEGMENTER: OnceLock<std::result::Result<Segmenter, String>> = OnceLock::new();
type CachedWords = VecDeque<(blake3::Hash, Arc<Vec<WordSpan>>)>;
static WORDS: Mutex<CachedWords> = Mutex::new(VecDeque::new());

fn segmenter() -> Result<&'static Segmenter> {
    SEGMENTER
        .get_or_init(|| {
            load_dictionary("embedded://ipadic")
                .map(|dictionary| Segmenter::new(Mode::Normal, dictionary, None))
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow!("Load embedded Japanese word dictionary: {error}"))
}

/// Finds the actual word containing a clicked source position. The bounded
/// chapter cache is built on demand, never during every rendered frame.
pub fn word_at(chapter: &Chapter, locator: Locator) -> Result<Option<WordSpan>> {
    if chapter.id != locator.chapter {
        return Ok(None);
    }
    let mut hash = blake3::Hasher::new();
    hash.update(&chapter.id.to_le_bytes());
    fn hash_blocks(hash: &mut blake3::Hasher, blocks: &[Block]) {
        hash.update(&(blocks.len() as u64).to_le_bytes());
        for block in blocks {
            // Words cross style nodes, but never cross paragraph/cell boundaries.
            // Identical text-node IDs in another book must not reuse that grouping.
            hash.update(&block.id.to_le_bytes());
            hash.update(&[u8::from(block.mathml.is_some())]);
            hash.update(&(block.atoms.len() as u64).to_le_bytes());
            for atom in &block.atoms {
                hash.update(&(atom.text.len() as u64).to_le_bytes());
                hash.update(atom.text.as_bytes());
                hash.update(&[u8::from(atom.ruby.is_some())]);
                if let Some(ruby) = &atom.ruby {
                    hash.update(ruby.as_bytes());
                }
                for source in &atom.sources {
                    hash.update(&source.node.to_le_bytes());
                    hash.update(&source.start.to_le_bytes());
                    hash.update(&source.end.to_le_bytes());
                }
            }
            if let Some(table) = &block.table {
                for row in &table.rows {
                    for cell in &row.cells {
                        hash_blocks(hash, &cell.blocks);
                    }
                }
            }
        }
    }
    hash_blocks(&mut hash, &chapter.blocks);
    let key = hash.finalize();
    let cached = WORDS
        .lock()
        .map_err(|_| anyhow!("Word cache lock was poisoned"))?
        .iter()
        .find(|(cached_key, _)| *cached_key == key)
        .map(|(_, words)| Arc::clone(words));
    let words = if let Some(cached) = cached {
        cached
    } else {
        let words = Arc::new(index_chapter(chapter)?);
        let mut cache = WORDS
            .lock()
            .map_err(|_| anyhow!("Word cache lock was poisoned"))?;
        if cache.len() >= 4 {
            cache.pop_front();
        }
        cache.push_back((key, Arc::clone(&words)));
        words
    };
    Ok(words
        .iter()
        .find(|word| word.start <= locator && locator < word.end)
        .cloned())
}

fn index_chapter(chapter: &Chapter) -> Result<Vec<WordSpan>> {
    fn walk_blocks(nodes: &[Block], index: &mut usize, words: &mut Vec<WordSpan>) -> Result<()> {
        for block in nodes {
            let atoms: Vec<_> = block
                .atoms
                .iter()
                .enumerate()
                .map(|(local, atom)| (*index + local, atom))
                .collect();
            *index += block.atoms.len();
            if block.mathml.is_none() {
                words.extend(index_atoms(&atoms)?);
            }
            if let Some(table) = &block.table {
                for row in &table.rows {
                    for cell in &row.cells {
                        walk_blocks(&cell.blocks, index, words)?;
                    }
                }
            }
        }
        Ok(())
    }
    let mut words = Vec::new();
    walk_blocks(&chapter.blocks, &mut 0, &mut words)?;
    Ok(words)
}

fn index_atoms(atoms: &[(usize, &Atom)]) -> Result<Vec<WordSpan>> {
    let text = atoms
        .iter()
        .map(|(_, atom)| atom.text.as_str())
        .collect::<String>();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut locations = Vec::new();
    let mut byte = 0;
    for &(index, atom) in atoms {
        locations.push((byte, byte + atom.text.len(), index, atom));
        byte += atom.text.len();
    }
    let locate = |offset: usize, end: bool| {
        locations.iter().find_map(|&(start, stop, index, atom)| {
            if offset >= start && (offset < stop || (end && offset == stop)) {
                let local = offset - start;
                atom.locator_at(local)
                    .map(|locator| (locator, index, local))
            } else {
                None
            }
        })
    };
    let ascii = ascii_words(&text);
    let mut tokens = segmenter()?
        .segment(Cow::Borrowed(&text))
        .map_err(|error| anyhow!("Segment Japanese text: {error}"))?;
    let mut output = Vec::new();
    let mut seen = Vec::new();
    for token in &mut tokens {
        if !token.surface.chars().any(char::is_alphanumeric) {
            continue;
        }
        let (mut start, mut end) = (token.byte_start, token.byte_end);
        if let Some(&(word_start, word_end)) = ascii.iter().find(|&&(s, e)| s <= start && end <= e)
        {
            start = word_start;
            end = word_end;
        }
        for &(ruby_start, ruby_end, _, atom) in &locations {
            if atom.ruby.is_some() && start < ruby_end && end > ruby_start {
                start = start.min(ruby_start);
                end = end.max(ruby_end);
            }
        }
        if seen.contains(&(start, end)) {
            continue;
        }
        seen.push((start, end));
        let Some((start_locator, atom_index, offset)) = locate(start, false) else {
            continue;
        };
        let Some((end_locator, _, _)) = locate(end, true) else {
            continue;
        };
        let Some(surface) = text.get(start..end) else {
            continue;
        };
        let lemma = if start == token.byte_start && end == token.byte_end {
            token
                .get("base_form")
                .map(str::to_owned)
                .or_else(|| token.get_detail(6).map(str::to_owned))
                .filter(|lemma| lemma != "*" && lemma != surface)
        } else {
            None
        };
        output.push(WordSpan {
            text: surface.to_owned(),
            start: start_locator,
            end: end_locator,
            atom_index,
            offset,
            lemma,
        });
    }
    output.sort_by_key(|word| word.start);
    Ok(output)
}

fn ascii_words(text: &str) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    for (offset, ch) in text.char_indices() {
        let connector = matches!(ch, '\'' | '-' | '.')
            && start.is_some()
            && text[offset + ch.len_utf8()..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_alphanumeric());
        if ch.is_ascii_alphanumeric() || ch == '_' || connector {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            words.push((begin, offset));
        }
    }
    if let Some(begin) = start {
        words.push((begin, text.len()));
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_html;

    #[test]
    fn kanji_click_resolves_morphological_word_start_and_surface() {
        let chapter = parse_html(
            "<html><body><p>日本語の辞書を使います。</p></body></html>",
            0,
            "x.xhtml",
            "",
        )
        .unwrap();
        let atom = chapter.atoms()[0];
        let word = word_at(&chapter, atom.locator_at("日".len()).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(word.text, "日本語");
        assert_eq!(word.start, atom.locator_at(0).unwrap());
        assert_eq!(word.offset, 0);
    }

    #[test]
    fn words_cross_style_nodes_but_never_split_group_ruby() {
        let chapter = parse_html("<html><body><p>日<strong>本語</strong>と<ruby>関西国際空港<rt>かんさいこくさいくうこう</rt></ruby>。</p></body></html>", 0, "x.xhtml", "").unwrap();
        let atoms = chapter.atoms();
        let first = word_at(&chapter, atoms[1].locator_at(0).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(first.text, "日本語");
        assert_eq!(first.atom_index, 0);
        let ruby = atoms.iter().find(|atom| atom.ruby.is_some()).unwrap();
        let word = word_at(&chapter, ruby.locator_at("関西".len()).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(word.text, "関西国際空港");
        assert_eq!(word.start, ruby.locator_at(0).unwrap());
    }

    #[test]
    fn english_identifier_click_returns_whole_word() {
        let chapter = parse_html(
            "<html><body><p>Read native_reader123 now.</p></body></html>",
            0,
            "x.xhtml",
            "",
        )
        .unwrap();
        let atom = chapter
            .atoms()
            .into_iter()
            .find(|atom| atom.text.contains("native"))
            .unwrap();
        let word = word_at(&chapter, atom.locator_at(8).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(word.text, "native_reader123");
    }

    #[test]
    fn cached_words_respect_block_boundaries_in_other_books() {
        let separate = parse_html(
            "<html><body><p>日</p><p>本語</p></body></html>",
            0,
            "words.xhtml",
            "",
        )
        .unwrap();
        let locator = separate.atoms()[1].locator_at(0).unwrap();
        assert_ne!(word_at(&separate, locator).unwrap().unwrap().text, "日本語");
        let mut combined = separate.clone();
        let second = combined.blocks.pop().unwrap();
        combined.blocks[0].atoms.extend(second.atoms);
        assert_eq!(word_at(&combined, locator).unwrap().unwrap().text, "日本語");
    }
}
