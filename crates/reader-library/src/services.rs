//! User-invoked, blocking network services. Run these methods on a worker.
//! No constructor starts a network request. Credentials are redacted in Debug.
use anyhow::{Context, Result};
use base64::Engine;
use reqwest::blocking::{Client, Response};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

const MAX_FEED: usize = 8 * 1024 * 1024;
const MAX_BOOK: usize = 256 * 1024 * 1024;
const MAX_IMAGE: usize = 16 * 1024 * 1024;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpdsConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for OpdsConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpdsConfig")
            .field("url", &redacted_url(&self.url))
            .field("username", &self.username.as_ref().map(|_| "[redacted]"))
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpdsLink {
    pub title: String,
    pub href: String,
    pub media_type: Option<String>,
    pub rel: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpdsPublication {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    /// Only a direct, free/downloadable EPUB acquisition. Buy/borrow/DRM links
    /// are not presented as immediately downloadable files.
    pub epub_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpdsPage {
    pub title: String,
    pub url: String,
    pub publications: Vec<OpdsPublication>,
    pub navigation: Vec<OpdsLink>,
    pub next: Option<String>,
    pub previous: Option<String>,
}

#[derive(Clone)]
pub struct OpdsClient {
    client: Client,
    config: OpdsConfig,
    origin: Url,
}

impl OpdsClient {
    pub fn new(config: OpdsConfig) -> Result<Self> {
        let origin = http_url(&config.url)?;
        Ok(Self {
            client: http_client()?,
            config,
            origin,
        })
    }

    pub fn fetch(&self, url: &str) -> Result<OpdsPage> {
        let response = self.get(&http_url(url)?)?;
        let final_url = response.url().clone();
        let bytes = response_bytes(response, MAX_FEED)?;
        parse_opds(&bytes, final_url.as_str())
    }

    pub fn download_epub(&self, url: &str, data_dir: &Path) -> Result<PathBuf> {
        let response = self.get(&http_url(url)?)?;
        let bytes = response_bytes(response, MAX_BOOK)?;
        anyhow::ensure!(
            bytes.starts_with(b"PK\x03\x04"),
            "The server did not return an EPUB/ZIP file; check catalog access"
        );
        let directory = data_dir.join("downloads");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.epub", blake3::hash(&bytes).to_hex()));
        if path.is_file() {
            return Ok(path);
        }
        let mut pending = tempfile::NamedTempFile::new_in(&directory)?;
        pending.write_all(&bytes)?;
        pending.flush()?;
        pending.as_file().sync_all()?;
        match pending.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
        Ok(path)
    }

    fn get(&self, url: &Url) -> Result<Response> {
        let mut request = self.client.get(url.clone()).header(
            "Accept",
            "application/opds+json, application/atom+xml, application/epub+zip;q=0.9, */*;q=0.5",
        );
        // A catalog may link to an unrelated CDN. Never preemptively attach its
        // credentials to a different origin, including a different local port.
        if url.origin() == self.origin.origin()
            && let Some(username) = self
                .config
                .username
                .as_deref()
                .filter(|value| !value.is_empty())
        {
            request = request.basic_auth(username, self.config.password.as_deref());
        }
        request.send().map_err(network_error)
    }
}

pub fn parse_opds(bytes: &[u8], base_url: &str) -> Result<OpdsPage> {
    anyhow::ensure!(bytes.len() <= MAX_FEED, "OPDS feed exceeds 8 MiB");
    let base = http_url(base_url)?;
    let text = std::str::from_utf8(bytes)?
        .trim_start_matches('\u{feff}')
        .trim_start();
    if text.starts_with('{') {
        parse_opds_json(text, &base)
    } else {
        parse_opds_xml(text, &base)
    }
}

fn parse_opds_xml(text: &str, base: &Url) -> Result<OpdsPage> {
    let document = roxmltree::Document::parse(text).context("Invalid OPDS/Atom XML")?;
    let root = document.root_element();
    anyhow::ensure!(
        matches!(root.tag_name().name(), "feed" | "entry"),
        "This document is not an OPDS/Atom feed"
    );
    let mut page = OpdsPage {
        title: xml_child_text(root, "title").unwrap_or_else(|| "Online library".into()),
        url: base.to_string(),
        publications: Vec::new(),
        navigation: Vec::new(),
        next: None,
        previous: None,
    };
    let base = xml_base(root, base);
    let root_links = xml_links(root, &base);
    page.next = relation_href(&root_links, "next");
    page.previous =
        relation_href(&root_links, "previous").or_else(|| relation_href(&root_links, "prev"));
    let entries: Vec<_> = if root.tag_name().name() == "entry" {
        vec![root]
    } else {
        root.children()
            .filter(|node| node.is_element() && node.tag_name().name() == "entry")
            .collect()
    };
    for entry in entries {
        let entry_base = xml_base(entry, &base);
        let title = xml_child_text(entry, "title").unwrap_or_else(|| "Untitled".into());
        let links = xml_links(entry, &entry_base);
        let has_acquisition = links.iter().any(|link| {
            link.rel.iter().any(|rel| {
                rel.starts_with("http://opds-spec.org/acquisition")
                    || rel == "acquisition"
                    || rel == "download"
            })
        });
        if has_acquisition {
            page.publications.push(OpdsPublication {
                id: xml_child_text(entry, "id").unwrap_or_else(|| {
                    links
                        .first()
                        .map(|link| link.href.clone())
                        .unwrap_or_else(|| title.clone())
                }),
                title,
                authors: entry
                    .children()
                    .filter(|node| node.is_element() && node.tag_name().name() == "author")
                    .filter_map(|node| xml_child_text(node, "name"))
                    .collect(),
                description: xml_child_text(entry, "summary")
                    .or_else(|| xml_child_text(entry, "content")),
                cover_url: links
                    .iter()
                    .find(|link| {
                        link.rel.iter().any(|rel| {
                            rel.contains("/image") || rel == "cover" || rel == "thumbnail"
                        })
                    })
                    .map(|link| link.href.clone()),
                epub_url: direct_epub(&links),
            });
        } else {
            for mut link in links.into_iter().filter(is_catalog_link) {
                if link.title.is_empty() {
                    link.title = title.clone();
                }
                page.navigation.push(link);
            }
        }
    }
    Ok(page)
}

fn xml_base(node: roxmltree::Node<'_, '_>, fallback: &Url) -> Url {
    node.attribute(("http://www.w3.org/XML/1998/namespace", "base"))
        .and_then(|value| fallback.join(value).ok())
        .unwrap_or_else(|| fallback.clone())
}

fn xml_child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    let child = node
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == name)?;
    let text = child
        .descendants()
        .filter(|node| node.is_text())
        .filter_map(|node| node.text())
        .collect::<String>();
    (!text.trim().is_empty()).then(|| text.trim().to_owned())
}

