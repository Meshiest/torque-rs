//! The HTTP side of the web transport: a request head read under caps, the path it names normalized, the files served from
//! memory, and the WebSocket upgrade's handshake.
//!
//! Every request is hostile input. The request line is at most [`MAX_REQUEST_LINE`] bytes and the whole head at most
//! [`MAX_HEAD_BYTES`] in at most [`MAX_HEADERS`] headers; only `GET` is served, and a request with a body is refused. A path is
//! percent-decoded once and taken apart into segments: a `.` or `..` segment, a backslash, a control character, a NUL or an
//! invalid escape refuses the request (nothing is resolved, so nothing climbs out). The files are a map in memory
//! ([`WebFiles`], read from a folder when the server starts): a request never touches the disk.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::rc::Rc;

use sha1::{Digest, Sha1};
use torque_resource::plain::{PlainError, at_path, is_link, read_plain};

/// The most bytes of the request line (method, target and version).
pub const MAX_REQUEST_LINE: usize = 2048;
/// The most bytes of a request head (the request line and the headers, with the blank line that ends them).
pub const MAX_HEAD_BYTES: usize = 8192;
/// The most headers a request may have.
pub const MAX_HEADERS: usize = 32;
/// The most bytes of a path once decoded.
pub const MAX_PATH_BYTES: usize = 512;
/// The path a browser opens its game socket on.
pub const WEB_SOCKET_PATH: &str = "/ws";
/// The most files [`WebFiles::from_dir`] reads.
pub const MAX_FILES: usize = 512;
/// The most bytes of all files [`WebFiles::from_dir`] reads.
pub const MAX_FILES_BYTES: u64 = 256 << 20;
/// How deep [`WebFiles::from_dir`] goes into a folder's folders.
pub const MAX_DEPTH: usize = 4;

/// The WebSocket handshake's fixed key suffix (RFC 6455, section 1.3).
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// One file served.
#[derive(Clone)]
pub struct WebFile {
    pub bytes: Rc<[u8]>,
    pub content_type: &'static str,
}

/// The files a web transport serves, by path (`index.html`, `game.js`; `/` serves `index.html`).
#[derive(Clone, Default)]
pub struct WebFiles {
    files: HashMap<String, WebFile>,
}

impl fmt::Debug for WebFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes: usize = self.files.values().map(|w| w.bytes.len()).sum();
        write!(f, "WebFiles({} files, {bytes} bytes)", self.files.len())
    }
}

impl WebFiles {
    pub fn new() -> WebFiles {
        WebFiles::default()
    }

    /// Adds (or replaces) the file `path` (`game.js`, `assets/a.png`; a leading `/` is dropped).
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) {
        let path = path.trim_start_matches('/').to_string();
        let content_type = content_type(&path);
        self.files.insert(
            path,
            WebFile {
                bytes: bytes.into(),
                content_type,
            },
        );
    }

    /// Every regular file under `dir` (at most [`MAX_DEPTH`] folders deep, [`MAX_FILES`] files and [`MAX_FILES_BYTES`] bytes),
    /// read once. A name with characters other than letters, digits, `.`, `_` and `-` (or starting with `.`) is left out, as it
    /// is never served. A link of any kind under a served name (on Windows any reparse point, so a cloud file placeholder too)
    /// fails the load, naming the path, as it does for a game folder.
    pub fn from_dir(dir: &Path) -> io::Result<WebFiles> {
        let mut files = WebFiles::new();
        let mut total = 0u64;
        read_dir(dir, "", 0, &mut files, &mut total)?;
        if files.files.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no files to serve in {}", dir.display()),
            ));
        }
        Ok(files)
    }

    /// The file a normalized path names ([`normalize_path`]): `/` is `index.html`.
    pub fn get(&self, path: &str) -> Option<&WebFile> {
        let path = path.trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };
        self.files.get(path)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn read_dir(
    dir: &Path,
    prefix: &str,
    depth: usize,
    files: &mut WebFiles,
    total: &mut u64,
) -> io::Result<()> {
    let too_many = || {
        io::Error::other(format!(
            "more than {MAX_FILES} files or {MAX_FILES_BYTES} bytes to serve"
        ))
    };
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !safe_name(&name) {
            continue;
        }
        let on_disk = entry.path();
        // `DirEntry::metadata` does not follow a link.
        let meta = entry.metadata().map_err(|e| at_path(&on_disk, e))?;
        if is_link(&meta) {
            return Err(at_path(&on_disk, PlainError::NotPlain.into()));
        }
        let path = format!("{prefix}{name}");
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                read_dir(&on_disk, &format!("{path}/"), depth + 1, files, total)?;
            }
        } else if meta.is_file() {
            let len = meta.len();
            *total = total.saturating_add(len);
            if files.len() >= MAX_FILES || *total > MAX_FILES_BYTES {
                return Err(too_many());
            }
            let bytes = read_plain(&on_disk, MAX_FILES_BYTES, Some(len))
                .map_err(|e| at_path(&on_disk, e.into()))?;
            files.insert(&path, bytes);
        }
    }
    Ok(())
}

