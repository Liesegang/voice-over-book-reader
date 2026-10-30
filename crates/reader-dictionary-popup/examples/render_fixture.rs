//! Emit original popup HTML for syntax/accessibility QA without opening a window.
use reader_dictionary_popup::{DictionaryTab, render_popup_tabs};
use reader_library::DictionaryHit;

fn main() {
    let tabs = vec![
        DictionaryTab {
            id: "a".repeat(64),
            title: "日本語辞典".into(),
            count: Some(1),
        },
        DictionaryTab {
            id: "b".repeat(64),
            title: "用例辞典".into(),
            count: None,
        },
        DictionaryTab {
            id: "c".repeat(64),
            title: "英語だけの辞典".into(),
            count: Some(0),
        },
    ];
    let hits = vec![DictionaryHit {
        dictionary_id: tabs[0].id.clone(),
        dictionary_title: tabs[0].title.clone(),
        headword: "猫".into(),
        html: "<p>ねこ</p>".into(),
        ordinal: 1,
        redirected_from: None,
    }];
    println!(
        "{}",
        render_popup_tabs(
            "猫",
            &tabs,
            Some(&tabs[0].id),
            &hits,
            &[],
            "sepia",
            "2563eb",
            false,
            false
        )
    );
}
