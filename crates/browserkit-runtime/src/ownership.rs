//! Internal ownership model for future compositor input routing.

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HitTestTarget {
    Page,
    Chrome,
    None,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusOwner {
    Page,
    Chrome,
    None,
}

// Cursor ownership will be arbitrated by BrowserKit between native surfaces.
// IME follows FocusOwner's native surface; do not synthesize IME events here.
// Drag/drop routing remains platform-specific and unimplemented.
