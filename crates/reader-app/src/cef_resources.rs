//! Read-only EPUB resources for the book-only Chromium surface.
//!
//! No HTML wrappers or injected nodes are added here: XML MIME and the original
//! decoded DOM determine the shared reader_core locator identities.
use anyhow::{Result, ensure};
use reader_core::Book;
use std::{collections::BTreeMap, ops::Range, sync::Arc};

#[derive(Clone)]
pub struct BookOrigin {
    token: String,
    source: Arc<Book>,
    memory_resources: Arc<BTreeMap<String, Arc<Vec<u8>>>>,
    mime_types: Arc<BTreeMap<String, String>>,
}

impl BookOrigin {
    pub fn new(book: Book) -> Result<Self> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|error| anyhow::anyhow!("Book origin randomness: {error}"))?;
        Self::with_token(
            book,
            random.iter().map(|byte| format!("{byte:02x}")).collect(),
        )
    }

    fn with_token(mut book: Book, token: String) -> Result<Self> {
        ensure!(
            !token.is_empty()
                && token.len() <= 63
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !token.starts_with('-')
                && !token.ends_with('-'),
            "Invalid book origin token"
        );
        // Keep the lazy archive and in-memory assets, without retaining copies
        // of the parsed chapters, search results, or native page geometry.
        book.chapters.clear();
        book.toc.clear();
        let memory_resources = std::mem::take(&mut book.resources)
            .into_iter()
            .map(|(path, bytes)| (path, Arc::new(bytes)))
            .collect();
        Ok(Self {
            token,
            source: Arc::new(book),
            memory_resources: Arc::new(memory_resources),
            mime_types: Arc::new(BTreeMap::new()),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// `path` is a decoded archive name, not an already escaped URL.
    pub fn url(&self, path: &str) -> Result<String> {
        validate_path(path)?;
        let mut encoded = String::new();
        for byte in path.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
                encoded.push(byte as char);
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        Ok(format!("vobr://{}/{encoded}", self.token))
    }

    /// Manifest media types take priority over filename inference. Keys are
    /// decoded paths; changing this table never mutates the source publication.
    /// Set this before creating/registering factories: clones retain a snapshot.
    pub fn set_mime_types(&mut self, mime_types: BTreeMap<String, String>) -> Result<()> {
        for (path, mime) in &mime_types {
            validate_path(path)?;
            ensure!(
                mime.contains('/') && !mime.contains(['\r', '\n', '\0']) && mime.is_ascii(),
                "Invalid book resource MIME type"
            );
        }
        self.mime_types = Arc::new(mime_types);
        Ok(())
    }

    pub fn path_from_url(&self, url: &str) -> Result<String> {
        let rest = url
            .strip_prefix("vobr://")
            .ok_or_else(|| anyhow::anyhow!("Not a book resource URL"))?;
        let (authority, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("Book resource URL has no path"))?;
        ensure!(
            authority == self.token,
            "Book resource belongs to another origin"
        );
        let path = path.split(['?', '#']).next().unwrap_or("");
        let path = decode_once(path)?;
        validate_path(&path)?;
        Ok(path)
    }

    /// Pure response preparation, also used by the asynchronous CEF adapter.
    /// A single byte range is supported; the native stream accounts for CEF's
    /// automatic initial Skip(), rather than skipping the requested prefix twice.
    pub fn response(&self, url: &str, method: &str, range: Option<&str>) -> BookResponse {
        let failure = |status, reason| {
            let mut response = BookResponse::error(status, reason);
            response.head = method == "HEAD";
            response
        };
        if !matches!(method, "GET" | "HEAD") {
            return failure(405, "Method Not Allowed");
        }
        let path = match self.path_from_url(url) {
            Ok(path) => path,
            Err(_) => return failure(403, "Forbidden"),
        };
        let resource = if let Some(bytes) = self.memory_resources.get(&path) {
            Ok(Some(Arc::clone(bytes)))
        } else {
            self.source.resource_bytes(&path)
        };
        let mut bytes = match resource {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return failure(404, "Not Found"),
            Err(_) => return failure(500, "Resource Read Failed"),
        };
        // CEF's custom StreamReaderURLLoader does not apply HTTP content
        // decoders. Inflate SVGZ here, off the IO thread, and serve plain SVG.
        if path.to_ascii_lowercase().ends_with(".svgz") && bytes.starts_with(&[0x1f, 0x8b]) {
            use std::io::Read;
            const LIMIT: u64 = 64 * 1024 * 1024;
            let mut inflated = Vec::new();
            let decoder = flate2::read::GzDecoder::new(bytes.as_slice());
            if decoder.take(LIMIT + 1).read_to_end(&mut inflated).is_err() {
                return failure(500, "Invalid Compressed SVG");
            }
            if inflated.len() as u64 > LIMIT {
                return failure(413, "SVG Resource Too Large");
            }
            bytes = Arc::new(inflated);
        }
        let size = bytes.len();
        let requested = if method == "HEAD" {
            None
        } else {
            range.filter(|header| !header.is_empty())
        };
        let (window, cef_skip) = match requested {
            Some(header) => match byte_range(header, size) {
                Ok(range) => range,
                Err(_) => {
                    let mut response = BookResponse::error(416, "Range Not Satisfiable");
                    response
                        .headers
                        .push(("Content-Range".into(), format!("bytes */{size}")));
                    return response;
                }
            },
            None => (0..size, 0),
        };
        let mut headers = vec![("Accept-Ranges".into(), "bytes".into())];
        if requested.is_some() {
            headers.push((
                "Content-Range".into(),
                format!("bytes {}-{}/{size}", window.start, window.end - 1),
            ));
        }
        headers.push(("Content-Length".into(), window.len().to_string()));
        BookResponse {
            status: if requested.is_some() { 206 } else { 200 },
            reason: if requested.is_some() {
                "Partial Content"
            } else {
                "OK"
            },
            mime: self
                .mime_types
                .get(&path)
                .cloned()
                .unwrap_or_else(|| infer_mime(&path).into()),
            bytes,
            window,
            cef_skip,
            head: method == "HEAD",
            headers,
        }
    }
}

fn validate_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && !path.starts_with('/')
            && !path.contains(['\\', '\0'])
            && !path.chars().any(char::is_control),
        "Invalid archive resource path"
    );
    ensure!(
        path.split('/').all(|part| !matches!(part, "" | "." | "..")),
        "Archive traversal is not a resource path"
    );
    Ok(())
}

