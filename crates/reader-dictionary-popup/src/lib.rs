//! Dictionary-only popup protocol and presentation. Book content never crosses
//! into this webview process.
use reader_library::DictionaryHit;
use serde::{Deserialize, Serialize};
pub mod geometry;
pub mod presentation;
pub use presentation::{DictionaryTab, render_popup_tabs, tabs_from_hits};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PopupCommand {
    Lookup {
        query: String,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        #[serde(default)]
        theme: Option<String>,
        #[serde(default)]
        accent: Option<String>,
        #[serde(default)]
        paper: Option<String>,
        #[serde(default)]
        ink: Option<String>,
    },
    Hide,
    /// Explicit user dismissal, including Esc, also closes a pinned popup.
    ForceHide,
    SetPinned {
        pinned: bool,
    },
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceRoute {
    pub dictionary_id: String,
    pub resource: String,
}

/// Parse only the application's registered custom scheme. The catalog applies
/// a second resource path check, including percent-decoded traversal rejection.
pub fn parse_resource_route(uri: &str) -> anyhow::Result<ResourceRoute> {
    let parsed = url::Url::parse(uri)?;
    anyhow::ensure!(
        (parsed.scheme() == "dictionary" && parsed.host_str() == Some("localhost"))
            || (matches!(parsed.scheme(), "http" | "https")
                && parsed.host_str() == Some("dictionary.localhost")),
        "External origin is not a dictionary resource"
    );
    let mut parts = parsed.path().trim_start_matches('/').splitn(2, '/');
    let dictionary_id = parts.next().unwrap_or_default();
    anyhow::ensure!(
        dictionary_id.len() == 64 && dictionary_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid dictionary identity"
    );
    let resource = parts.next().unwrap_or_default();
    anyhow::ensure!(!resource.is_empty(), "Missing dictionary resource");
    Ok(ResourceRoute {
        dictionary_id: dictionary_id.into(),
        resource: resource.into(),
    })
}

pub fn dictionary_origin(id: &str) -> String {
    if cfg!(any(target_os = "windows", target_os = "android")) {
        format!("http://dictionary.localhost/{id}/")
    } else {
        format!("dictionary://localhost/{id}/")
    }
}

pub fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn render_popup(query: &str, hits: &[DictionaryHit], diagnostics: &[String]) -> String {
    render_popup_styled(query, hits, diagnostics, "light", "2563eb")
}

pub fn render_entry(hit: &DictionaryHit) -> String {
    let origin = dictionary_origin(&hit.dictionary_id);
    let id_json = serde_json::to_string(&hit.dictionary_id).unwrap();
    let head = format!(
        r#"<base href="{}"><meta charset="utf-8"><style>html,body{{margin:0;padding:0;overflow-wrap:anywhere;font-family:'Yu Gothic','Noto Sans CJK JP',sans-serif;font-size:15px;line-height:1.65}}img,video,svg{{max-width:100%;height:auto}}table{{max-width:100%}}a{{color:#2563eb}}html,body{{height:auto!important;min-height:0!important;overflow-y:hidden!important}}body{{display:flow-root}}</style>"#,
        escape_html(&origin)
    );
    let bridge = format!(
        "<script>window.dictionaryIdentity={id_json};{}</script>",
        include_str!("entry.js")
    );
    // Keep the dictionary's original style/script markup. A base element is
    // prepended so relative CSS, images and audio resolve through MDD only.
    format!(
        "<!doctype html><html><head>{head}</head><body>{}{bridge}</body></html>",
        hit.html
    )
}

/// Theme only the popup chrome. Dictionary-authored CSS and definition colors
/// remain inside their isolated frames.
pub fn render_popup_styled(
    query: &str,
    hits: &[DictionaryHit],
    diagnostics: &[String],
    theme: &str,
    accent: &str,
) -> String {
    render_popup_tabs(
        query,
        &tabs_from_hits(hits),
        None,
        hits,
        diagnostics,
        theme,
        accent,
        false,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit() -> DictionaryHit {
        DictionaryHit{dictionary_id:"a".repeat(64),dictionary_title:"辞書 <&>".into(),headword:"猫".into(),html:"<link href='style.css' rel='stylesheet'><b>猫</b><script>window.dictLoaded=true</script>".into(),ordinal:4,redirected_from:None}
    }
    #[test]
    fn wrapper_escapes_app_text_but_keeps_dictionary_css_js() {
        let hit = hit();
        let html = render_popup("<cat>", std::slice::from_ref(&hit), &[]);
        assert!(html.contains("&lt;cat&gt;"));
        assert!(!html.contains("<cat>"));
        assert!(html.contains("__entry/4"));
        let entry = render_entry(&hit);
        assert!(entry.contains("style.css"));
        assert!(entry.contains("window.dictLoaded=true"));
        assert!(entry.contains("<base href="));
        assert!(entry.contains("sound://"));
    }
    #[test]
    fn resource_protocol_rejects_foreign_origins() {
        let id = "a".repeat(64);
        assert_eq!(
            parse_resource_route(&format!("http://dictionary.localhost/{id}/a.png"))
                .unwrap()
                .resource,
            "a.png"
        );
        assert!(parse_resource_route(&format!("https://example.com/{id}/a.png")).is_err());
        assert!(parse_resource_route("file:///etc/passwd").is_err());
        assert!(parse_resource_route("dictionary://localhost/not-an-id/a.png").is_err());
    }

    #[test]
    fn appearance_controls_only_chrome_and_rejects_css_injection() {
        let html = render_popup_styled("猫", &[hit()], &[], "sepia", "ff8800");
        assert!(html.contains("f5efdf"));
        assert!(html.contains("ff8800"));
        assert!(html.contains("allow-scripts allow-same-origin"));
        let html = render_popup_styled("猫", &[], &[], "dark", "red;}body{display:none");
        assert!(!html.contains("red;}"));
        assert!(html.contains("2563eb"));
    }

    #[test]
    fn reuse_commands_accept_optional_appearance_without_requiring_it() {
        assert!(matches!(
            serde_json::from_str::<PopupCommand>(r#"{"type":"lookup","query":"猫","x":10,"y":20}"#)
                .unwrap(),
            PopupCommand::Lookup {
                theme: None,
                accent: None,
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<PopupCommand>(
                r#"{"type":"lookup","query":"猫","theme":"sepia","accent":"ff8800"}"#
            )
            .unwrap(),
            PopupCommand::Lookup {
                theme: Some(_),
                accent: Some(_),
                ..
            }
        ));
    }
}
