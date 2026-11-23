//! Serving a web build embedded at compile time by `ui-build` (`ui::embedded!()`).

use crate::webview::{Request, Response};

/// A web build (`dist/`) compiled into the binary, served from a custom scheme.
///
/// Made by [`embedded!`](crate::embedded) from the table `ui_build::web(..)`
/// generates in `build.rs`:
///
/// ```ignore
/// // build.rs
/// fn main() { ui_build::web("dist").before("npm run build").emit(); }
///
/// // main.rs
/// WebView::new().bundle("app", ui::embedded!().spa("index.html").immutable("assets/"))
/// ```
///
/// - `/` serves `index.html` (or the `spa` page) and the URL stays `/`.
/// - With [`spa`](Self::spa), unknown paths that don't look like files serve the
///   SPA page, so client-side routes survive a reload.
/// - Files under an [`immutable`](Self::immutable) prefix (hashed bundler output)
///   get `Cache-Control: public, max-age=31536000, immutable`; everything else
///   gets `no-cache`.
#[derive(Debug, Clone)]
pub struct Embedded {
    files: &'static [(&'static str, &'static [u8])],
    spa: Option<String>,
    immutable: Vec<String>,
}

impl Embedded {
    /// Wraps a generated file table of `(relative path, contents)` pairs.
    pub const fn new(files: &'static [(&'static str, &'static [u8])]) -> Self {
        Self {
            files,
            spa: None,
            immutable: Vec::new(),
        }
    }

    /// Serves `page` for unknown non-file paths (single-page app routing).
    pub fn spa(mut self, page: impl Into<String>) -> Self {
        self.spa = Some(trim_slashes(&page.into()).to_owned());
        self
    }

    /// Marks files under `prefix` (e.g. `"assets/"`) as content-hashed and cacheable forever.
    pub fn immutable(mut self, prefix: impl Into<String>) -> Self {
        let p = trim_slashes(&prefix.into()).to_owned();
        self.immutable.push(if p.is_empty() { p } else { format!("{p}/") });
        self
    }

    /// The embedded paths, relative to the web build's root.
    pub fn paths(&self) -> impl Iterator<Item = &'static str> {
        self.files.iter().map(|(p, _)| *p)
    }

    /// The contents of one embedded file.
    pub fn get(&self, path: &str) -> Option<&'static [u8]> {
        let path = trim_slashes(path);
        self.files.iter().find(|(p, _)| *p == path).map(|(_, b)| *b)
    }

    /// Answers one request; this is what `.bundle(scheme, embedded)` calls.
    pub fn serve(&self, req: &Request) -> Response {
        let decoded = percent_decode(req.path());
        let path = trim_slashes(&decoded);
        let index = self.spa.as_deref().unwrap_or("index.html");
        let path = if path.is_empty() { index } else { path };

        if let Some(bytes) = self.get(path) {
            return self.respond(path, bytes);
        }
        let under_immutable = self.immutable.iter().any(|p| path.starts_with(p.as_str()));
        if let Some(spa) = &self.spa
            && !looks_like_file(path)
            && !under_immutable
            && let Some(bytes) = self.get(spa)
        {
            return self.respond(spa, bytes);
        }
        Response::not_found()
    }

    fn respond(&self, path: &str, bytes: &'static [u8]) -> Response {
        let cache = if self.immutable.iter().any(|p| path.starts_with(p.as_str())) {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        Response::asset(path, bytes).with_header("Cache-Control", cache)
    }
}

fn trim_slashes(s: &str) -> &str {
    s.trim_matches('/')
}

fn looks_like_file(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|last| last.contains('.'))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Serves the web build that `ui_build::web(..).emit()` embedded from `build.rs`.
///
/// Expands to an [`Embedded`]; pass it to `WebView::bundle`.
#[macro_export]
macro_rules! embedded {
    () => {
        $crate::embed::Embedded::new(::std::include!(::std::concat!(
            ::std::env!("OUT_DIR"),
            "/ui_embedded.rs"
        )))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    static FILES: &[(&str, &[u8])] = &[
        ("index.html", b"<html>home</html>"),
        ("assets/app-1a2b.js", b"export {}"),
        ("docs/My File.txt", b"hi"),
    ];

    fn get(e: &Embedded, url: &str) -> Response {
        e.serve(&Request::new(url, "GET"))
    }

    #[test]
    fn serves_root_files_and_cache_headers() {
        let e = Embedded::new(FILES).spa("index.html").immutable("assets/");
        let root = get(&e, "app://localhost/");
        assert_eq!(root.status, 200);
        assert_eq!(root.body, b"<html>home</html>".to_vec());
        assert_eq!(root.headers.get("Cache-Control").map(String::as_str), Some("no-cache"));

        let js = get(&e, "app://localhost/assets/app-1a2b.js?v=1");
        assert_eq!(js.status, 200);
        assert!(js.mime().contains("javascript"), "{}", js.mime());
        assert_eq!(
            js.headers.get("Cache-Control").map(String::as_str),
            Some("public, max-age=31536000, immutable")
        );
        assert_eq!(get(&e, "app://localhost/docs/My%20File.txt").body, b"hi".to_vec());
    }

    #[test]
    fn spa_fallback_only_for_routes() {
        let e = Embedded::new(FILES).spa("/index.html/").immutable("/assets");
        assert_eq!(get(&e, "app://localhost/settings/profile").body, b"<html>home</html>".to_vec());
        // Missing files and anything under an asset folder stay 404.
        assert_eq!(get(&e, "app://localhost/missing.png").status, 404);
        assert_eq!(get(&e, "app://localhost/assets/gone").status, 404);

        let plain = Embedded::new(FILES);
        assert_eq!(get(&plain, "app://localhost/").status, 200);
        assert_eq!(get(&plain, "app://localhost/settings").status, 404);
    }
}