fn xml_links(node: roxmltree::Node<'_, '_>, base: &Url) -> Vec<OpdsLink> {
    node.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "link")
        .filter_map(|node| {
            let href = resolve_http(&xml_base(node, base), node.attribute("href")?)?;
            Some(OpdsLink {
                title: node.attribute("title").unwrap_or_default().into(),
                href,
                media_type: node.attribute("type").map(Into::into),
                rel: node
                    .attribute("rel")
                    .unwrap_or("alternate")
                    .split_whitespace()
                    .map(Into::into)
                    .collect(),
            })
        })
        .collect()
}

fn parse_opds_json(text: &str, base: &Url) -> Result<OpdsPage> {
    let value: serde_json::Value = serde_json::from_str(text).context("Invalid OPDS 2 JSON")?;
    let links = json_links(value.get("links"), base);
    let mut page = OpdsPage {
        title: json_title(value.pointer("/metadata/title"))
            .unwrap_or_else(|| "Online library".into()),
        url: base.to_string(),
        publications: Vec::new(),
        navigation: Vec::new(),
        next: relation_href(&links, "next"),
        previous: relation_href(&links, "previous").or_else(|| relation_href(&links, "prev")),
    };
    append_json_collection(&value, base, &mut page);
    if let Some(groups) = value.get("groups").and_then(|value| value.as_array()) {
        for group in groups {
            append_json_collection(group, base, &mut page);
        }
    }
    // A standalone OPDS publication is also a valid catalog target.
    if value.get("publications").is_none()
        && value.get("navigation").is_none()
        && value.get("groups").is_none()
        && value.get("metadata").is_some()
    {
        page.publications.push(json_publication(&value, base));
    }
    Ok(page)
}

