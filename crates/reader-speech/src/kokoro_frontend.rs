//! Japanese Kokoro frontend: native MeCab + the model's bundled Misaki tables.
//! Phoneme rules follow audio.cpp's Apache-2.0 multilingual frontend; model
//! resources retain their own licenses. No Python or web process is involved.
//! Locally adapted to Rust with source mapping; see licenses/kokoro/NOTICE.txt
//! for audio.cpp, Misaki, and original Cutlet attribution and modification notes.
use anyhow::{Context, Result, bail, ensure};
use libloading::Library;
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, CString, c_char, c_void},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

fn u32r(file: &mut File) -> Result<u32> {
    let mut bytes = [0; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn u64r(file: &mut File) -> Result<u64> {
    let mut bytes = [0; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
fn string(file: &mut File) -> Result<String> {
    let length = u64r(file)?;
    ensure!(length <= 1024 * 1024, "Oversize GGUF string");
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}
fn skip(file: &mut File, kind: u32, depth: u8) -> Result<()> {
    ensure!(depth < 8, "Invalid GGUF metadata nesting");
    let bytes = match kind {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        8 => u64r(file)?,
        9 => {
            let item = u32r(file)?;
            let count = u64r(file)?;
            ensure!(count <= 2_000_000_000, "Invalid GGUF metadata count");
            let width = match item {
                0 | 1 | 7 => 1,
                2 | 3 => 2,
                4..=6 => 4,
                10..=12 => 8,
                _ => 0,
            };
            if width != 0 {
                file.seek(SeekFrom::Current((count * width) as i64))?;
            } else {
                for _ in 0..count {
                    skip(file, item, depth + 1)?;
                }
            }
            return Ok(());
        }
        _ => bail!("Unsupported GGUF metadata type"),
    };
    ensure!(bytes <= 2_000_000_000, "Oversize GGUF field");
    file.seek(SeekFrom::Current(bytes as i64))?;
    Ok(())
}

/// Read only the Japanese frontend resources from the verified GGUF. Neural
/// tensor data and unrelated language files are never expanded or allocated.
pub(crate) fn extract(gguf: &Path, root: &Path) -> Result<()> {
    if root.join("frontend-v3.ready").is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(root)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("frontend.lock"))?;
    lock.lock()?;
    if root.join("frontend-v3.ready").is_file() {
        return Ok(());
    }
    let mut file = File::open(gguf)?;
    let total = file.metadata()?.len();
    let mut magic = [0; 4];
    file.read_exact(&mut magic)?;
    ensure!(
        &magic == b"GGUF" && u32r(&mut file)? == 3,
        "Unsupported Kokoro resource package"
    );
    let _ = u64r(&mut file)?;
    let count = u64r(&mut file)?;
    ensure!(count <= 100_000, "Invalid GGUF metadata count");
    let mut names = Vec::new();
    let mut offsets = Vec::new();
    let mut data = None;
    for _ in 0..count {
        let key = string(&mut file)?;
        let kind = u32r(&mut file)?;
        match key.as_str() {
            "audiocpp.embedded_files.names" => {
                ensure!(kind == 9 && u32r(&mut file)? == 8, "Invalid resource names");
                let n = u64r(&mut file)?;
                ensure!(n <= 4096, "Too many Kokoro resources");
                for _ in 0..n {
                    names.push(string(&mut file)?);
                }
            }
            "audiocpp.embedded_files.offsets" => {
                ensure!(
                    kind == 9 && u32r(&mut file)? == 10,
                    "Invalid resource offsets"
                );
                let n = u64r(&mut file)?;
                ensure!(n <= 4097, "Too many Kokoro offsets");
                for _ in 0..n {
                    offsets.push(u64r(&mut file)?);
                }
            }
            "audiocpp.embedded_files.data" => {
                ensure!(kind == 9 && u32r(&mut file)? == 0, "Invalid resource bytes");
                let size = u64r(&mut file)?;
                let start = file.stream_position()?;
                ensure!(
                    start.checked_add(size).is_some_and(|end| end <= total),
                    "Truncated Kokoro resources"
                );
                data = Some((start, size));
                file.seek(SeekFrom::Current(size as i64))?;
            }
            _ => skip(&mut file, kind, 0)?,
        }
        if !names.is_empty() && !offsets.is_empty() && data.is_some() {
            break;
        }
    }
    let (data, size) = data.context("Kokoro package has no embedded frontend")?;
    ensure!(
        offsets.len() == names.len() + 1
            && offsets.windows(2).all(|pair| pair[0] <= pair[1])
            && offsets.last().is_some_and(|last| *last <= size),
        "Invalid embedded resource ranges"
    );
    for (index, name) in names.iter().enumerate() {
        let wanted = name == "config.json"
            || name == "g2p/ja.json"
            || name == "g2p/versions.json"
            || name.starts_with("licenses/")
            || name.starts_with("unidic/")
            || name.starts_with("voices/jf_")
            || name.starts_with("voices/jm_");
        if !wanted {
            continue;
        }
        ensure!(
            Path::new(name)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "Invalid frontend resource path"
        );
        let target = root.join(name);
        std::fs::create_dir_all(target.parent().unwrap())?;
        let length = offsets[index + 1] - offsets[index];
        ensure!(length <= 1024 * 1024 * 1024, "Oversize resource");
        if target
            .metadata()
            .is_ok_and(|metadata| metadata.len() == length)
        {
            continue;
        }
        file.seek(SeekFrom::Start(data + offsets[index]))?;
        let mut output = File::create(&target)?;
        let copied = std::io::copy(&mut Read::by_ref(&mut file).take(length), &mut output)?;
        ensure!(copied == length, "Truncated frontend resource");
    }
    ensure!(
        root.join("unidic/sys.dic").is_file() && root.join("g2p/ja.json").is_file(),
        "Kokoro package omits Japanese frontend"
    );
    File::create(root.join("frontend-v3.ready"))?.write_all(b"3")?;
    Ok(())
}

#[repr(C)]
struct Node {
    prev: *const Node,
    next: *const Node,
    enext: *const Node,
    bnext: *const Node,
    rpath: *const c_void,
    lpath: *const c_void,
    surface: *const c_char,
    feature: *const c_char,
    id: u32,
    length: u16,
    rlength: u16,
    rc_attr: u16,
    lc_attr: u16,
    posid: u16,
    char_type: u8,
    stat: u8,
}
#[derive(Clone, Debug)]
pub(crate) struct Phone {
    pub id: i64,
    pub source: Option<std::ops::Range<usize>>,
}
type MappedPhone = (char, Option<std::ops::Range<usize>>);

// Misaki Cutlet removes word-separating spaces around a sokuon after
// whitespace normalization. Keeping "aʔ to" or "naʔ ʨaʔ ta" gives Kokoro
// false prosodic boundaries. Punctuation and opening quotes still separate
// phrases; removing only the separator preserves every lexical source range.
fn join_sokuon(phones: Vec<MappedPhone>) -> Vec<MappedPhone> {
    let mut normalized = Vec::<MappedPhone>::new();
    for (mut ch, source) in phones {
        if ch.is_whitespace() {
            ch = ' ';
            if normalized.is_empty() || normalized.last().is_some_and(|(ch, _)| *ch == ' ') {
                continue;
            }
        }
        normalized.push((ch, source));
    }
    while normalized.last().is_some_and(|(ch, _)| *ch == ' ') {
        normalized.pop();
    }
    normalized
        .iter()
        .enumerate()
        .filter_map(|(index, phone)| {
            let before = index.checked_sub(1).map(|i| normalized[i].0);
            let after = normalized.get(index + 1).map(|(ch, _)| *ch);
            let before_sokuon =
                after == Some('ʔ') && before.is_none_or(|ch| !"!\",.:;?»)—…”".contains(ch));
            let after_sokuon = before == Some('ʔ') && after.is_none_or(|ch| !"\"«“(".contains(ch));
            (phone.0 != ' ' || !(before_sokuon || after_sokuon)).then(|| phone.clone())
        })
        .collect()
}

pub(crate) struct Japanese {
    _library: Library,
    tagger: *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    parse: unsafe extern "C" fn(*mut c_void, *const c_char) -> *const Node,
    kana: HashMap<String, String>,
    words: HashSet<String>,
    vocab: HashMap<char, i64>,
    root: PathBuf,
}
// One synthesis worker owns the tagger. No handle or MeCab node escapes it.
unsafe impl Send for Japanese {}
impl Drop for Japanese {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.tagger) }
    }
}
impl Japanese {
    pub fn new(root: &Path, library_path: &Path) -> Result<Self> {
        #[cfg(windows)]
        let library = unsafe {
            libloading::os::windows::Library::load_with_flags(library_path, 0x100 | 0x1000)?.into()
        };
        #[cfg(not(windows))]
        let library = unsafe { Library::new(library_path)? };
        let library: Library = library;
        let create = unsafe {
            *library
                .get::<unsafe extern "C" fn(i32, *mut *mut c_char) -> *mut c_void>(b"mecab_new\0")?
        };
        let destroy =
            unsafe { *library.get::<unsafe extern "C" fn(*mut c_void)>(b"mecab_destroy\0")? };
        let parse = unsafe {
            *library.get::<unsafe extern "C" fn(*mut c_void, *const c_char) -> *const Node>(
                b"mecab_sparse_tonode\0",
            )?
        };
        let strings = [
            "mecab".to_owned(),
            "-r".into(),
            root.join("unidic/dicrc").to_string_lossy().into_owned(),
            "-d".into(),
            root.join("unidic").to_string_lossy().into_owned(),
        ];
        let strings: Vec<CString> = strings
            .into_iter()
            .map(CString::new)
            .collect::<std::result::Result<_, _>>()?;
        let mut argv: Vec<_> = strings.iter().map(|s| s.as_ptr() as *mut c_char).collect();
        let tables: serde_json::Value =
            serde_json::from_reader(File::open(root.join("g2p/ja.json"))?)?;
        let config: serde_json::Value =
            serde_json::from_reader(File::open(root.join("config.json"))?)?;
        let kana = tables["kana"]
            .as_object()
            .context("Missing kana table")?
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|value| (k.clone(), value.to_owned())))
            .collect();
        let words = tables["words"]
            .as_array()
            .context("Missing lexical grouping")?
            .iter()
            .filter_map(|word| word.as_str().map(str::to_owned))
            .collect();
        let vocab = config["vocab"]
            .as_object()
            .context("Missing Kokoro vocabulary")?
            .iter()
            .filter_map(|(key, value)| Some((key.chars().next()?, value.as_i64()?)))
            .collect();
        let tagger = unsafe { create(argv.len() as i32, argv.as_mut_ptr()) };
        ensure!(!tagger.is_null(), "MeCabの日本語辞書を開けません");
        Ok(Self {
            _library: library,
            tagger,
            destroy,
            parse,
            kana,
            words,
            vocab,
            root: root.to_path_buf(),
        })
    }
    pub fn voice(&self, id: &str) -> Result<Vec<f32>> {
        let bytes = std::fs::read(self.root.join(format!("voices/{id}.bin")))?;
        ensure!(
            bytes.len() == 510 * 256 * 4,
            "Invalid Kokoro voice embedding"
        );
        Ok(bytes
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect())
    }
    pub fn tokens(&self, text: &str) -> Result<Vec<Phone>> {
        #[derive(Clone)]
        struct Word {
            surface: String,
            reading: String,
            kind: u8,
            source: std::ops::Range<usize>,
        }
        let (normalized, source_map) = normalize_mapped(text);
        let text = CString::new(normalized.as_str())?;
        let mut cursor = 0usize;
        let mut node = unsafe { (self.parse)(self.tagger, text.as_ptr()) };
        ensure!(!node.is_null(), "Japanese pronunciation failed");
        let mut words = Vec::<Word>::new();
        let mut nodes = 0;
        while !node.is_null() {
            nodes += 1;
            ensure!(nodes <= 100_000, "Too many MeCab nodes");
            let current = unsafe { &*node };
            if current.stat < 2 && current.length > 0 {
                let surface = std::str::from_utf8(unsafe {
                    std::slice::from_raw_parts(
                        current.surface.cast::<u8>(),
                        current.length as usize,
                    )
                })?
                .to_owned();
                let start = cursor
                    + normalized[cursor..]
                        .find(&surface)
                        .context("MeCab source text mismatch")?;
                let end = start + surface.len();
                let source = source_map[start].start..source_map[end - 1].end;
                cursor = end;
                let features = unsafe { CStr::from_ptr(current.feature) }.to_str()?;
                let fields: Vec<_> = features.split(',').collect();
                let pronunciation = fields
                    .get(9)
                    .copied()
                    .filter(|value| !value.is_empty() && *value != "*")
                    .or_else(|| {
                        fields
                            .get(17)
                            .copied()
                            .filter(|value| !value.is_empty() && *value != "*")
                    })
                    .unwrap_or(&surface);
                if pronunciation != "*" {
                    let pronunciation = if pronunciation == surface
                        && surface.is_ascii()
                        && surface.chars().any(|ch| ch.is_ascii_alphabetic())
                    {
                        ascii_spelling(&surface)
                    } else {
                        pronunciation.to_owned()
                    };
                    let reading = pronunciation
                        .chars()
                        .map(|ch| {
                            if ('ァ'..='ヶ').contains(&ch) {
                                char::from_u32(ch as u32 - 0x60).unwrap()
                            } else {
                                ch
                            }
                        })
                        .collect();
                    let kind = if current.char_type == 7 || current.stat == 0 || surface.is_ascii()
                    {
                        6
                    } else {
                        current.char_type
                    };
                    words.push(Word {
                        source,
                        surface,
                        reading,
                        kind,
                    });
                }
            }
            node = current.next;
        }
        let mut index = 0;
        while index < words.len() {
            let mut combined = String::new();
            let mut last = index;
            for next in index..words.len() {
                if words[next].kind != words[index].kind {
                    break;
                }
                combined.push_str(&words[next].surface);
                if self.words.contains(&combined) {
                    last = next;
                }
            }
            while last > index {
                let other = words.remove(index + 1);
                words[index].source.end = other.source.end;
                words[index].surface.push_str(&other.surface);
                words[index].reading.push_str(&other.reading);
                last -= 1;
            }
            index += 1;
        }
        let mut phones = Vec::<MappedPhone>::new();
        for word in words {
            let mut segment = String::new();
            if word.reading.is_ascii() || word.reading.chars().all(|ch| "“”‘’—…".contains(ch))
            {
                segment = word.reading;
            } else if word.kind == 6 || word.kind == 3 {
                let chars: Vec<_> = word.reading.chars().collect();
                for (index, ch) in chars.iter().copied().enumerate() {
                    let prev: String = chars[index.saturating_sub(1)..=index].iter().collect();
                    let next: String = chars[index..(index + 2).min(chars.len())].iter().collect();
                    if index > 0 && self.kana.contains_key(&prev) {
                        segment.push_str(&self.kana[&prev]);
                        continue;
                    }
                    if next.chars().count() == 2 && self.kana.contains_key(&next) {
                        continue;
                    }
                    match ch {
                        'ー' => segment.push('ː'),
                        'っ' => segment.push('ʔ'),
                        'ん' => {
                            let after = chars
                                .get(index + 1)
                                .and_then(|ch| self.kana.get(&ch.to_string()))
                                .and_then(|s| s.chars().next());
                            segment.push(match after {
                                Some('m' | 'p' | 'b') => 'm',
                                Some('k' | 'ɡ') => 'ŋ',
                                Some('ɲ' | 'ʨ' | 'ʥ') => 'ɲ',
                                Some('n' | 't' | 'd' | 'ɾ' | 'z') => 'n',
                                _ => 'ɴ',
                            });
                        }
                        'ゃ' | 'ゅ' | 'ょ' | 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' => {}
                        _ => {
                            if let Some(value) = self.kana.get(&ch.to_string()) {
                                segment.push_str(value);
                            } else if "!\"(),.:;?—“”…".contains(ch) {
                                segment.push(ch);
                            } else if !ch.is_whitespace() && ch != '\u{3099}' && ch != '\u{309a}' {
                                // Misaki pron2moras and audio.cpp's own G2P
                                // skip unsupported well-formed symbols. One
                                // unknown mark must not discard the sentence.
                                tracing::debug!(character=%ch, "Kokoro skipped an unsupported pronunciation symbol");
                            }
                        }
                    }
                }
            }
            if segment.is_empty() {
                continue;
            }
            if segment.chars().count() == 1 && "]).,?!:”»".contains(segment.as_str()) {
                while phones.last().is_some_and(|(ch, _)| ch.is_whitespace()) {
                    phones.pop();
                }
            }
            phones.extend(segment.chars().map(|ch| (ch, Some(word.source.clone()))));
            if !(segment.chars().count() == 1 && "([“".contains(segment.as_str())) {
                phones.push((' ', None));
            }
        }
        let mut ids = Vec::new();
        for (mut ch, source) in join_sokuon(phones) {
            if ch.is_whitespace() {
                ch = ' ';
                if ids.is_empty()
                    || ids
                        .last()
                        .is_some_and(|phone: &Phone| phone.source.is_none())
                {
                    continue;
                }
            }
            // The original multilingual model vocabulary calls IDs 12/13
            // '(' and ')'. Remapping to guillemets here silently dropped them.
            if let Some(&id) = self.vocab.get(&ch) {
                ids.push(Phone { id, source });
            }
        }
        while ids.last().is_some_and(|phone| phone.source.is_none()) {
            ids.pop();
        }
        if ids.is_empty() {
            // A separator-only chunk contributes a real model pause. It has no
            // lexical source mark, so no fine timing is fabricated for a glyph
            // which the phonemizer cannot pronounce.
            ids.push(Phone {
                id: *self.vocab.get(&'.').context("Kokoro has no pause token")?,
                source: None,
            });
        }
        Ok(ids)
    }
}

