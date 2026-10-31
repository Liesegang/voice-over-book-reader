use serde::{Deserialize, Serialize};

/// Settings/viewport/resources generation. Discard events from older revisions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(pub u64);

/// Mirrors reader_core::Locator. Offset is UTF-8 bytes in the decoded node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocator {
    pub chapter: u32,
    pub node: u32,
    pub offset: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub chapter: u32,
    pub node: u32,
    pub start: u32,
    pub end: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeBinding {
    pub node: u32,
    /// childNodes indices beginning at document.documentElement. No wrappers
    /// are inserted into author content, so CSS child/sibling selectors survive.
    pub path: Vec<u32>,
    /// Decoded source text, for validating the XML/DOM mapping before use.
    /// None binds an element (e.g. an inline image), not invented text.
    pub text: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    Reflowable,
    Fixed,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Writing {
    #[default]
    Author,
    Horizontal,
    VerticalRl,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayoutOptions {
    pub flow: Flow,
    pub writing: Writing,
    /// top/right/bottom/left, CSS pixels. Native chrome must never change these.
    pub margins: [f64; 4],
    pub font_family: Option<String>,
    pub font_size: Option<f64>,
    pub line_height: Option<f64>,
    pub letter_spacing: Option<f64>,
    pub ruby_scale: Option<f64>,
    pub show_ruby: bool,
    pub fixed_viewport: Option<[f64; 2]>,
    /// -1 aligns fixed artwork left, 0 centers, +1 aligns right.
    /// A spread uses inside edges; a single cover remains centered.
    #[serde(default)]
    pub fixed_horizontal_align: i32,
    /// Native reader palette. Fixed author artwork is not recolored.
    #[serde(default)]
    pub background: Option<[u8; 3]>,
    #[serde(default)]
    pub foreground: Option<[u8; 3]>,
    #[serde(default)]
    pub highlight_color: Option<[u8; 3]>,
    /// Optional two-column text layout within one native viewport.
    /// Image spreads are composed from separate fixed-page surfaces by the host.
    #[serde(default = "single_column")]
    pub columns: u32,
    #[serde(default)]
    pub page_gap: f64,
    /// Reader defaults have a lower cascade layer than author stylesheets.
    #[serde(default)]
    pub default_css: Option<String>,
    /// Diagnostic GPU marker. Off for ordinary reading and never persisted.
    #[serde(default)]
    pub measurement_marker: bool,
    #[serde(default)]
    pub adapt_monochrome_images: bool,
}
impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            flow: Flow::Reflowable,
            writing: Writing::Author,
            margins: [48.; 4],
            font_family: None,
            font_size: None,
            line_height: None,
            letter_spacing: None,
            ruby_scale: None,
            show_ruby: true,
            fixed_viewport: None,
            fixed_horizontal_align: 0,
            background: None,
            foreground: None,
            highlight_color: None,
            columns: 1,
            page_gap: 0.,
            default_css: None,
            measurement_marker: false,
            adapt_monochrome_images: false,
        }
    }
}
fn single_column() -> u32 {
    1
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Configure {
        revision: Revision,
        chapter: u32,
        nodes: Vec<NodeBinding>,
        options: Box<LayoutOptions>,
    },
    Page {
        revision: Revision,
        index: u32,
    },
    Locate {
        revision: Revision,
        location: SourceLocator,
    },
    Fragment {
        revision: Revision,
        fragment: String,
    },
    HitTest {
        revision: Revision,
        x: f64,
        y: f64,
    },
    Rects {
        revision: Revision,
        ranges: Vec<SourceRange>,
    },
    Highlight {
        revision: Revision,
        ranges: Vec<SourceRange>,
    },
    Annotations {
        revision: Revision,
        ranges: Vec<SourceRange>,
    },
    ClearHighlight {
        revision: Revision,
    },
    Selection {
        revision: Revision,
    },
    ClearSelection {
        revision: Revision,
    },
    Refresh {
        revision: Revision,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Ready {
    pub fonts: bool,
    pub images: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CssRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NavigatorEvent {
    Invalidated {
        revision: Revision,
        reason: String,
    },
    Pagination {
        revision: Revision,
        chapter: u32,
        pages: u32,
        page_starts: Vec<Option<SourceLocator>>,
        ready: Ready,
        writing_mode: String,
    },
    Position {
        revision: Revision,
        chapter: u32,
        page: u32,
        pages: u32,
        start: Option<SourceLocator>,
        #[serde(default)]
        marker: Option<[u8; 3]>,
    },
    Hit {
        revision: Revision,
        location: Option<SourceLocator>,
    },
    Rects {
        revision: Revision,
        rects: Vec<CssRect>,
    },
    Selection {
        revision: Revision,
        start: Option<SourceLocator>,
        end: Option<SourceLocator>,
        text: String,
    },
    Pointer {
        revision: Revision,
        kind: String,
        x: f64,
        y: f64,
        location: Option<SourceLocator>,
        text: String,
        selection_start: Option<SourceLocator>,
        selection_end: Option<SourceLocator>,
    },
    Link {
        revision: Revision,
        href: String,
        x: f64,
        y: f64,
        location: Option<SourceLocator>,
        user_gesture: bool,
    },
    Error {
        revision: Revision,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_layout_json_keeps_one_column_and_author_colors() {
        let value = serde_json::json!({
            "flow":"reflowable", "writing":"author", "margins":[48,60,48,60],
            "font_family":null, "font_size":null, "line_height":null,
            "letter_spacing":null, "ruby_scale":null, "show_ruby":true,
            "fixed_viewport":null
        });
        let options: LayoutOptions = serde_json::from_value(value).unwrap();
        assert_eq!(options.columns, 1);
        assert_eq!(options.fixed_horizontal_align, 0);
        assert_eq!(options.page_gap, 0.);
        assert!(options.background.is_none() && options.foreground.is_none());
        assert!(options.default_css.is_none());
    }

    #[test]
    fn context_and_internal_link_events_preserve_source_offsets() {
        let events: Vec<NavigatorEvent> = serde_json::from_value(serde_json::json!([
            {"type":"pointer","revision":12,"kind":"context_menu","x":84.5,"y":102.0,
             "location":{"chapter":3,"node":10,"offset":3},"text":"😀",
             "selection_start":{"chapter":3,"node":10,"offset":3},
             "selection_end":{"chapter":3,"node":10,"offset":7}},
            {"type":"link","revision":12,"href":"vobr://book/chapter.xhtml#note",
             "x":84.5,"y":102.0,"location":null,"user_gesture":true}
        ]))
        .unwrap();
        assert!(matches!(
            &events[0],
            NavigatorEvent::Pointer {
                location: Some(SourceLocator { offset: 3, .. }),
                selection_end: Some(SourceLocator { offset: 7, .. }),
                ..
            }
        ));
        assert!(
            matches!(&events[1], NavigatorEvent::Link {href,user_gesture:true,..} if href.ends_with("#note"))
        );
        let fragment = Command::Fragment {
            revision: Revision(12),
            fragment: "注記\"\\".into(),
        };
        let json = serde_json::to_value(fragment).unwrap();
        assert_eq!(json["type"], "fragment");
        assert_eq!(json["fragment"], "注記\"\\");
    }
}