fn append_json_collection(value: &serde_json::Value, base: &Url, page: &mut OpdsPage) {
    if let Some(publications) = value.get("publications").and_then(|value| value.as_array()) {
        page.publications.extend(
            publications
                .iter()
                .map(|value| json_publication(value, base)),
        );
    }
    page.navigation
        .extend(json_links(value.get("navigation"), base));
}

fn json_publication(value: &serde_json::Value, base: &Url) -> OpdsPublication {
    let links = json_links(value.get("links"), base);
    let images = json_links(value.get("images"), base);
    let title = json_title(value.pointer("/metadata/title")).unwrap_or_else(|| "Untitled".into());
    let mut authors = Vec::new();
    if let Some(author) = value.pointer("/metadata/author") {
        json_authors(author, &mut authors);
    }
    OpdsPublication {
        id: value
            .pointer("/metadata/identifier")
            .and_then(|value| value.as_str())
            .map(Into::into)
            .unwrap_or_else(|| {
                links
                    .first()
                    .map(|link| link.href.clone())
                    .unwrap_or_else(|| title.clone())
            }),
        title,
        authors,
        description: json_title(value.pointer("/metadata/description")),
        cover_url: images.first().map(|link| link.href.clone()),
        epub_url: direct_epub(&links),
    }
}

fn json_title(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    value.as_str().map(Into::into).or_else(|| {
        value
            .as_object()?
            .values()
            .find_map(|value| value.as_str().map(Into::into))
    })
}

fn json_authors(value: &serde_json::Value, result: &mut Vec<String>) {
    if let Some(text) = value.as_str() {
        result.push(text.into());
    } else if let Some(array) = value.as_array() {
        for value in array {
            json_authors(value, result);
        }
    } else if let Some(name) = json_title(value.get("name")) {
        result.push(name);
    }
}

fn json_links(value: Option<&serde_json::Value>, base: &Url) -> Vec<OpdsLink> {
    value
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let href = resolve_http(base, value.get("href")?.as_str()?)?;
            let rel = match value.get("rel") {
                Some(serde_json::Value::String(rel)) => {
                    rel.split_whitespace().map(Into::into).collect()
                }
                Some(serde_json::Value::Array(rel)) => rel
                    .iter()
                    .filter_map(|value| value.as_str().map(Into::into))
                    .collect(),
                _ => Vec::new(),
            };
            Some(OpdsLink {
                title: json_title(value.get("title")).unwrap_or_default(),
                href,
                media_type: value
                    .get("type")
                    .and_then(|value| value.as_str())
                    .map(Into::into),
                rel,
            })
        })
        .collect()
}

fn relation_href(links: &[OpdsLink], relation: &str) -> Option<String> {
    links
        .iter()
        .find(|link| link.rel.iter().any(|rel| rel == relation))
        .map(|link| link.href.clone())
}
fn is_catalog_link(link: &OpdsLink) -> bool {
    link.media_type.as_deref().is_some_and(|mime| {
        mime.contains("atom+xml")
            || mime.contains("opds+json")
            || mime.contains("opds-publication+json")
    })
}
fn direct_epub(links: &[OpdsLink]) -> Option<String> {
    links
        .iter()
        .find(|link| {
            link.media_type
                .as_deref()
                .is_some_and(|mime| mime.split(';').next() == Some("application/epub+zip"))
                && link.rel.iter().any(|rel| {
                    matches!(
                        rel.as_str(),
                        "acquisition"
                            | "download"
                            | "http://opds-spec.org/acquisition"
                            | "http://opds-spec.org/acquisition/open-access"
                    )
                })
        })
        .map(|link| link.href.clone())
}

