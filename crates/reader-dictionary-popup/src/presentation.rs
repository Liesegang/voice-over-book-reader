use crate::{dictionary_origin, escape_html};
use reader_library::DictionaryHit;
use serde::Serialize;

/// A dictionary section in configured order. The historical name is kept for callers.
#[derive(Clone, Debug)]
pub struct DictionaryTab {
    pub id: String,
    pub title: String,
    /// None means queued or searching; Some(0) remains visible in the index.
    pub count: Option<usize>,
}

#[derive(Debug, Serialize)]
struct Entry<'a> {
    ordinal: u64,
    headword: &'a str,
    src: String,
}
#[derive(Debug, Serialize)]
struct Section<'a> {
    id: &'a str,
    title: &'a str,
    count: Option<usize>,
    entries: Vec<Entry<'a>>,
}
#[derive(Debug, Serialize)]
struct Snapshot<'a> {
    query: &'a str,
    selected: Option<&'a str>,
    sections: Vec<Section<'a>>,
    diagnostics: &'a [String],
    theme: &'a str,
    accent: &'a str,
    pinned: bool,
    paper: Option<&'a str>,
    ink: Option<&'a str>,
}

pub fn tabs_from_hits(hits: &[DictionaryHit]) -> Vec<DictionaryTab> {
    let mut tabs: Vec<DictionaryTab> = Vec::new();
    for hit in hits {
        if let Some(tab) = tabs.iter_mut().find(|tab| tab.id == hit.dictionary_id) {
            tab.count = Some(tab.count.unwrap_or(0) + 1);
        } else {
            tabs.push(DictionaryTab {
                id: hit.dictionary_id.clone(),
                title: hit.dictionary_title.clone(),
                count: Some(1),
            });
        }
    }
    tabs
}

#[allow(clippy::too_many_arguments)]
fn snapshot<'a>(
    query: &'a str,
    tabs: &'a [DictionaryTab],
    selected: Option<&'a str>,
    hits: impl IntoIterator<Item = &'a DictionaryHit>,
    diagnostics: &'a [String],
    theme: &'a str,
    accent: &'a str,
    pinned: bool,
) -> Snapshot<'a> {
    let mut sections: Vec<_> = tabs
        .iter()
        .map(|tab| Section {
            id: &tab.id,
            title: &tab.title,
            count: tab.count,
            entries: vec![],
        })
        .collect();
    for hit in hits {
        if let Some(section) = sections
            .iter_mut()
            .find(|section| section.id == hit.dictionary_id)
            && !section
                .entries
                .iter()
                .any(|entry| entry.ordinal == hit.ordinal)
        {
            section.entries.push(Entry {
                ordinal: hit.ordinal,
                headword: &hit.headword,
                src: format!(
                    "{}__entry/{}",
                    dictionary_origin(&hit.dictionary_id),
                    hit.ordinal
                ),
            });
        }
    }
    Snapshot {
        query,
        selected,
        sections,
        diagnostics,
        theme,
        accent: valid_accent(accent),
        pinned,
        paper: None,
        ink: None,
    }
}
fn valid_accent(accent: &str) -> &str {
    if accent.len() == 6 && accent.bytes().all(|b| b.is_ascii_hexdigit()) {
        accent
    } else {
        "2563eb"
    }
}
fn safe_json(value: &impl Serialize) -> String {
    // JSON appears inside a script; prevent an author's title/query closing it.
    serde_json::to_string(value)
        .expect("popup snapshot serialization")
        .replace('<', "\\u003c")
}

/// Patch only changed sections. Definition HTML never crosses the IPC update.
#[allow(clippy::too_many_arguments)]
pub fn popup_update_script<'a>(
    query: &'a str,
    tabs: &'a [DictionaryTab],
    selected: Option<&'a str>,
    hits: impl IntoIterator<Item = &'a DictionaryHit>,
    diagnostics: &'a [String],
    theme: &'a str,
    accent: &'a str,
    pinned: bool,
    paper: Option<&'a str>,
    ink: Option<&'a str>,
) -> String {
    let mut state = snapshot(
        query,
        tabs,
        selected,
        hits,
        diagnostics,
        theme,
        accent,
        pinned,
    );
    state.paper = paper.filter(|value| valid_accent(value) == *value);
    state.ink = ink.filter(|value| valid_accent(value) == *value);
    format!("window.dictionaryView?.apply({})", safe_json(&state))
}

