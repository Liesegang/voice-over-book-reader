use eframe::egui::{self, Key, Modifiers};
use std::collections::BTreeMap;

macro_rules! actions {
    ($( $id:ident, $label:literal, $group:literal, $default:literal; )*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Action { $( $id, )* }
        impl Action {
            pub const ALL: &'static [Self] = &[$( Self::$id, )*];
            pub fn id(self) -> &'static str { match self { $( Self::$id => stringify!($id), )* } }
            pub fn label(self) -> &'static str { match self { $( Self::$id => $label, )* } }
            pub fn group(self) -> &'static str { match self { $( Self::$id => $group, )* } }
            pub fn default_binding(self) -> &'static str { match self { $( Self::$id => $default, )* } }
        }
    }
}

actions! {
    Open, "本を開く（新しいウィンドウ）", "ウィンドウ", "Primary+O";
    Library, "本棚ウィンドウを開く", "ウィンドウ", "Primary+L";
    CloseWindow, "このウィンドウを閉じる", "ウィンドウ", "Primary+W";
    MinimizeWindow, "ウィンドウを最小化", "ウィンドウ", "";
    MaximizeWindow, "最大化・元のサイズに戻す", "ウィンドウ", "";
    ToggleChrome, "読書メニューの表示・非表示", "読書", "";
    CloseTransient, "メニュー・選択・ポップアップを閉じる", "読書", "Escape";
    NextPage, "次のページ", "読書", "PageDown";
    PreviousPage, "前のページ", "読書", "PageUp";
    PageLeft, "左側へページをめくる", "読書", "ArrowLeft";
    PageRight, "右側へページをめくる", "読書", "ArrowRight";
    FirstPage, "本の先頭", "読書", "Primary+Home";
    LastPage, "本の末尾", "読書", "Primary+End";
    NextChapter, "次の章", "読書", "";
    PreviousChapter, "前の章", "読書", "";
    Toc, "目次", "読書", "Primary+T";
    Bookmarks, "ブックマーク一覧", "読書", "";
    Notes, "注釈一覧", "読書", "";
    Search, "本文を検索", "読書", "Primary+F";
    AddBookmark, "ブックマークを追加", "読書", "B";
    AddNote, "選択箇所に注釈を書く", "選択", "";
    Copy, "選択文字をコピー", "選択", "Primary+C";
    Dictionary, "選択した語を辞書で引く", "選択", "D";
    PinDictionary, "辞書の固定を切り替える", "選択", "";
    TranslateSelection, "選択文字を翻訳", "選択", "";
    TranslateChapter, "この章を翻訳", "選択", "";
    PlayPause, "読み上げを再生・一時停止", "読み上げ", "Space";
    Stop, "読み上げを停止", "読み上げ", "";
    ReadSelection, "選択した位置から読み上げ", "読み上げ", "";
    PickReadPosition, "読み上げ開始位置を選ぶ", "読み上げ", "";
    FollowSpeech, "読み上げ位置への追従を切り替える", "読み上げ", "";
    Faster, "読み上げを速くする", "読み上げ", "";
    Slower, "読み上げを遅くする", "読み上げ", "";
    PreviewVoice, "設定した声を試聴", "読み上げ", "";
    Appearance, "外観設定", "設定", "Primary+Comma";
    ReadingSettings, "読書設定", "設定", "";
    SpeechSettings, "音声設定", "設定", "";
    DictionarySettings, "辞書の管理", "設定", "";
    LibrarySettings, "本棚・Calibre設定", "設定", "";
    KeySettings, "キー割り当て", "設定", "";
    Light, "ライトテーマ", "表示", "";
    Dark, "ダークテーマ", "表示", "";
    Sepia, "セピアテーマ", "表示", "";
    Vertical, "縦書きにする", "表示", "";
    Horizontal, "横書きにする", "表示", "";
    AutoWriting, "本の文字方向を使う", "表示", "";
    ToggleRuby, "ルビ表示を切り替える", "表示", "";
    FontLarger, "文字を大きくする", "表示", "";
    FontSmaller, "文字を小さくする", "表示", "";
    AutoSpread, "画面に合わせた見開き", "表示", "";
    SinglePage, "単ページ表示", "表示", "";
    DoublePage, "見開き表示", "表示", "";
    ToggleTextSpread, "本文の中央の余白を切り替える", "表示", "";
    RefreshLibrary, "本棚を更新", "本棚", "Primary+R";
    AllBooks, "すべての本", "本棚", "";
    ReadingBooks, "読書中の本", "本棚", "";
    FinishedBooks, "読了した本", "本棚", "";
    LibraryCards, "表紙表示", "本棚", "";
    LibraryList, "行リスト表示", "本棚", "";
    CreateShelf, "自分の本棚を作成", "本棚", "";
    Opds, "オンライン本棚", "本棚", "";
    ImportWeb, "Web記事を取り込む", "本棚", "";
    AddDictionary, "MDX辞書を追加", "辞書管理", "";
    AddDictionaryFolder, "辞書フォルダーを追加", "辞書管理", "";
    RefreshDictionaries, "辞書一覧を更新", "辞書管理", "";
}

pub fn binding(action: Action, saved: &BTreeMap<String, String>) -> &str {
    saved
        .get(action.id())
        .map(String::as_str)
        .unwrap_or(action.default_binding())
}

pub fn chord(key: Key, mods: Modifiers) -> String {
    let mut parts = Vec::new();
    if mods.command {
        parts.push("Primary".to_owned());
    } else if mods.ctrl {
        parts.push("Ctrl".to_owned());
    }
    if mods.alt {
        parts.push("Alt".to_owned());
    }
    if mods.shift {
        parts.push("Shift".to_owned());
    }
    parts.push(format!("{key:?}"));
    parts.join("+")
}

pub fn matches(binding: &str, key: Key, mods: Modifiers) -> bool {
    !binding.is_empty() && binding == chord(key, mods)
}

pub fn pressed(ctx: &egui::Context, saved: &BTreeMap<String, String>) -> Vec<Action> {
    ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } = event
                {
                    Action::ALL
                        .iter()
                        .copied()
                        .find(|action| matches(binding(*action, saved), *key, *modifiers))
                } else {
                    None
                }
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_f11_and_bindings_are_independent_of_direction() {
        assert!(
            !Action::ALL
                .iter()
                .any(|action| action.default_binding().contains("F11"))
        );
        assert_eq!(Action::PageLeft.default_binding(), "ArrowLeft");
        assert_eq!(Action::NextPage.default_binding(), "PageDown");
        let mut saved = BTreeMap::new();
        saved.insert(Action::NextPage.id().into(), "Alt+N".into());
        assert!(matches(
            binding(Action::NextPage, &saved),
            Key::N,
            Modifiers::ALT
        ));
        saved.insert(Action::NextPage.id().into(), String::new());
        assert!(!matches(
            binding(Action::NextPage, &saved),
            Key::N,
            Modifiers::ALT
        ));
    }
}