#[derive(Clone, Debug)]
pub struct WebImportResult {
    pub path: PathBuf,
    pub title: String,
    pub source_url: String,
    pub warnings: Vec<String>,
    pub image_count: usize,
}

pub fn import_web_page(url: &str, data_dir: &Path) -> Result<WebImportResult> {
    import_web_page_with_client(&http_client()?, url, data_dir)
}

fn import_web_page_with_client(
    client: &Client,
    url: &str,
    data_dir: &Path,
) -> Result<WebImportResult> {
    let response = client
        .get(http_url(url)?)
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
        .map_err(network_error)?;
    let final_url = response.url().clone();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|header| header.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    anyhow::ensure!(
        content_type.is_empty()
            || content_type.contains("html")
            || content_type.starts_with("text/"),
        "This URL does not return an HTML page"
    );
    let bytes = response_bytes(response, MAX_FEED)?;
    let text = decode_html(&bytes, &content_type)?;
    let document = Html::parse_document(&text);
    let title = document
        .select(&selector("title"))
        .next()
        .map(|title| title.text().collect::<String>())
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| final_url.host_str().unwrap_or("Web page").into());
    let article = document
        .select(&selector("article"))
        .next()
        .or_else(|| document.select(&selector("main")).next())
        .or_else(|| document.select(&selector("body")).next())
        .unwrap_or_else(|| document.root_element());
    let readable = article.text().collect::<String>();
    let base = document
        .select(&selector("base[href]"))
        .next()
        .and_then(|element| element.value().attr("href"))
        .and_then(|href| final_url.join(href).ok())
        .unwrap_or_else(|| final_url.clone());
    let destination = data_dir.join("web");
    fs::create_dir_all(&destination)?;
    let pending = tempfile::Builder::new()
        .prefix("page-")
        .tempdir_in(&destination)?;
    fs::create_dir(pending.path().join("images"))?;
    let mut images = HashMap::<String, String>::new();
    let mut downloaded = HashMap::<String, String>::new();
    let mut warnings = Vec::new();
    let mut total_bytes = 0usize;
    for image in article.select(&selector("img")) {
        let Some(raw) = image_source(image) else {
            continue;
        };
        if images.contains_key(raw) {
            continue;
        }
        if downloaded.len() >= 64 {
            warnings.push("Only the first 64 images were saved".into());
            break;
        }
        let resolved = if raw.starts_with("data:") {
            raw.into()
        } else if let Some(url) = resolve_http(&base, raw) {
            url
        } else {
            warnings.push("An image uses an unsupported URL".into());
            continue;
        };
        if let Some(local) = downloaded.get(&resolved) {
            images.insert(raw.into(), local.clone());
            continue;
        }
        let fetched = if resolved.starts_with("data:") {
            data_image(&resolved)
        } else {
            fetch_image(client, &resolved)
        };
        match fetched {
            Ok((bytes, extension)) => {
                if total_bytes + bytes.len() > 64 * 1024 * 1024 {
                    warnings.push("The saved page image budget is 64 MiB".into());
                    break;
                }
                total_bytes += bytes.len();
                let local = format!("images/{}.{}", blake3::hash(&bytes).to_hex(), extension);
                fs::write(pending.path().join(&local), bytes)?;
                images.insert(raw.into(), local.clone());
                downloaded.insert(resolved, local);
            }
            Err(_) => warnings.push("An image could not be saved for offline reading".into()),
        }
    }
    if readable.trim().chars().count() < 100
        && document.select(&selector("script")).next().is_some()
    {
        warnings.push(
            "This page may generate its main text with JavaScript; check the imported text".into(),
        );
    }
    let mut body = String::new();
    render_static_element(article, &images, &base, &mut body);
    let visible = Html::parse_fragment(&body)
        .root_element()
        .text()
        .collect::<String>();
    anyhow::ensure!(
        !visible.trim().is_empty() || !images.is_empty(),
        "The page has no static readable content; JavaScript-rendered pages are unavailable"
    );
    let mut styles = String::new();
    for style in document.select(&selector("style")) {
        let css = style.text().collect::<String>();
        if styles.len() + css.len() <= 256 * 1024 {
            styles.push_str(&css);
            styles.push('\n');
        }
    }
    // Inline CSS is interpreted by the native reader's supported subset. No
    // scripts, iframes, remote image URLs or external CSS survive this snapshot.
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>{}</style></head><body>{body}</body></html>",
        escape_html(title.trim()),
        styles.replace("</style", "<\\/style")
    );
    fs::write(pending.path().join("index.html"), html)?;
    fs::write(
        pending.path().join("source.json"),
        serde_json::to_vec(&serde_json::json!({"url":final_url.as_str(),"title":title.trim()}))?,
    )?;
    let directory = pending.keep();
    Ok(WebImportResult {
        path: directory.join("index.html"),
        title: title.trim().into(),
        source_url: final_url.to_string(),
        warnings,
        image_count: downloaded.len(),
    })
}