#[cfg(test)]
fn normalize(text: &str) -> String {
    normalize_mapped(text).0
}

fn normalize_mapped(text: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut chars = Vec::<(char, std::ops::Range<usize>)>::new();
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        let ch = match ch {
                    '\u{ff61}'..='\u{ff9f}' => "。「」、・ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン\u{3099}\u{309a}".chars().nth(ch as usize-0xff61).unwrap(),
                    _=>ch,
                };
        let mut ch = match ch {
            '０'..='９' => char::from_u32(ch as u32 - '０' as u32 + '0' as u32).unwrap(),
            '。' => '.',
            '、' => ',',
            '！' => '!',
            '？' => '?',
            '「' | '『' | '《' | '〈' | '【' | '〔' | '［' | '｛' | '〝' | '〘' | '〚' | '〖'
            | '﹁' | '﹃' => '“',
            '」' | '』' | '》' | '〉' | '】' | '〕' | '］' | '｝' | '〞' | '〟' | '〙' | '〛'
            | '〗' | '﹂' | '﹄' => '”',
            '（' => '(',
            '）' => ')',
            '：' => ':',
            '；' => ';',
            '―' | '–' | '─' | '‐' | '‑' | '‒' | '⁃' | '−' | '〜' => '—',
            '‥' | '⋯' => '…',
            '〻' => '々',
            '※' | '・' | '●' | '○' | '■' | '□' | '◆' | '◇' | '★' | '☆' | '†' | '‡' | '⁂' => {
                ' '
            }
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(ch as u32 - 0xfee0).unwrap(),
            ch if ch.is_control() => ' ',
            _ => ch,
        };
        if matches!(ch, '\u{3099}' | '\u{309a}')
            && let Some((previous, range)) = chars.last_mut()
            && let Some(composed) = compose_kana_mark(*previous, ch)
        {
            *previous = composed;
            range.end = end;
            continue;
        }
        if matches!(ch, 'ゝ' | 'ゞ' | 'ヽ' | 'ヾ')
            && let Some((previous, _)) = chars.last()
            && (('ぁ'..='ゖ').contains(previous) || ('ァ'..='ヶ').contains(previous))
        {
            ch = if matches!(ch, 'ゞ' | 'ヾ') {
                compose_kana_mark(*previous, '\u{3099}').unwrap_or(*previous)
            } else {
                *previous
            };
        }
        chars.push((ch, start..end));
    }
    let mut output = String::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (part, range) = if chars[i].0.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].0.is_ascii_digit() {
                i += 1;
            }
            let digits: String = chars[start..i].iter().map(|(ch, _)| *ch).collect();
            let part = if let Ok(value) = digits.parse::<u64>() {
                kanji_number(value)
            } else {
                digits
                    .chars()
                    .map(|ch| {
                        "零一二三四五六七八九"
                            .chars()
                            .nth(ch as usize - '0' as usize)
                            .unwrap()
                    })
                    .collect()
            };
            (part, chars[start].1.start..chars[i - 1].1.end)
        } else {
            let (ch, range) = &chars[i];
            i += 1;
            (
                match ch {
                    'ゟ' => "より".into(),
                    'ヿ' => "コト".into(),
                    _ => ch.to_string(),
                },
                range.clone(),
            )
        };
        map.extend(std::iter::repeat_n(range, part.len()));
        output.push_str(&part);
    }
    (output, map)
}

