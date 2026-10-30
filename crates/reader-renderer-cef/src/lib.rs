//! Book-only Chromium renderer. The application UI and input policy stay native.
pub mod protocol;
pub use protocol::*;

pub const NAVIGATOR_SCRIPT: &str = include_str!("navigator.js");
pub const DRAIN_SCRIPT: &str = "globalThis.__vobrBook?.drain() ?? []";

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
#[cfg(windows)]
pub use {cef, welding, wgpu};

#[derive(Debug, thiserror::Error)]
pub enum RendererError {
    #[error("Invalid renderer protocol: {0}")]
    Protocol(String),
    #[error("Book renderer: {0}")]
    Platform(String),
}

/// Encode native commands as JSON values; text is never interpolated as code.
pub fn command_script(command: &Command) -> Result<String, RendererError> {
    let json =
        serde_json::to_string(command).map_err(|e| RendererError::Protocol(e.to_string()))?;
    Ok(format!("globalThis.__vobrBook.dispatch({json})"))
}

pub fn decode_events(json: &str) -> Result<Vec<NavigatorEvent>, RendererError> {
    if json.len() > 8 * 1024 * 1024 {
        return Err(RendererError::Protocol("event batch exceeded 8 MiB".into()));
    }
    serde_json::from_str(json).map_err(|e| RendererError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_quotes_are_data_and_offsets_remain_utf8() {
        let command = Command::Highlight {
            revision: Revision(9),
            ranges: vec![SourceRange {
                chapter: 2,
                node: 17,
                start: 3,
                end: 7,
            }],
        };
        let script = command_script(&command).unwrap();
        assert!(script.contains("\"start\":3,\"end\":7"));
        assert!(script.starts_with("globalThis.__vobrBook.dispatch({"));
    }
    #[test]
    fn pagination_is_a_revisioned_exact_index_not_a_fraction() {
        let json = r#"[{"type":"pagination","revision":12,"chapter":1,"pages":3,"page_starts":[{"chapter":1,"node":2,"offset":0},{"chapter":1,"node":3,"offset":9},{"chapter":1,"node":5,"offset":0}],"ready":{"fonts":true,"images":true},"writing_mode":"vertical-rl"}]"#;
        let events = decode_events(json).unwrap();
        assert!(
            matches!(&events[0],NavigatorEvent::Pagination{revision:Revision(12),pages:3,page_starts,..} if page_starts[1].is_some_and(|p|p.offset==9))
        );
    }
    #[test]
    fn malformed_or_oversize_bridge_payloads_are_errors() {
        assert!(decode_events("{}").is_err());
        assert!(decode_events(&" ".repeat(8 * 1024 * 1024 + 1)).is_err());
    }
}