fn render_static_element(
    element: ElementRef<'_>,
    images: &HashMap<String, String>,
    base: &Url,
    output: &mut String,
) {
    let tag = element.value().name();
    if matches!(
        tag,
        "script"
            | "iframe"
            | "object"
            | "embed"
            | "link"
            | "base"
            | "form"
            | "input"
            | "button"
            | "noscript"
            | "style"
            | "nav"
    ) {
        return;
    }
    if tag == "img" {
        if let Some(local) = image_source(element).and_then(|raw| images.get(raw)) {
            output.push_str(&format!(
                "<img src=\"{}\" alt=\"{}\"",
                escape_html(local),
                escape_html(element.value().attr("alt").unwrap_or_default())
            ));
            for attribute in ["width", "height"] {
                if let Some(value) = element.value().attr(attribute) {
                    output.push_str(&format!(" {attribute}=\"{}\"", escape_html(value)));
                }
            }
            output.push('>');
        } else if let Some(alt) = element
            .value()
            .attr("alt")
            .filter(|value| !value.is_empty())
        {
            output.push_str(&format!("<span>[{}]</span>", escape_html(alt)));
        }
        return;
    }
    let wrapper = !matches!(tag, "html" | "head" | "body");
    if wrapper {
        output.push('<');
        output.push_str(tag);
        for (name, value) in element.value().attrs() {
            if name.starts_with("on")
                || matches!(name, "src" | "srcset" | "action" | "formaction" | "srcdoc")
            {
                continue;
            }
            let value = if name == "href" {
                if value.starts_with('#') {
                    Some(value.to_owned())
                } else {
                    resolve_http(base, value)
                }
            } else {
                Some(value.to_owned())
            };
            if let Some(value) = value {
                output.push_str(&format!(" {name}=\"{}\"", escape_html(&value)));
            }
        }
        output.push('>');
    }
    for child in element.children() {
        match child.value() {
            scraper::node::Node::Text(text) => output.push_str(&escape_html(&text.text)),
            scraper::node::Node::Element(_) => {
                if let Some(element) = ElementRef::wrap(child) {
                    render_static_element(element, images, base, output)
                }
            }
            _ => {}
        }
    }
    if wrapper && !matches!(tag, "br" | "hr" | "meta" | "wbr" | "source") {
        output.push_str("</");
        output.push_str(tag);
        output.push('>');
    }
}

fn image_source(element: ElementRef<'_>) -> Option<&str> {
    element
        .value()
        .attr("src")
        .filter(|value| !value.is_empty() && !value.starts_with("data:"))
        .or_else(|| element.value().attr("data-src"))
        .or_else(|| element.value().attr("data-original"))
        .or_else(|| element.value().attr("src"))
        .or_else(|| {
            element
                .value()
                .attr("srcset")
                .and_then(|value| value.split(',').next()?.split_whitespace().next())
        })
}

fn fetch_image(client: &Client, url: &str) -> Result<(Vec<u8>, String)> {
    let response = client.get(http_url(url)?).send().map_err(network_error)?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    anyhow::ensure!(
        content_type.is_empty()
            || content_type.starts_with("image/")
            || content_type.starts_with("application/octet-stream"),
        "Resource is not an image"
    );
    let bytes = response_bytes(response, MAX_IMAGE)?;
    Ok((bytes, image_extension(&content_type, url)))
}