fn compose_kana_mark(base: char, mark: char) -> Option<char> {
    if mark == '\u{309a}' {
        return "はひふへほハヒフヘホ"
            .chars()
            .find(|ch| *ch == base)
            .and_then(|ch| char::from_u32(ch as u32 + 2));
    }
    if mark != '\u{3099}' {
        return None;
    }
    match base {
        'う'=>Some('ゔ'),'ウ'=>Some('ヴ'),
        'ワ'=>Some('ヷ'),'ヰ'=>Some('ヸ'),'ヱ'=>Some('ヹ'),'ヲ'=>Some('ヺ'),
        ch if "かきくけこさしすせそたちつてとはひふへほカキクケコサシスセソタチツテトハヒフヘホ".contains(ch)=>char::from_u32(ch as u32+1),
        ch if "がぎぐげござじずぜぞだぢづでどばびぶべぼガギグゲゴザジズゼゾダヂヅデドバビブベボゔヴ".contains(ch)=>Some(ch),
        _=>None,
    }
}

fn ascii_spelling(text: &str) -> String {
    const LETTERS: [&str; 26] = [
        "えー",
        "びー",
        "しー",
        "でぃー",
        "いー",
        "えふ",
        "じー",
        "えいち",
        "あい",
        "じぇー",
        "けー",
        "える",
        "えむ",
        "えぬ",
        "おー",
        "ぴー",
        "きゅー",
        "あーる",
        "えす",
        "てぃー",
        "ゆー",
        "ぶい",
        "だぶりゅー",
        "えっくす",
        "わい",
        "ぜっと",
    ];
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphabetic() {
                LETTERS[ch.to_ascii_lowercase() as usize - 'a' as usize].to_owned()
            } else {
                ch.to_string()
            }
        })
        .collect()
}