/// Construct the persistent shell. All enabled dictionaries are sections, including
/// those still queued and those with no matches; `selected` only chooses a jump target.
#[allow(clippy::too_many_arguments)]
pub fn render_popup_tabs(
    query: &str,
    tabs: &[DictionaryTab],
    selected: Option<&str>,
    hits: &[DictionaryHit],
    diagnostics: &[String],
    theme: &str,
    accent: &str,
    pinned: bool,
    _loading: bool,
) -> String {
    let initial = safe_json(&snapshot(
        query,
        tabs,
        selected,
        hits,
        diagnostics,
        theme,
        accent,
        pinned,
    ));
    let handles = ["n", "ne", "e", "se", "s", "sw", "w", "nw"]
        .into_iter()
        .map(|d| format!("<div class='resize {d}' data-resize='{d}'></div>"))
        .collect::<String>();
    format!(
        r#"<!doctype html><html lang='ja'><head><meta charset='utf-8'><meta name='color-scheme' content='light dark'><style>{}</style></head><body><main id='surface'><header id='topbar'><span class='icon' id='grip' title='ドラッグして移動' aria-label='ドラッグして移動'><svg viewBox='0 0 24 24'><path d='M7 5h10M7 9h10M7 13h10M7 17h10'/></svg></span><input id='query' value='{}' aria-label='辞書を検索' title='語を入力してEnterで検索'><button class='icon' id='pin' aria-label='辞書を固定' title='固定して本文を読んでも開いたままにする' aria-pressed='{pinned}'><svg viewBox='0 0 24 24'><path d='m8 3 8 0-1 6 4 4H5l4-4zM12 13v8'/></svg></button><button class='icon' id='close' title='閉じる (Esc)' aria-label='辞書を閉じる'><svg viewBox='0 0 24 24'><path d='m6 6 12 12M18 6 6 18'/></svg></button></header><div id='content'><div id='results' tabindex='0' aria-label='すべての辞書の検索結果'></div><aside id='dictionary-index'><div id='index-label'>辞書</div><nav id='index-items' aria-label='辞書一覧'></nav></aside></div><div id='search-status' class='sr-only' role='status' aria-live='polite'></div></main>{handles}<script>{}
window.dictionaryView.apply({initial});window.ipc?.postMessage(JSON.stringify({{action:'ready'}}));</script></body></html>"#,
        include_str!("popup.css"),
        escape_html(query),
        include_str!("popup.js")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tabs() -> Vec<DictionaryTab> {
        [
            ("a", "国語辞典", Some(1)),
            ("b", "英和 <辞書>", Some(0)),
            ("c", "未検索", None),
        ]
        .into_iter()
        .map(|(id, title, count)| DictionaryTab {
            id: id.repeat(64),
            title: title.into(),
            count,
        })
        .collect()
    }
    fn hit(id: &str, ordinal: u64) -> DictionaryHit {
        DictionaryHit {
            dictionary_id: id.repeat(64),
            dictionary_title: "test".into(),
            headword: "猫".into(),
            html: "<script>privateDefinition()</script>".into(),
            ordinal,
            redirected_from: None,
        }
    }
    #[test]
    fn all_dictionaries_and_zero_pending_sections_are_present_without_raw_definitions() {
        let tabs = tabs();
        let hits = [hit("a", 1), hit("b", 2)];
        let state = snapshot(
            "猫",
            &tabs,
            Some(&tabs[1].id),
            &hits,
            &[],
            "light",
            "2563eb",
            false,
        );
        assert_eq!(state.sections.len(), 3);
        assert_eq!(state.sections[0].entries[0].ordinal, 1);
        assert_eq!(state.sections[1].entries[0].ordinal, 2);
        assert_eq!(state.sections[1].count, Some(0));
        assert_eq!(state.sections[2].count, None);
        let update = popup_update_script(
            "猫",
            &tabs,
            None,
            &hits,
            &[],
            "light",
            "2563eb",
            false,
            None,
            None,
        );
        assert!(!update.contains("privateDefinition"));
        assert!(!update.contains("load_html"));
    }
    #[test]
    fn script_json_cannot_be_closed_by_dictionary_title_or_query() {
        let html = render_popup_tabs(
            "</script><猫>",
            &tabs(),
            None,
            &[],
            &[],
            "sepia",
            "red;}body{}",
            true,
            true,
        );
        assert!(!html.contains("</script><猫>"));
        assert!(html.contains("\\u003c/script>"));
        assert!(html.contains("&lt;/script&gt;&lt;猫&gt;"));
        assert!(!html.contains("red;}body"));
        assert_eq!(html.matches("data-resize='").count(), 8);
        assert!(html.contains("id='dictionary-index'"));
        assert!(!html.contains("role='tab'"));
    }
    #[test]
    fn custom_paper_and_ink_are_validated_before_the_script_boundary() {
        let tabs = tabs();
        let valid = popup_update_script(
            "猫",
            &tabs,
            None,
            [],
            &[],
            "light",
            "2563eb",
            false,
            Some("002B36"),
            Some("839496"),
        );
        assert!(valid.contains("\"paper\":\"002B36\""));
        assert!(valid.contains("\"ink\":\"839496\""));
        let invalid = popup_update_script(
            "猫",
            &tabs,
            None,
            [],
            &[],
            "light",
            "2563eb",
            false,
            Some("red;}"),
            Some("</script>"),
        );
        assert!(invalid.contains("\"paper\":null"));
        assert!(!invalid.contains("red;}"));
    }
    #[test]
    fn duplicate_redirects_do_not_decode_the_same_definition_twice() {
        let tabs = tabs();
        let hits = [hit("a", 1), hit("a", 1)];
        let state = snapshot("猫", &tabs, None, &hits, &[], "dark", "ff8800", false);
        assert_eq!(state.sections[0].entries.len(), 1);
    }
}