/// Whether a file or folder name is served: letters, digits, `.`, `_` and `-`, not starting with `.`.
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The content type a file is served with, by its extension.
pub fn content_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// An HTTP status the transport answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    SwitchingProtocols,
    BadRequest,
    NotFound,
    MethodNotAllowed,
    UriTooLong,
    HeadersTooLarge,
    TooManyRequests,
    VersionNotSupported,
    Forbidden,
    MisdirectedRequest,
}

impl Status {
    pub fn code(self) -> u16 {
        match self {
            Status::Ok => 200,
            Status::SwitchingProtocols => 101,
            Status::BadRequest => 400,
            Status::NotFound => 404,
            Status::MethodNotAllowed => 405,
            Status::UriTooLong => 414,
            Status::HeadersTooLarge => 431,
            Status::TooManyRequests => 429,
            Status::VersionNotSupported => 505,
            Status::Forbidden => 403,
            Status::MisdirectedRequest => 421,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::SwitchingProtocols => "Switching Protocols",
            Status::BadRequest => "Bad Request",
            Status::NotFound => "Not Found",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::UriTooLong => "URI Too Long",
            Status::HeadersTooLarge => "Request Header Fields Too Large",
            Status::TooManyRequests => "Too Many Requests",
            Status::VersionNotSupported => "HTTP Version Not Supported",
            Status::Forbidden => "Forbidden",
            Status::MisdirectedRequest => "Misdirected Request",
        }
    }
}

/// A request head that passed the caps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The normalized path (`/`, `/game.js`).
    pub path: String,
    /// Whether the connection stays open for another request (HTTP/1.1 without `Connection: close`).
    pub keep_alive: bool,
    /// The WebSocket handshake's key, when the request asks for the upgrade properly (`GET /ws`, `Upgrade: websocket`,
    /// `Connection: Upgrade`, version 13, a key of 16 bytes).
    pub upgrade: Option<String>,
    /// Whether the request asks for an upgrade at all (a broken one is refused).
    pub wants_upgrade: bool,
    /// The bytes of the head (what follows is the next request, or the socket's first frames).
    pub head_len: usize,
    /// The `Host` header (`name` or `name:port`), if there is one.
    pub host: Option<String>,
    /// The `Origin` header (a browser's page asking: `http://name:port`), if there is one.
    pub origin: Option<String>,
    /// The protocol's minor version (`1` for HTTP/1.1).
    pub version: u8,
}

/// Which pages may open game sockets (their `Origin`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Origins {
    /// The server's own page: an origin whose scheme is the server's (`https` under TLS, else `http`) and whose host and
    /// port are the request's `Host`.
    #[default]
    SameHost,
    /// These origins only (`https://games.example:8443`, compared without case and a trailing `/`): a page served by
    /// another web server.
    List(Vec<String>),
    /// Any page.
    Any,
}

/// Who may reach a web transport ([`check_access`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// The names a request's `Host` may carry besides IP addresses and `localhost` (`games.example`).
    pub hosts: Vec<String>,
    /// The pages that may open game sockets.
    pub origins: Origins,
}

/// Whether `request` may be answered under `access`: its `Host` (required by HTTP/1.1) must name an IP address,
/// `localhost` or one of [`Access::hosts`], else 421 (a name pointed at this server's address by its owner's DNS does not
/// reach it); a game socket asked for with an `Origin` must come from an origin [`Access::origins`] allows, else 403. A
/// request with no `Origin` is not a browser page's and is let through.
pub fn check_access(request: &Request, access: &Access, tls: bool) -> Result<(), Status> {
    let Some(host) = request.host.as_deref() else {
        return if request.version >= 1 {
            Err(Status::BadRequest)
        } else if request.upgrade.is_some() && request.origin.is_some() {
            Err(Status::Forbidden)
        } else {
            Ok(())
        };
    };
    let name = host_name(host).ok_or(Status::BadRequest)?;
    let known = name.parse::<std::net::IpAddr>().is_ok()
        || name.eq_ignore_ascii_case("localhost")
        || access.hosts.iter().any(|h| h.eq_ignore_ascii_case(name));
    if !known {
        return Err(Status::MisdirectedRequest);
    }
    if !request.wants_upgrade {
        return Ok(());
    }
    let Some(origin) = request.origin.as_deref() else {
        return Ok(());
    };
    let origin = origin.trim().trim_end_matches('/');
    let allowed = match &access.origins {
        Origins::Any => true,
        Origins::List(list) => list
            .iter()
            .any(|o| o.trim().trim_end_matches('/').eq_ignore_ascii_case(origin)),
        Origins::SameHost => {
            let scheme = if tls { "https" } else { "http" };
            origin.eq_ignore_ascii_case(&format!("{scheme}://{host}"))
        }
    };
    if allowed {
        Ok(())
    } else {
        Err(Status::Forbidden)
    }
}

