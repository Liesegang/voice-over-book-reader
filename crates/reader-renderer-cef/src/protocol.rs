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
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Configure {
        revision: Revision,
        chapter: u32,
        nodes: Vec<NodeBinding>,
        options: LayoutOptions,
    },
    Page {
        revision: Revision,
        index: u32,
    },
    Locate {
        revision: Revision,
        location: SourceLocator,
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
    ClearHighlight {
        revision: Revision,
    },
    Selection {
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
    Error {
        revision: Revision,
        message: String,
    },
}
