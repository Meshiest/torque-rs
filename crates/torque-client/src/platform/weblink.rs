//! The windowed client's URL opener: an http(s) URL that `gotoWebPage` passed (the host has checked it: a plain host, no quote or
//! space, at most 511 bytes) is handed to the system's URL handler API: on Windows `ShellExecuteExW` (the `open` crate's
//! `shellexecute-on-windows` path, no command line). Elsewhere nothing is opened. Never used by a test (they run headless, with no
//! opener).

/// Opens `url` in the system's browser.
pub fn open_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http and https are opened".to_string());
    }
    #[cfg(windows)]
    {
        open::that_detached(url).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("the engine opens a browser on Windows only".to_string())
    }
}