/// The name of a `Host` value (`name`, `name:port`, `[v6]` or `[v6]:port`); None when it is malformed.
fn host_name(host: &str) -> Option<&str> {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        if !(after.is_empty()
            || after
                .strip_prefix(':')
                .is_some_and(|p| p.parse::<u16>().is_ok()))
        {
            return None;
        }
        return Some(inside);
    }
    let (name, port) = match host.split_once(':') {
        Some((n, p)) => (n, Some(p)),
        None => (host, None),
    };
    let name_ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    (name_ok && port.is_none_or(|p| p.parse::<u16>().is_ok())).then_some(name)
}

/// What a buffer of request bytes holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Not a whole head yet (and within the caps).
    Partial,
    Request(Request),
    /// Refused with this status; the connection closes after it.
    Refuse(Status),
}

/// Reads a request head from `buf` under the caps (module docs).
pub fn parse_head(buf: &[u8]) -> Parsed {
    let line_end = buf.windows(2).position(|w| w == b"\r\n");
    match line_end {
        Some(end) if end > MAX_REQUEST_LINE => return Parsed::Refuse(Status::UriTooLong),
        None if buf.len() > MAX_REQUEST_LINE => return Parsed::Refuse(Status::UriTooLong),
        _ => {}
    }
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    let head_len = match req.parse(buf) {
        Ok(httparse::Status::Complete(n)) if n <= MAX_HEAD_BYTES => n,
        Ok(httparse::Status::Complete(_)) => return Parsed::Refuse(Status::HeadersTooLarge),
        Ok(httparse::Status::Partial) if buf.len() >= MAX_HEAD_BYTES => {
            return Parsed::Refuse(Status::HeadersTooLarge);
        }
        Ok(httparse::Status::Partial) => return Parsed::Partial,
        Err(httparse::Error::TooManyHeaders) => return Parsed::Refuse(Status::HeadersTooLarge),
        Err(_) => return Parsed::Refuse(Status::BadRequest),
    };
    if req.method != Some("GET") {
        return Parsed::Refuse(Status::MethodNotAllowed);
    }
    let version = req.version.unwrap_or(0);
    if version > 1 {
        return Parsed::Refuse(Status::VersionNotSupported);
    }
    let Some(path) = req.path.and_then(normalize_path) else {
        return Parsed::Refuse(Status::BadRequest);
    };
    let headers: &[httparse::Header<'_>] = req.headers;
    let header = |name: &'static str| header_values(headers, name);
    // A GET carries no body: a length or a transfer coding is refused rather than read past.
    if header("transfer-encoding").next().is_some()
        || header("content-length").any(|v| v.trim() != "0")
    {
        return Parsed::Refuse(Status::BadRequest);
    }
    let tokens = |name: &'static str, token: &str| {
        header(name).any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
    };
    let close = tokens("connection", "close");
    let keep_alive = version == 1 && !close;
    let wants_upgrade =
        header("upgrade").next().is_some() || header("sec-websocket-key").next().is_some();
    let upgrade = (wants_upgrade
        && version == 1
        && path == WEB_SOCKET_PATH
        && tokens("upgrade", "websocket")
        && tokens("connection", "upgrade")
        && header("sec-websocket-version").any(|v| v.trim() == "13"))
    .then(|| {
        let keys: Vec<&str> = header("sec-websocket-key").collect();
        match keys.as_slice() {
            [key] if valid_key(key.trim()) => Some(key.trim().to_string()),
            _ => None,
        }
    })
    .flatten();
    let single = |name: &'static str| -> Result<Option<String>, ()> {
        let mut values = header(name);
        let first = values.next().map(str::to_string);
        if values.next().is_some() {
            return Err(());
        }
        Ok(first)
    };
    let (Ok(host), Ok(origin)) = (single("host"), single("origin")) else {
        return Parsed::Refuse(Status::BadRequest);
    };
    Parsed::Request(Request {
        path,
        keep_alive,
        upgrade,
        wants_upgrade,
        head_len,
        host,
        origin,
        version,
    })
}

