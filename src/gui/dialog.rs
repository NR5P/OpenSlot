//! File dialogs, refused while the screen is being drawn for a remote: a native dialog would
//! open on the station's monitor, where nobody may be, and hold its window until closed.

use std::cell::Cell;
use std::path::PathBuf;

thread_local! {
    /// Drawing the screen for a remote now (on this thread)
    static FOR_REMOTE: Cell<bool> = const { Cell::new(false) };
    /// A dialog was asked for from a remote since the last take_refused()
    static REFUSED: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` (a remote's pass of the screen) with file dialogs refused.
pub fn for_remote<T>(f: impl FnOnce() -> T) -> T {
    FOR_REMOTE.with(|r| r.set(true));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FOR_REMOTE.with(|r| r.set(false));
        }
    }
    let _reset = Reset;
    f()
}

/// Whether a remote asked for a file dialog (and was refused) since the last call.
pub fn take_refused() -> bool {
    REFUSED.with(|r| r.replace(false))
}

fn refused() -> bool {
    let remote = FOR_REMOTE.with(|r| r.get());
    if remote {
        REFUSED.with(|r| r.set(true));
    }
    remote
}

/// rfd's FileDialog, as the app uses it.
#[derive(Default)]
pub struct FileDialog(rfd::FileDialog);

impl FileDialog {
    pub fn new() -> Self {
        Self(rfd::FileDialog::new())
    }

    pub fn add_filter(self, name: impl Into<String>, extensions: &[impl ToString]) -> Self {
        Self(self.0.add_filter(name, extensions))
    }

    pub fn set_file_name(self, name: impl Into<String>) -> Self {
        Self(self.0.set_file_name(name))
    }

    pub fn pick_file(self) -> Option<PathBuf> {
        if refused() { None } else { self.0.pick_file() }
    }

    pub fn pick_files(self) -> Option<Vec<PathBuf>> {
        if refused() { None } else { self.0.pick_files() }
    }

    pub fn pick_folder(self) -> Option<PathBuf> {
        if refused() { None } else { self.0.pick_folder() }
    }

    pub fn save_file(self) -> Option<PathBuf> {
        if refused() { None } else { self.0.save_file() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_only_for_a_remote() {
        assert!(!take_refused());
        assert_eq!(for_remote(|| FileDialog::new().add_filter("Images", &["png"]).pick_file()), None);
        assert!(take_refused(), "the station is told");
        assert!(!take_refused());
        assert!(!FOR_REMOTE.with(|r| r.get()), "back to normal after the pass");
    }
}