fn decode_once(path: &str) -> Result<String> {
    let input = path.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            ensure!(index + 2 < input.len(), "Truncated URL escape");
            let high = (input[index + 1] as char)
                .to_digit(16)
                .ok_or_else(|| anyhow::anyhow!("Invalid URL escape"))?;
            let low = (input[index + 2] as char)
                .to_digit(16)
                .ok_or_else(|| anyhow::anyhow!("Invalid URL escape"))?;
            output.push((high * 16 + low) as u8);
            index += 3;
        } else {
            output.push(input[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8(output)?)
}

fn byte_range(header: &str, size: usize) -> Result<(Range<usize>, usize)> {
    let value = header
        .trim()
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow::anyhow!("Unsupported range unit"))?;
    ensure!(
        !value.contains(',') && size > 0,
        "Unsupported or empty byte range"
    );
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("Invalid byte range"))?;
    if start.is_empty() {
        let count = end.parse::<usize>()?;
        ensure!(count > 0, "Empty suffix range");
        return Ok((size.saturating_sub(count)..size, 0));
    }
    let start = start.parse::<usize>()?;
    ensure!(start < size, "Range begins after resource");
    let end = if end.is_empty() {
        size
    } else {
        end.parse::<usize>()?.saturating_add(1).min(size)
    };
    ensure!(end > start, "Reversed byte range");
    Ok((start..end, start))
}