/// The values of the headers named `name` (any case); a value that is not UTF-8 reads as empty.
fn header_values<'a>(
    headers: &'a [httparse::Header<'a>],
    name: &'static str,
) -> impl Iterator<Item = &'a str> + 'a {
    headers
        .iter()
        .filter(move |h| h.name.eq_ignore_ascii_case(name))
        .map(|h| std::str::from_utf8(h.value).unwrap_or(""))
}

/// Whether a `Sec-WebSocket-Key` is base64 of 16 bytes (24 characters, two of them padding).
fn valid_key(key: &str) -> bool {
    key.len() == 24
        && key.ends_with("==")
        && key[..22]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// The path a request target names: the query and fragment dropped, percent escapes decoded once, empty segments dropped;
/// None for anything else than an absolute path of safe segments (module docs). `/` stays `/`.
pub fn normalize_path(target: &str) -> Option<String> {
    let raw = target.split(['?', '#']).next().unwrap_or("");
    if !raw.starts_with('/') {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            decoded.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            decoded.push(b);
            i += 1;
        }
        if decoded.len() > MAX_PATH_BYTES {
            return None;
        }
    }
    let text = String::from_utf8(decoded).ok()?;
    if text.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    let mut out = String::new();
    for segment in text.split('/').filter(|s| !s.is_empty()) {
        if segment == "." || segment == ".." || segment.contains(':') {
            return None;
        }
        out.push('/');
        out.push_str(segment);
    }
    if out.is_empty() {
        out.push('/');
    }
    Some(out)
}

/// The `Sec-WebSocket-Accept` value for a handshake's key (RFC 6455, section 4.2.2).
pub fn accept_key(key: &str) -> String {
    let mut sha = Sha1::new();
    sha.update(key.as_bytes());
    sha.update(WS_GUID.as_bytes());
    base64(&sha.finalize())
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A response head: the status line, the content type and length, no caching, no sniffing, and whether the connection
/// stays open.
pub fn response_head(status: Status, content_type: &str, len: usize, keep_alive: bool) -> Vec<u8> {
    let connection = if keep_alive { "keep-alive" } else { "close" };
    let allow = if status == Status::MethodNotAllowed {
        "Allow: GET\r\n"
    } else {
        ""
    };
    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nCache-Control: no-cache\r\n\
         X-Content-Type-Options: nosniff\r\n{allow}Connection: {connection}\r\n\r\n",
        status.code(),
        status.reason()
    )
    .into_bytes()
}

/// The answer to an upgrade request with `key`.
pub fn upgrade_head(key: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    )
    .into_bytes()
}

