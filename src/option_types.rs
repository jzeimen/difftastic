use crate::display::style::BackgroundColor;
use std::{ffi::OsStr, path::PathBuf};

// Chosen experimentally: this is sufficiently many for all the sample
// files (the highest is slow_before/after.rs at 1.3M nodes), but
// small enough to terminate in ~5 seconds like the test file in #306.
pub const DEFAULT_GRAPH_LIMIT: usize = 3_000_000;

#[derive(Debug, Copy, Clone)]
pub enum DisplayMode {
    Inline,
    SideBySide,
    SideBySideShowBoth,
}

#[derive(Debug, Clone)]
pub struct DisplayOptions {
    pub background_color: BackgroundColor,
    pub use_color: bool,
    pub display_mode: DisplayMode,
    pub print_unchanged: bool,
    pub tab_width: usize,
    pub display_width: usize,
    pub num_context_lines: u32,
    pub in_vcs: bool,
    pub syntax_highlight: bool,
}

#[derive(Eq, PartialEq, Debug)]
pub enum FileArgument {
    NamedPath(std::path::PathBuf),
    Stdin,
    DevNull,
}

impl FileArgument {
    /// Return a `FileArgument` representing this command line
    /// argument.
    pub fn from_cli_argument(arg: &OsStr) -> Self {
        if arg == "/dev/null" {
            FileArgument::DevNull
        } else if arg == "-" {
            FileArgument::Stdin
        } else {
            FileArgument::NamedPath(PathBuf::from(arg))
        }
    }

    /// Return a `FileArgument` that always represents a path that
    /// exists.
    pub fn from_path_argument(arg: &OsStr) -> Self {
        FileArgument::NamedPath(PathBuf::from(arg))
    }

    pub fn display(&self) -> String {
        match self {
            FileArgument::NamedPath(path) => path.display().to_string(),
            FileArgument::Stdin => "(stdin)".to_string(),
            FileArgument::DevNull => "/dev/null".to_string(),
        }
    }
}