fn kanji_number(mut value: u64) -> String {
    if value == 0 {
        return "零".into();
    }
    let mut groups = Vec::new();
    let mut index = 0;
    while value > 0 {
        let group = (value % 10000) as usize;
        if group > 0 {
            let mut part = String::new();
            for (divisor, suffix) in [(1000, "千"), (100, "百"), (10, "十"), (1, "")] {
                let digit = group / divisor % 10;
                if digit > 0 {
                    if digit != 1 || divisor == 1 {
                        part.push("零一二三四五六七八九".chars().nth(digit).unwrap());
                    }
                    part.push_str(suffix);
                }
            }
            part.push_str(["", "万", "億", "兆", "京"][index]);
            groups.push(part);
        }
        value /= 10000;
        index += 1;
    }
    groups.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sokuon_join_preserves_phrase_breaks_and_lexical_source_ranges() {
        for (input, expected) in [
            (" aʔ to ", "aʔto"),
            ("naʔ ʨaʔ ta", "naʔʨaʔta"),
            ("jaʔta ʔte", "jaʔtaʔte"),
            ("aʔ,  maʔte", "aʔ, maʔte"),
            ("aʔ “hoɴ”", "aʔ “hoɴ”"),
            ("aʔ (hoɴ)", "aʔ (hoɴ)"),
            ("a. ʔte", "a. ʔte"),
        ] {
            let mapped: Vec<_> = input
                .char_indices()
                .map(|(start, ch)| {
                    (
                        ch,
                        (!ch.is_whitespace()).then_some(start..start + ch.len_utf8()),
                    )
                })
                .collect();
            let joined = join_sokuon(mapped.clone());
            assert_eq!(
                joined.iter().map(|(ch, _)| *ch).collect::<String>(),
                expected
            );
            assert_eq!(
                joined
                    .iter()
                    .filter(|(_, source)| source.is_some())
                    .cloned()
                    .collect::<Vec<_>>(),
                mapped
                    .into_iter()
                    .filter(|(_, source)| source.is_some())
                    .collect::<Vec<_>>()
            );
        }
    }
    #[test]
    fn numbers_and_punctuation_are_readable_japanese() {
        assert_eq!(normalize("１２月に2026冊。"), "十二月に二千二十六冊.");
        assert_eq!(kanji_number(10001), "一万一");
    }
    #[test]
    fn japanese_quotes_and_ascii_spelling_preserve_original_utf8_locations() {
        let source = "「ＣＰＵ」（USB）※…";
        let (text, map) = normalize_mapped(source);
        assert_eq!(text, "“CPU”(USB) …");
        for (at, ch) in text.char_indices() {
            let range = &map[at];
            assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
            assert!(!source[range.clone()].is_empty());
            assert!(map[at..at + ch.len_utf8()].iter().all(|part| part == range));
        }
        assert_eq!(ascii_spelling("CPU"), "しーぴーゆー");
        assert_eq!(ascii_spelling("API"), "えーぴーあい");
    }
    #[test]
    fn old_japanese_quotes_iteration_and_width_forms_keep_source_ranges() {
        let source = "〝本〟‐人〻‐ｶﾞﾗｽとﾊﾟﾝ‐いすゞ‐ＡＰＩ＃\0";
        let (normalized, map) = normalize_mapped(source);
        assert_eq!(normalized, "“本”—人々—ガラスとパン—いすず—API# ");
        for (start, ch) in normalized.char_indices() {
            let range = &map[start];
            assert!(source.is_char_boundary(range.start) && source.is_char_boundary(range.end));
            assert!(
                map[start..start + ch.len_utf8()]
                    .iter()
                    .all(|entry| entry == range)
            );
        }
        let ga = normalized.find('ガ').unwrap();
        assert_eq!(&source[map[ga].clone()], "ｶﾞ");
        let pa = normalized.find('パ').unwrap();
        assert_eq!(&source[map[pa].clone()], "ﾊﾟ");
        let iteration = normalized.find('々').unwrap();
        assert_eq!(&source[map[iteration].clone()], "〻");
        assert_eq!(normalize("カ\u{3099}とハ\u{309a}、くゝ。"), "ガとパ,くく.");
    }
    #[test]
    fn frontend_upgrade_preserves_all_embedded_license_notices() {
        fn string(out: &mut Vec<u8>, text: &str) {
            out.extend_from_slice(&(text.len() as u64).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
        }
        let names = [
            "unidic/sys.dic",
            "g2p/ja.json",
            "licenses/misaki-LICENSE",
            "licenses/unidic-dictionary/BSD",
            "g2p/versions.json",
            "ignored.bin",
        ];
        let root = tempfile::tempdir().unwrap();
        let mut package = b"GGUF".to_vec();
        package.extend_from_slice(&3u32.to_le_bytes());
        package.extend_from_slice(&0u64.to_le_bytes());
        package.extend_from_slice(&3u64.to_le_bytes());
        string(&mut package, "audiocpp.embedded_files.names");
        package.extend_from_slice(&9u32.to_le_bytes());
        package.extend_from_slice(&8u32.to_le_bytes());
        package.extend_from_slice(&(names.len() as u64).to_le_bytes());
        for name in names {
            string(&mut package, name);
        }
        string(&mut package, "audiocpp.embedded_files.offsets");
        package.extend_from_slice(&9u32.to_le_bytes());
        package.extend_from_slice(&10u32.to_le_bytes());
        package.extend_from_slice(&7u64.to_le_bytes());
        for offset in 0..=6u64 {
            package.extend_from_slice(&offset.to_le_bytes());
        }
        string(&mut package, "audiocpp.embedded_files.data");
        package.extend_from_slice(&9u32.to_le_bytes());
        package.extend_from_slice(&0u32.to_le_bytes());
        package.extend_from_slice(&6u64.to_le_bytes());
        package.extend_from_slice(b"123456");
        let source = root.path().join("frontend.gguf");
        std::fs::write(&source, package).unwrap();
        let expanded = root.path().join("expanded");
        std::fs::create_dir_all(&expanded).unwrap();
        std::fs::write(expanded.join("frontend-v2.ready"), b"2").unwrap();
        extract(&source, &expanded).unwrap();
        assert_eq!(
            std::fs::read(expanded.join("licenses/misaki-LICENSE")).unwrap(),
            b"3"
        );
        assert_eq!(
            std::fs::read(expanded.join("licenses/unidic-dictionary/BSD")).unwrap(),
            b"4"
        );
        assert!(!expanded.join("ignored.bin").exists());
        assert_eq!(
            std::fs::read(expanded.join("g2p/versions.json")).unwrap(),
            b"5"
        );
        assert!(expanded.join("frontend-v3.ready").is_file());
    }
    #[test]
    #[ignore = "manual probe: requires the installed pinned Kokoro Japanese frontend"]
    fn actual_mecab_frontend_reads_quotes_parentheses_and_technical_acronyms() {
        let assets = crate::data_dir().join("speech/assets");
        let library = crate::native::kokoro_mecab_library(
            &assets.join("audio.cpp-v0.9.3-portable"),
            false,
            &crate::Cancellation::uncancelled(),
            &|_| {},
        )
        .unwrap();
        let frontend =
            Japanese::new(&assets.join("kokoro-japanese-frontend-v1"), &library).unwrap();
        let inverse: HashMap<_, _> = frontend.vocab.iter().map(|(ch, id)| (*id, *ch)).collect();
        // Goldens from official Misaki 0.9.4 Cutlet with this same UniDic.
        // These have no punctuation, so intentional punctuation preservation
        // does not obscure the pronunciation/word-boundary comparison.
        for (text, expected) in [
            ("あっと驚く", "aʔto odoɾokɯ"),
            ("なっちゃった", "naʔʨaʔta"),
            ("やったって言う", "jaʔtaʔte jɯː"),
            ("日本語の本を読みます", "ɲiʔpoŋɡo no hoɴ o jomʲimasɨ"),
            ("サッカーとテニス", "saʔkaː to teɲisɨ"),
            (
                "「本を読む」と彼は言った。",
                "“hoɴ o jomɯ” to kaɾe βa iʔta.",
            ),
            (
                "これは（読み上げ）の試験です。",
                "koɾe βa (jomʲiaɡe) no ɕikeɴ desɨ.",
            ),
        ] {
            let actual: String = frontend
                .tokens(text)
                .unwrap()
                .iter()
                .map(|phone| inverse[&phone.id])
                .collect();
            assert_eq!(actual, expected, "{text}");
        }
        if let Ok(path) = std::env::var("YOMI_KOKORO_G2P_AUDIT") {
            let cases: Vec<serde_json::Value> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let inverse: HashMap<_, _> = frontend.vocab.iter().map(|(ch, id)| (*id, *ch)).collect();
            for case in cases {
                let text = case["text"].as_str().unwrap();
                let actual: String = frontend
                    .tokens(text)
                    .unwrap()
                    .iter()
                    .map(|phone| inverse[&phone.id])
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({"text":text,"actual":actual,"expected":case["expected"]})
                );
            }
        }
        for text in [
            "「本を読む」と彼は言った。",
            "漢字《かんじ》、ルビ（読み）です。",
            "CPUとUSB、APIを使います。",
            "ZZZを読む。",
            "〝本を読む〟‐人〻が歩きます。",
            "ｶﾞﾗｽとﾊﾟﾝ、こゝといすゞを読む。",
            "未知の記号☃・♜・〳〵も本文を止めません。",
        ] {
            let phones = frontend.tokens(text).unwrap();
            assert!(phones.len() > 5);
            for phone in &phones {
                if let Some(source) = &phone.source {
                    assert!(
                        source.start < source.end
                            && text.is_char_boundary(source.start)
                            && text.is_char_boundary(source.end)
                    );
                }
            }
            if text.starts_with("CPU") {
                for acronym in ["CPU", "USB", "API"] {
                    let start = text.find(acronym).unwrap();
                    assert!(
                        phones
                            .iter()
                            .filter(|phone| phone
                                .source
                                .as_ref()
                                .is_some_and(|range| range.start <= start
                                    && range.end >= start + acronym.len()))
                            .count()
                            >= 4,
                        "{acronym} was dropped instead of pronounced"
                    );
                }
            }
        }
        // Unsupported symbols contribute no lexical source mark. A pause-only
        // model input lets following speech chunks continue without claiming
        // an invented pronunciation or per-character timestamp.
        let pause = frontend.tokens("☃♜").unwrap();
        assert!(!pause.is_empty() && pause.iter().all(|phone| phone.source.is_none()));
        for code in (0x2000..=0x206f)
            .chain(0x3000..=0x303f)
            .chain(0xff00..=0xff9f)
        {
            let Some(ch) = char::from_u32(code) else {
                continue;
            };
            let text = format!("前{ch}後。");
            let tokens = frontend.tokens(&text).unwrap();
            assert!(
                tokens.iter().filter(|phone| phone.source.is_some()).count() > 1,
                "surrounding speech was lost at U+{code:04X}"
            );
        }
    }
}