fn infer_mime(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "xhtml" | "xht" => "application/xhtml+xml",
        "html" | "htm" => "text/html",
        "svg" | "svgz" => "image/svg+xml",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "opf" => "application/oebps-package+xml",
        "ncx" => "application/x-dtbncx+xml",
        "smil" => "application/smil+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "ttc" => "font/collection",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "mp3" => "audio/mpeg",
        "m4a" | "m4b" => "audio/mp4",
        "wav" => "audio/wav",
        "oga" | "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "txt" => "text/plain",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

#[derive(Clone)]
pub struct BookResponse {
    pub status: u16,
    pub reason: &'static str,
    pub mime: String,
    pub bytes: Arc<Vec<u8>>,
    window: Range<usize>,
    cef_skip: usize,
    head: bool,
    pub headers: Vec<(String, String)>,
}
impl BookResponse {
    fn error(status: u16, reason: &'static str) -> Self {
        let bytes = Arc::new(reason.as_bytes().to_vec());
        let size = bytes.len();
        let mut headers = Vec::new();
        if status == 405 {
            headers.push(("Allow".into(), "GET, HEAD".into()));
        }
        Self {
            status,
            reason,
            mime: "text/plain".into(),
            bytes,
            window: 0..size,
            cef_skip: 0,
            head: false,
            headers,
        }
    }
    pub fn body(&self) -> &[u8] {
        if self.head {
            &[]
        } else {
            &self.bytes[self.window.clone()]
        }
    }
    pub fn content_length(&self) -> usize {
        self.window.len()
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use cef::*;
    use reader_renderer_cef::cef;
    use std::sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Sender},
    };

    type Job = Box<dyn FnOnce() + Send + 'static>;
    static READS: AtomicUsize = AtomicUsize::new(0);
    fn workers() -> &'static Sender<Job> {
        static QUEUE: OnceLock<Sender<Job>> = OnceLock::new();
        QUEUE.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            for _ in 0..2 {
                let rx = Arc::clone(&rx);
                std::thread::spawn(move || {
                    loop {
                        let job = rx.lock().unwrap_or_else(|error| error.into_inner()).recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => break,
                        }
                    }
                });
            }
            tx
        })
    }
    /// Drain asynchronous reads before shutting down the last CEF runtime.
    pub fn pending_resource_reads() -> usize {
        READS.load(Ordering::Acquire)
    }
    struct Pending;
    impl Drop for Pending {
        fn drop(&mut self) {
            READS.fetch_sub(1, Ordering::Release);
        }
    }

    #[derive(Default)]
    struct Stream {
        response: Option<BookResponse>,
        cursor: usize,
        cancelled: bool,
        opened: bool,
    }
    impl Stream {
        fn ready(&mut self, response: BookResponse) {
            // CEF skips normal ranges before requesting headers. Suffix ranges
            // do not trigger that CEF path, and begin directly at their suffix.
            self.cursor = if response.cef_skip > 0 {
                0
            } else {
                response.window.start
            };
            self.response = Some(response);
        }
        fn read(&mut self, out: &mut [u8]) -> usize {
            if self.cancelled {
                return 0;
            }
            let Some(response) = &self.response else {
                return 0;
            };
            if response.head {
                return 0;
            }
            self.cursor = self.cursor.max(response.window.start);
            let count = out
                .len()
                .min(response.window.end.saturating_sub(self.cursor));
            out[..count].copy_from_slice(&response.bytes[self.cursor..self.cursor + count]);
            self.cursor += count;
            count
        }
        fn skip(&mut self, count: usize) -> usize {
            let Some(response) = &self.response else {
                return 0;
            };
            let count = count.min(response.bytes.len().saturating_sub(self.cursor));
            self.cursor += count;
            count
        }
    }

    wrap_resource_handler! {
        struct BookHandler {origin:BookOrigin,state:Arc<Mutex<Stream>>}
        impl ResourceHandler {
            fn open(&self,request:Option<&mut Request>,handle_request:Option<&mut i32>,callback:Option<&mut Callback>)->i32 {
                let Some(handle_request)=handle_request else{return 0;};
                let Some(request)=request else{*handle_request=1;return 0;};
                let Some(callback)=callback.cloned() else{*handle_request=1;return 0;};
                let url=CefString::from(&request.url()).to_string();let method=CefString::from(&request.method()).to_string();
                let range=CefString::from(&request.header_by_name(Some(&CefString::from("Range")))).to_string();
                let mut state=self.state.lock().unwrap_or_else(|error|error.into_inner());
                if state.cancelled {*handle_request=1;return 0;}
                if state.opened {*handle_request=i32::from(state.response.is_some());return 1;}
                state.opened=true;drop(state);*handle_request=0;
                let origin=self.origin.clone();let state=Arc::clone(&self.state);
                READS.fetch_add(1,Ordering::AcqRel);let pending=Pending;
                let job:Job=Box::new(move || {
                    let _pending=pending;
                    if state.lock().unwrap_or_else(|error|error.into_inner()).cancelled {return;}
                    let response=origin.response(&url,&method,Some(&range));
                    let mut state=state.lock().unwrap_or_else(|error|error.into_inner());
                    if state.cancelled {return;}
                    state.ready(response);drop(state);callback.cont();
                });
                if workers().send(job).is_err() {*handle_request=1;return 0;}
                1
            }
            fn response_headers(&self,response:Option<&mut Response>,response_length:Option<&mut i64>,_redirect_url:Option<&mut CefString>) {
                let state=self.state.lock().unwrap_or_else(|error|error.into_inner());
                let Some(data)=&state.response else {if let Some(length)=response_length{*length=0;}return;};
                if let Some(length)=response_length {*length=data.content_length() as i64;}
                if let Some(response)=response {
                    response.set_status(i32::from(data.status));response.set_status_text(Some(&CefString::from(data.reason)));
                    response.set_mime_type(Some(&CefString::from(data.mime.as_str())));
                    let mut headers=CefStringMultimap::new();
                    for (key,value) in &data.headers {headers.append(key,value);}
                    headers.append("X-Content-Type-Options","nosniff");headers.append("Referrer-Policy","no-referrer");
                    headers.append("Cross-Origin-Resource-Policy","same-origin");
                    headers.append("Content-Security-Policy","default-src 'self' data: blob:; style-src 'self' 'unsafe-inline' data: blob:; script-src 'none'; object-src 'self'; connect-src 'none'; base-uri 'self'; frame-src 'self'; form-action 'none'");
                    response.set_header_map(Some(&mut headers));
                }
            }
            fn read(&self,data_out:*mut u8,bytes_to_read:i32,bytes_read:Option<&mut i32>,_callback:Option<&mut ResourceReadCallback>)->i32 {
                let Some(bytes_read)=bytes_read else{return 0;};*bytes_read=0;
                if bytes_to_read<=0 || data_out.is_null(){return 0;}
                // SAFETY: CEF owns this output buffer and guarantees its length
                // for this synchronous Read call. No pointer survives the call.
                let output=unsafe{std::slice::from_raw_parts_mut(data_out,bytes_to_read as usize)};
                let count=self.state.lock().unwrap_or_else(|error|error.into_inner()).read(output);
                *bytes_read=count as i32;i32::from(count>0)
            }
            fn skip(&self,bytes_to_skip:i64,bytes_skipped:Option<&mut i64>,_callback:Option<&mut ResourceSkipCallback>)->i32 {
                let Some(bytes_skipped)=bytes_skipped else{return 0;};*bytes_skipped = -2;
                if bytes_to_skip<0{return 0;}
                let count=self.state.lock().unwrap_or_else(|error|error.into_inner()).skip(bytes_to_skip as usize);
                *bytes_skipped=count as i64;i32::from(count>0 || bytes_to_skip==0)
            }
            fn cancel(&self) {let mut state=self.state.lock().unwrap_or_else(|error|error.into_inner());state.cancelled=true;state.response=None;}
        }
    }
    wrap_scheme_handler_factory! {
        struct BookFactory {origin:BookOrigin}
        impl SchemeHandlerFactory {
            fn create(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_scheme_name:Option<&CefString>,_request:Option<&mut Request>)->Option<ResourceHandler> {
                Some(BookHandler::new(self.origin.clone(),Arc::new(Mutex::new(Stream::default()))))
            }
        }
    }
    impl BookOrigin {
        pub fn factory(&self) -> SchemeHandlerFactory {
            BookFactory::new(self.clone())
        }
        /// Call only after CEF initialization and before the first navigation.
        pub fn register_global(&self) -> Result<()> {
            let mut factory = self.factory();
            ensure!(
                cef::register_scheme_handler_factory(
                    Some(&CefString::from("vobr")),
                    Some(&CefString::from(self.token())),
                    Some(&mut factory)
                ) != 0,
                "CEF rejected the book resource factory"
            );
            Ok(())
        }
        pub fn unregister_global(&self) -> Result<()> {
            ensure!(
                cef::register_scheme_handler_factory(
                    Some(&CefString::from("vobr")),
                    Some(&CefString::from(self.token())),
                    None
                ) != 0,
                "CEF rejected book resource removal"
            );
            Ok(())
        }
        pub fn register_in_context(&self, context: &RequestContext) -> Result<()> {
            let mut factory = self.factory();
            ensure!(
                context.register_scheme_handler_factory(
                    Some(&CefString::from("vobr")),
                    Some(&CefString::from(self.token())),
                    Some(&mut factory)
                ) != 0,
                "CEF rejected the contextual book resource factory"
            );
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn normal_range_is_skipped_once_by_cef_and_suffix_begins_at_its_offset() {
            let origin = super::super::tests::origin();
            let url = origin.url("media.mp4").unwrap();
            let mut stream = Stream::default();
            stream.ready(origin.response(&url, "GET", Some("bytes=2-5")));
            assert_eq!(stream.skip(2), 2);
            let mut out = [0; 20];
            assert_eq!(stream.read(&mut out), 4);
            assert_eq!(&out[..4], b"2345");
            assert_eq!(stream.read(&mut out), 0);
            let mut stream = Stream::default();
            stream.ready(origin.response(&url, "GET", Some("bytes=-3")));
            assert_eq!(stream.read(&mut out), 3);
            assert_eq!(&out[..3], b"789");
        }
    }
}
#[cfg(windows)]
pub use native::pending_resource_reads;

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn origin() -> BookOrigin {
        let mut book = Book::demo();
        book.resources.clear();
        book.resources
            .insert("media.mp4".into(), b"0123456789".to_vec());
        book.resources.insert(
            "OPS/注釈 1.xhtml".into(),
            b"<?xml version='1.0'?><html xmlns='http://www.w3.org/1999/xhtml'/>".to_vec(),
        );
        book.resources.insert(
            "OPS/%2e%2e.txt".into(),
            b"literal percent filename".to_vec(),
        );
        BookOrigin::with_token(book, "book0123".into()).unwrap()
    }
    #[test]
    fn url_roundtrip_is_exactly_one_decode_and_keeps_original_xhtml() {
        let origin = origin();
        let url = origin.url("OPS/注釈 1.xhtml").unwrap();
        assert!(url.contains("%E6%B3%A8"));
        assert_eq!(
            origin.path_from_url(&format!("{url}?x=1#note")).unwrap(),
            "OPS/注釈 1.xhtml"
        );
        let response = origin.response(&url, "GET", None);
        assert_eq!(response.status, 200);
        assert_eq!(response.mime, "application/xhtml+xml");
        assert!(response.body().starts_with(b"<?xml"));
        let response = origin.response(&origin.url("OPS/%2e%2e.txt").unwrap(), "GET", None);
        assert_eq!(response.body(), b"literal percent filename");
    }
    #[test]
    fn another_origin_traversal_backslashes_and_malformed_urls_never_read_assets() {
        let origin = origin();
        for url in [
            "vobr://another/media.mp4",
            "vobr://book0123/../media.mp4",
            "vobr://book0123/%2e%2e/media.mp4",
            "vobr://book0123/OPS/%5cmedia.mp4",
            "vobr://book0123/media%00.mp4",
            "vobr://book0123/%FF",
            "vobr://book0123/%A",
            "vobr://book0123:80/media.mp4",
            "file:///media.mp4",
        ] {
            assert_eq!(origin.response(url, "GET", None).status, 403, "{url}");
        }
        assert!(origin.url("../media.mp4").is_err());
    }
    #[test]
    fn ranges_head_methods_and_absent_resources_have_honest_responses() {
        let origin = origin();
        let url = origin.url("media.mp4").unwrap();
        for (range, body) in [
            ("bytes=2-5", b"2345".as_slice()),
            ("bytes=7-", b"789"),
            ("bytes=-3", b"789"),
            ("bytes=8-999", b"89"),
        ] {
            let response = origin.response(&url, "GET", Some(range));
            assert_eq!(response.status, 206);
            assert_eq!(response.body(), body);
        }
        for range in [
            "bytes=10-",
            "bytes=5-2",
            "bytes=-0",
            "bytes=0-1,4-5",
            "items=1-2",
        ] {
            assert_eq!(origin.response(&url, "GET", Some(range)).status, 416);
        }
        let head = origin.response(&url, "HEAD", Some("bytes=2-5"));
        assert_eq!(head.status, 200);
        assert_eq!(head.content_length(), 10);
        assert!(head.body().is_empty());
        assert_eq!(origin.response(&url, "POST", None).status, 405);
        assert_eq!(
            origin
                .response("vobr://book0123/missing", "GET", None)
                .status,
            404
        );
    }
    #[test]
    fn mime_types_cover_book_layout_fonts_and_media_and_allow_manifest_override() {
        for (path, mime) in [
            ("plate.SVG", "image/svg+xml"),
            ("book.css", "text/css"),
            ("font.woff2", "font/woff2"),
            ("clip.mp4", "video/mp4"),
            ("voice.m4a", "audio/mp4"),
        ] {
            assert_eq!(infer_mime(path), mime);
        }
        let mut origin = origin();
        origin
            .set_mime_types(BTreeMap::from([("media.mp4".into(), "audio/mp4".into())]))
            .unwrap();
        assert_eq!(
            origin
                .response(&origin.url("media.mp4").unwrap(), "GET", None)
                .mime,
            "audio/mp4"
        );
        assert!(
            origin
                .set_mime_types(BTreeMap::from([(
                    "media.mp4".into(),
                    "audio/mp4\r\nInjected: bad".into()
                )]))
                .is_err()
        );
    }

    #[test]
    fn svgz_is_inflated_for_the_cef_stream_and_plain_assets_share_their_bytes() {
        use std::io::Write;
        let mut book = Book::demo();
        book.resources.clear();
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'><rect width='10' height='10'/></svg>";
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(svg).unwrap();
        book.resources
            .insert("image.svgz".into(), gzip.finish().unwrap());
        book.resources.insert("image.svg".into(), svg.to_vec());
        let origin = BookOrigin::with_token(book, "svgbook".into()).unwrap();
        let response = origin.response(&origin.url("image.svgz").unwrap(), "GET", None);
        assert_eq!(response.mime, "image/svg+xml");
        assert_eq!(response.body(), svg);
        assert!(
            !response
                .headers
                .iter()
                .any(|(key, _)| key == "Content-Encoding")
        );
        let first = origin.response(&origin.url("image.svg").unwrap(), "GET", None);
        let second = origin.response(&origin.url("image.svg").unwrap(), "GET", None);
        assert!(
            Arc::ptr_eq(&first.bytes, &second.bytes),
            "repeated requests must not copy an in-memory asset"
        );
        assert!(
            origin
                .response("vobr://svgbook/missing", "HEAD", None)
                .body()
                .is_empty()
        );
    }
}
