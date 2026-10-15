//! Watches the local clipboard so copied text can be passed to the remote
//! computer, and puts text from the remote computer on the local clipboard.

use std::time::{Duration, Instant};

/// How often to look at the clipboard while the window has focus.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Bigger texts are not passed along.
const MAX_TEXT: usize = 1 << 20;

pub struct ClipboardWatcher {
    clipboard: Option<arboard::Clipboard>,
    /// The text last seen on (or put on) the local clipboard.
    last_text: Option<String>,
    last_check: Option<Instant>,
}

impl ClipboardWatcher {
    pub fn new() -> Self {
        ClipboardWatcher { clipboard: None, last_text: None, last_check: None }
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(err) => {
                    log::warn!("clipboard not available: {err}");
                    return None;
                }
            }
        }
        self.clipboard.as_mut()
    }

    fn read(&mut self) -> Option<String> {
        let text = self.clipboard()?.get_text().ok()?;
        (!text.is_empty() && text.len() <= MAX_TEXT).then_some(text)
    }

    /// Current clipboard text, if any.
    pub fn text(&mut self) -> Option<String> {
        self.read()
    }

    /// Returns new text when the clipboard changed since the last look.
    /// `force` looks right away (e.g. when the window gains focus).
    pub fn poll(&mut self, force: bool) -> Option<String> {
        let now = Instant::now();
        if !force && self.last_check.is_some_and(|t| now.duration_since(t) < POLL_INTERVAL) {
            return None;
        }
        self.last_check = Some(now);
        let text = self.read()?;
        if self.last_text.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last_text = Some(text.clone());
        Some(text)
    }

    /// Put text from the remote computer on the local clipboard.
    pub fn set(&mut self, text: &str) {
        if text.len() > MAX_TEXT {
            return;
        }
        if self.last_text.as_deref() == Some(text) {
            return;
        }
        if let Some(cb) = self.clipboard() {
            match cb.set_text(text.to_owned()) {
                // Remember it, so it is not sent straight back.
                Ok(()) => self.last_text = Some(text.to_owned()),
                Err(err) => log::warn!("could not set the clipboard: {err}"),
            }
        }
    }
}
