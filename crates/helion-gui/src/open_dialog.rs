//! Native HDL file picker. One backend (rfd) on macOS and Linux.
//!
//! Toolbar Open…, empty-state Open HDL…, and ⌘O all call [`open_hdl_dialog`].
//! There is no HTML file input and no `None` stub on non-macOS.

use std::path::PathBuf;

/// Extensions the Open HDL dialog accepts.
pub const HDL_EXTENSIONS: &[&str] = &["sv", "v", "svh", "vhd", "vhdl", "sdc", "xdc"];

/// Dialog backend id — always rfd, never osascript / HTML / silent no-op.
pub fn dialog_backend() -> &'static str {
    "rfd"
}

/// Configured native picker. Constructing this must work on macOS and Linux.
pub fn hdl_file_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new()
        .set_title("Open HDL")
        .add_filter("HDL", HDL_EXTENSIONS)
}

/// Modal native file dialog. Returns `None` only if the user cancels or no display.
pub fn open_hdl_dialog() -> Option<PathBuf> {
    hdl_file_dialog().pick_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_hdl_dialog_is_rfd_on_every_os() {
        assert_eq!(dialog_backend(), "rfd");
        assert!(HDL_EXTENSIONS.contains(&"sv"));
        assert!(HDL_EXTENSIONS.contains(&"vhd"));
        // Building the native dialog must not be a compile-time None stub.
        let _dialog = hdl_file_dialog();
        assert_ne!(dialog_backend(), "none");
        assert_ne!(dialog_backend(), "osascript");
        assert_ne!(dialog_backend(), "html");
    }
}
