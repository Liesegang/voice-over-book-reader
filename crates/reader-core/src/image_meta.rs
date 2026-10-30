/// Dimensions from bounded image headers; never decode pixels just to lay out a
/// paragraph. JPEG scans are bounded by the caller's 64 KiB prefix limit.
pub(crate) fn dimensions(bytes: &[u8]) -> Option<[f32; 2]> {
    let be16 = |offset: usize| -> Option<u16> {
        Some(u16::from_be_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    let le16 = |offset: usize| -> Option<u16> {
        Some(u16::from_le_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    let size = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        [
            u32::from_be_bytes(bytes[16..20].try_into().ok()?) as f32,
            u32::from_be_bytes(bytes[20..24].try_into().ok()?) as f32,
        ]
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        [f32::from(le16(6)?), f32::from(le16(8)?)]
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        let mut index = 2;
        loop {
            if index >= bytes.len() {
                return None;
            }
            if bytes[index] != 0xff {
                index += 1;
                continue;
            }
            while bytes.get(index) == Some(&0xff) {
                index += 1;
            }
            let marker = *bytes.get(index)?;
            index += 1;
            if matches!(marker, 0xd8 | 0xd9 | 0xd0..=0xd7 | 0x01) {
                continue;
            }
            let length = be16(index)? as usize;
            if length < 2 {
                return None;
            }
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                break [f32::from(be16(index + 5)?), f32::from(be16(index + 3)?)];
            }
            index = index.checked_add(length)?;
        }
    } else if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        match bytes.get(12..16)? {
            b"VP8X" if bytes.len() >= 30 => {
                let u24 = |offset| {
                    u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], 0]) + 1
                };
                [u24(24) as f32, u24(27) as f32]
            }
            b"VP8L" if bytes.len() >= 25 && bytes[20] == 0x2f => {
                let bits = u32::from_le_bytes(bytes[21..25].try_into().ok()?);
                [
                    (bits & 0x3fff) as f32 + 1.0,
                    ((bits >> 14) & 0x3fff) as f32 + 1.0,
                ]
            }
            b"VP8 " if bytes.get(23..26) == Some(&[0x9d, 0x01, 0x2a]) => {
                [f32::from(le16(26)? & 0x3fff), f32::from(le16(28)? & 0x3fff)]
            }
            _ => return None,
        }
    } else {
        let text = std::str::from_utf8(bytes).ok()?;
        let start = text.find("<svg")?;
        let end = text[start..].find('>')? + start;
        let root = format!("{}/>", text[start..end].trim_end_matches('/'));
        let document = roxmltree::Document::parse(&root).ok()?;
        let svg = document.root_element();
        if let Some(view_box) = svg.attribute("viewBox") {
            let values: Vec<f32> = view_box
                .split(|character: char| character.is_whitespace() || character == ',')
                .filter(|part| !part.is_empty())
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            if values.len() != 4 || !values.iter().all(|value| value.is_finite()) {
                return None;
            }
            [values[2], values[3]]
        } else {
            let parse = |value: &str| value.trim().trim_end_matches("px").parse::<f32>().ok();
            [
                parse(svg.attribute("width")?)?,
                parse(svg.attribute("height")?)?,
            ]
        }
    };
    (size[0].is_finite() && size[1].is_finite() && size[0] > 0.0 && size[1] > 0.0).then_some(size)
}