/// A refusal: the status as a short text body, the connection closing after it.
pub fn refusal(status: Status) -> Vec<u8> {
    let body = format!("{} {}\n", status.code(), status.reason());
    let mut out = response_head(status, "text/plain; charset=utf-8", body.len(), false);
    out.extend_from_slice(body.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> Parsed {
        parse_head(text.as_bytes())
    }

    #[test]
    fn the_handshake_key_is_the_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn paths_are_normalized_and_never_climb() {
        assert_eq!(normalize_path("/").as_deref(), Some("/"));
        assert_eq!(normalize_path("//game.js?x=1").as_deref(), Some("/game.js"));
        assert_eq!(normalize_path("/a%2Fb").as_deref(), Some("/a/b"));
        for bad in [
            "/../secret",
            "/a/../../b",
            "/%2e%2e/x",
            "/a/./b",
            "/a\\b",
            "/a%5cb",
            "/a%00b",
            "/a%zz",
            "/a%2",
            "game.js",
            "http://x/y",
            "/c:/windows",
        ] {
            assert_eq!(normalize_path(bad), None, "{bad}");
        }
        assert_eq!(normalize_path(&format!("/{}", "a".repeat(600))), None);
    }

    #[test]
    fn only_a_plain_get_is_taken() {
        let Parsed::Request(r) = request("GET /game.js HTTP/1.1\r\nHost: x\r\n\r\nGET") else {
            panic!("a GET");
        };
        assert_eq!(r.path, "/game.js");
        assert!(r.keep_alive);
        assert_eq!(r.head_len, 34);
        assert_eq!(request("GET / HTTP/1.1\r\nHost: x\r\n"), Parsed::Partial);
        assert_eq!(
            request("POST / HTTP/1.1\r\n\r\n"),
            Parsed::Refuse(Status::MethodNotAllowed)
        );
        assert_eq!(
            request("HEAD / HTTP/1.1\r\n\r\n"),
            Parsed::Refuse(Status::MethodNotAllowed)
        );
        assert_eq!(
            request("GET / HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"),
            Parsed::Refuse(Status::BadRequest)
        );
        assert_eq!(
            request("GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Parsed::Refuse(Status::BadRequest)
        );
        assert_eq!(
            request("GET /../x HTTP/1.1\r\n\r\n"),
            Parsed::Refuse(Status::BadRequest)
        );
        assert_eq!(
            request("\x16\x03\x01junk\r\n\r\n"),
            Parsed::Refuse(Status::BadRequest)
        );
        let Parsed::Request(r) = request("GET / HTTP/1.0\r\n\r\n") else {
            panic!("HTTP/1.0");
        };
        assert!(!r.keep_alive);
    }

    #[test]
    fn heads_past_the_caps_are_refused() {
        let long = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(MAX_REQUEST_LINE));
        assert_eq!(request(&long), Parsed::Refuse(Status::UriTooLong));
        let unended = "a".repeat(MAX_REQUEST_LINE + 1);
        assert_eq!(request(&unended), Parsed::Refuse(Status::UriTooLong));
        let many: String = (0..MAX_HEADERS + 1)
            .map(|i| format!("X-{i}: y\r\n"))
            .collect();
        assert_eq!(
            request(&format!("GET / HTTP/1.1\r\n{many}\r\n")),
            Parsed::Refuse(Status::HeadersTooLarge)
        );
        let big = format!("GET / HTTP/1.1\r\nX: {}\r\n", "y".repeat(MAX_HEAD_BYTES));
        assert_eq!(request(&big), Parsed::Refuse(Status::HeadersTooLarge));
    }

    #[test]
    fn an_upgrade_needs_every_part() {
        let good = "GET /ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\n\
                    Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let Parsed::Request(r) = request(good) else {
            panic!("an upgrade");
        };
        assert_eq!(r.upgrade.as_deref(), Some("dGhlIHNhbXBsZSBub25jZQ=="));
        for (from, to) in [
            ("/ws", "/other"),
            ("websocket", "h2c"),
            ("13", "8"),
            ("dGhlIHNhbXBsZSBub25jZQ==", "short=="),
            ("keep-alive, Upgrade", "keep-alive"),
        ] {
            let Parsed::Request(r) = request(&good.replace(from, to)) else {
                panic!("a request");
            };
            assert!(r.wants_upgrade && r.upgrade.is_none(), "{to}");
        }
    }

    #[test]
    fn hosts_must_be_known_and_well_formed() {
        let access = Access {
            hosts: vec!["Games.Example".to_string()],
            origins: Origins::SameHost,
        };
        let check = |head: &str| {
            let Parsed::Request(r) = request(head) else {
                panic!("a request: {head}");
            };
            check_access(&r, &access, false)
        };
        assert_eq!(
            check("GET / HTTP/1.1\r\nHost: games.example:80\r\n\r\n"),
            Ok(())
        );
        assert_eq!(check("GET / HTTP/1.1\r\nHost: [::1]:8080\r\n\r\n"), Ok(()));
        assert_eq!(check("GET / HTTP/1.1\r\nHost: 10.0.0.2\r\n\r\n"), Ok(()));
        assert_eq!(check("GET / HTTP/1.0\r\n\r\n"), Ok(()));
        assert_eq!(check("GET / HTTP/1.1\r\n\r\n"), Err(Status::BadRequest));
        assert_eq!(
            check("GET / HTTP/1.1\r\nHost: a.example\r\n\r\n"),
            Err(Status::MisdirectedRequest)
        );
        for bad in ["a b", "x:99999", "[::1", "x/y", ""] {
            assert_eq!(
                check(&format!("GET / HTTP/1.1\r\nHost: {bad}\r\n\r\n")),
                Err(Status::BadRequest),
                "{bad}"
            );
        }
        assert_eq!(
            request("GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"),
            Parsed::Refuse(Status::BadRequest)
        );
    }

    #[test]
    fn files_are_found_by_path() {
        let mut files = WebFiles::new();
        files.insert("index.html", b"<p>".to_vec());
        files.insert("/game_bg.wasm", vec![0, 97, 115, 109]);
        assert_eq!(&*files.get("/").unwrap().bytes, b"<p>");
        assert_eq!(
            files.get("/game_bg.wasm").unwrap().content_type,
            "application/wasm"
        );
        assert!(files.get("/missing").is_none());
        assert!(safe_name("game_bg.wasm") && !safe_name(".git") && !safe_name("a b"));
    }
}