fn data_image(uri: &str) -> Result<(Vec<u8>, String)> {
    let (header, data) = uri.split_once(',').context("Invalid data image")?;
    anyhow::ensure!(
        header.starts_with("data:image/"),
        "Only image data URLs are accepted"
    );
    let bytes = if header.ends_with(";base64") {
        anyhow::ensure!(data.len() <= MAX_IMAGE * 2, "Data image too large");
        base64::engine::general_purpose::STANDARD.decode(data)?
    } else {
        percent_encoding::percent_decode_str(data).collect()
    };
    anyhow::ensure!(bytes.len() <= MAX_IMAGE, "Data image exceeds 16 MiB");
    Ok((
        bytes,
        image_extension(header.trim_start_matches("data:"), ""),
    ))
}

fn image_extension(mime: &str, url: &str) -> String {
    match mime.split(';').next().unwrap_or_default() {
        "image/png" => "png".into(),
        "image/jpeg" => "jpg".into(),
        "image/gif" => "gif".into(),
        "image/webp" => "webp".into(),
        "image/svg+xml" => "svg".into(),
        "image/avif" => "avif".into(),
        _ => Url::parse(url)
            .ok()
            .and_then(|url| {
                Path::new(url.path())
                    .extension()
                    .and_then(|value| value.to_str())
                    .map(str::to_ascii_lowercase)
            })
            .filter(|extension| {
                matches!(
                    extension.as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "avif"
                )
            })
            .unwrap_or_else(|| "bin".into()),
    }
}

fn decode_html(bytes: &[u8], content_type: &str) -> Result<String> {
    let header_encoding = content_type
        .split(';')
        .find_map(|part| part.trim().strip_prefix("charset="))
        .and_then(|label| {
            encoding_rs::Encoding::for_label(label.trim_matches(['\'', '"']).as_bytes())
        });
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]);
    let prefix_doc = Html::parse_document(&prefix);
    let meta_encoding = prefix_doc
        .select(&selector("meta[charset]"))
        .find_map(|meta| {
            encoding_rs::Encoding::for_label(meta.value().attr("charset")?.as_bytes())
        });
    let (encoding, bom) = encoding_rs::Encoding::for_bom(bytes).unwrap_or((
        header_encoding
            .or(meta_encoding)
            .unwrap_or(encoding_rs::UTF_8),
        0,
    ));
    let (text, _, errors) = encoding.decode(&bytes[bom..]);
    anyhow::ensure!(
        !errors,
        "The webpage contains invalid {} text",
        encoding.name()
    );
    Ok(text.into_owned())
}

fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("Yomi/0.1 (native ebook reader)")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()?)
}
fn http_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("Enter a complete HTTP(S) URL")?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Only HTTP(S) URLs are supported"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Enter credentials in the separate credential fields"
    );
    Ok(url)
}
fn resolve_http(base: &Url, href: &str) -> Option<String> {
    let url = base.join(href).ok()?;
    http_url(url.as_str()).ok().map(Into::into)
}
fn response_bytes(mut response: Response, limit: usize) -> Result<Vec<u8>> {
    anyhow::ensure!(
        response.status().is_success(),
        "Server returned HTTP {}",
        response.status().as_u16()
    );
    anyhow::ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= limit as u64),
        "Response exceeds the {} MiB limit",
        limit / 1024 / 1024
    );
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= limit, "Response exceeds the size limit");
    Ok(bytes)
}
fn network_error(error: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("Network request failed: {}", error.without_url())
}
fn redacted_url(value: &str) -> String {
    Url::parse(value)
        .map(|mut url| {
            url.set_query(None);
            url.set_fragment(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        })
        .unwrap_or_else(|_| "[invalid URL]".into())
}
fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("Static selector is valid")
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        thread,
    };

    struct Request {
        path: String,
        headers: HashMap<String, String>,
    }

    fn server(
        count: usize,
        handler: impl Fn(Request) -> (u16, &'static str, Vec<u8>) + Send + 'static,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let thread = thread::spawn(move || {
            for _ in 0..count {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "Mock server request timed out"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("Mock server accept failed: {error}"),
                    }
                };
                // Windows may inherit the listener's nonblocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                assert_eq!(first.split_whitespace().next(), Some("GET"));
                let path = first.split_whitespace().nth(1).unwrap().to_owned();
                let mut headers = HashMap::<String, String>::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.to_ascii_lowercase(), value.trim().into());
                    }
                }
                drop(reader);
                let (status, mime, body) = handler(Request { path, headers });
                write!(stream,"HTTP/1.1 {status} Response\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        (address, thread)
    }

    fn local_client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    #[test]
    fn opds_atom_resolves_navigation_pagination_and_free_acquisition() -> Result<()> {
        let xml=br#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Catalog</title><link rel="next" href="?page=2"/><entry><title>Authors</title><link href="authors" type="application/atom+xml;profile=opds-catalog"/></entry><entry xml:base="../books/"><id>urn:book:1</id><title>A Book</title><author><name>Author One</name></author><author><name>Author Two</name></author><link rel="http://opds-spec.org/acquisition/open-access" href="one.epub" type="application/epub+zip"/><link rel="http://opds-spec.org/image" href="one.jpg"/></entry><entry><id>paid</id><title>Paid book</title><link rel="http://opds-spec.org/acquisition/buy" href="buy.epub" type="application/epub+zip"/></entry></feed>"#;
        let page = parse_opds(xml, "https://example.test/opds/feed")?;
        assert_eq!(
            page.next.as_deref(),
            Some("https://example.test/opds/feed?page=2")
        );
        assert_eq!(page.navigation[0].href, "https://example.test/opds/authors");
        assert_eq!(page.publications.len(), 2);
        assert_eq!(page.publications[0].authors, ["Author One", "Author Two"]);
        assert_eq!(
            page.publications[0].epub_url.as_deref(),
            Some("https://example.test/books/one.epub")
        );
        assert!(page.publications[1].epub_url.is_none());
        Ok(())
    }

    #[test]
    fn opds_two_groups_and_relation_arrays_are_supported() -> Result<()> {
        let json = serde_json::json!({"metadata":{"title":{"ja":"本棚"}},"links":[{"rel":["next"],"href":"?page=3"}],"groups":[{"navigation":[{"title":"新着","href":"/new","type":"application/opds+json"}]},{"publications":[{"metadata":{"identifier":"id1","title":"日本語の本","author":[{"name":"著者"},"Second"]},"links":[{"rel":"http://opds-spec.org/acquisition/open-access","href":"/book.epub","type":"application/epub+zip"}],"images":[{"href":"/cover.png","type":"image/png"}]}]}]});
        let page = parse_opds(&serde_json::to_vec(&json)?, "https://example.test/feed")?;
        assert_eq!(page.title, "本棚");
        assert_eq!(page.navigation[0].title, "新着");
        assert_eq!(page.publications[0].authors, ["著者", "Second"]);
        assert_eq!(
            page.publications[0].cover_url.as_deref(),
            Some("https://example.test/cover.png")
        );
        assert_eq!(
            page.next.as_deref(),
            Some("https://example.test/feed?page=3")
        );
        Ok(())
    }

    #[test]
    fn opds_fetch_and_download_use_local_server_and_app_owned_paths() -> Result<()> {
        let (address, handle) = server(2, |request| {
            assert!(
                request
                    .headers
                    .get("authorization")
                    .is_some_and(|header| header.starts_with("Basic "))
            );
            if request.path == "/feed" {
                (200,"application/opds+json",br#"{"metadata":{"title":"Local"},"publications":[{"metadata":{"title":"Book"},"links":[{"rel":"acquisition","href":"/book.epub","type":"application/epub+zip"}]}]}"#.to_vec())
            } else {
                assert_eq!(request.path, "/book.epub");
                (
                    200,
                    "application/epub+zip",
                    b"PK\x03\x04test-only ZIP transport fixture".to_vec(),
                )
            }
        });
        let config = OpdsConfig {
            url: format!("{address}/feed"),
            username: Some("user".into()),
            password: Some("test-password".into()),
        };
        let mut client = OpdsClient::new(config)?;
        client.client = local_client();
        let page = client.fetch(&format!("{address}/feed"))?;
        let directory = tempfile::tempdir()?;
        let path = client.download_epub(
            page.publications[0].epub_url.as_deref().unwrap(),
            directory.path(),
        )?;
        assert!(path.starts_with(directory.path().join("downloads")));
        assert_eq!(path.extension().unwrap(), "epub");
        assert!(fs::read(path)?.starts_with(b"PK\x03\x04"));
        handle.join().unwrap();
        Ok(())
    }

    #[test]
    fn catalog_credentials_are_not_attached_to_foreign_origin() -> Result<()> {
        let (address, handle) = server(1, |request| {
            assert!(!request.headers.contains_key("authorization"));
            (
                200,
                "application/opds+json",
                br#"{"metadata":{"title":"Public"},"publications":[]}"#.to_vec(),
            )
        });
        let config = OpdsConfig {
            url: "https://private.example.test/feed".into(),
            username: Some("private-user".into()),
            password: Some("private-password".into()),
        };
        let mut client = OpdsClient::new(config)?;
        client.client = local_client();
        client.fetch(&address)?;
        handle.join().unwrap();
        Ok(())
    }

    #[test]
    fn static_web_import_saves_images_removes_scripts_and_opens_natively() -> Result<()> {
        let (address, handle) = server(2, |request| {
            if request.path == "/article" {
                (200,"text/html; charset=utf-8","<html><head><title>日本語の記事</title></head><body><nav>navigation</nav><article><h1>見出し</h1><p>本文と<ruby>漢字<rt>かんじ</rt></ruby>。</p><img src='/image.svg' alt='図'><script>document.body.innerHTML='unsafe'</script><iframe src='https://example.test/'></iframe></article></body></html>".as_bytes().to_vec())
            } else {
                assert_eq!(request.path, "/image.svg");
                (200,"image/svg+xml",br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><circle cx="5" cy="5" r="4"/></svg>"#.to_vec())
            }
        });
        let directory = tempfile::tempdir()?;
        let imported = import_web_page_with_client(
            &local_client(),
            &format!("{address}/article"),
            directory.path(),
        )?;
        assert_eq!(imported.title, "日本語の記事");
        assert_eq!(imported.image_count, 1);
        assert!(imported.path.starts_with(directory.path().join("web")));
        let html = fs::read_to_string(&imported.path)?;
        assert!(!html.contains("<script"));
        assert!(!html.contains("<iframe"));
        assert!(!html.contains("navigation"));
        assert!(html.contains("images/"));
        assert!(html.contains("<ruby>"));
        let document = reader_core::open(&imported.path)?;
        assert_eq!(document.title, "日本語の記事");
        assert!(document.chapters[0].plain_text().contains("本文"));
        assert_eq!(document.resources.len(), 1);
        handle.join().unwrap();
        Ok(())
    }

    #[test]
    fn script_only_pages_fail_instead_of_importing_an_empty_book() -> Result<()> {
        let (address, handle) = server(1, |_| {
            (
                200,
                "text/html",
                b"<html><body><script>let generated='not static';</script></body></html>".to_vec(),
            )
        });
        let directory = tempfile::tempdir()?;
        assert!(import_web_page_with_client(&local_client(), &address, directory.path()).is_err());
        handle.join().unwrap();
        Ok(())
    }

    #[test]
    fn catalog_credentials_are_redacted_and_unsafe_urls_are_rejected() {
        let config = OpdsConfig {
            url: "https://example.test/feed?token=secret-query".into(),
            username: Some("secret-user".into()),
            password: Some("secret-password".into()),
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-"));
        assert!(http_url("file:///C:/private.epub").is_err());
        assert!(http_url("http://user:pass@example.test/feed").is_err());
    }
}
