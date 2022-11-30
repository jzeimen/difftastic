//! Difftastic is a syntactic diff tool.
//!
//! For usage instructions and advice on contributing, see [the
//! manual](http://difftastic.wilfred.me.uk/).
//!

// This tends to trigger on larger tuples of simple types, and naming
// them would probably be worse for readability.
#![allow(clippy::type_complexity)]
// == "" is often clearer when dealing with strings.
#![allow(clippy::comparison_to_empty)]
// It's common to have pairs foo_lhs and foo_rhs, leading to double
// the number of arguments and triggering this lint.
#![allow(clippy::too_many_arguments)]
// Has false positives on else if chains that sometimes have the same
// body for readability.
#![allow(clippy::if_same_then_else)]
// Purely stylistic, and ignores whether there are explanatory
// comments in the if/else.
#![allow(clippy::bool_to_int_with_if)]

pub mod options;

#[macro_use]
extern crate log;

use difftastic::diff_file_content;

use difftastic::files::{read_files_or_die, read_or_die, relative_paths_in_either};
use difftastic::option_types::{DisplayOptions, FileArgument};
use difftastic::parse::guess_language::LANG_EXTENSIONS;
use difftastic::parse::guess_language::{guess, language_name, Language};
use difftastic::print_diff_result;
use log::info;

use difftastic::summary::DiffResult;
use options::{Mode, DEFAULT_TAB_WIDTH};
use owo_colors::OwoColorize;
use rayon::prelude::*;
use std::path::Path;
use typed_arena::Arena;

use difftastic::{parse::syntax::init_all_info, parse::tree_sitter_parser as tsp};

extern crate pretty_env_logger;

/// Terminate the process if we get SIGPIPE.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {
    // Do nothing.
}

/// The entrypoint.
fn main() {
    pretty_env_logger::init_timed();
    reset_sigpipe();

    match options::parse_args() {
        Mode::DumpTreeSitter {
            path,
            language_override,
        } => {
            let path = Path::new(&path);
            let bytes = read_or_die(path);
            let src = String::from_utf8_lossy(&bytes).to_string();
            // TODO: Load display options rather than hard-coding.
            let src = replace_tabs(&src, DEFAULT_TAB_WIDTH);

            let language = language_override.or_else(|| guess(path, &src));
            match language {
                Some(lang) => {
                    let ts_lang = tsp::from_language(lang);
                    let tree = tsp::parse_to_tree(&src, &ts_lang);
                    tsp::print_tree(&src, &tree);
                }
                None => {
                    eprintln!("No tree-sitter parser for file: {:?}", path);
                }
            }
        }
        Mode::DumpSyntax {
            path,
            language_override,
        } => {
            let path = Path::new(&path);
            let bytes = read_or_die(path);
            let src = String::from_utf8_lossy(&bytes).to_string();
            // TODO: Load display options rather than hard-coding.
            let src = replace_tabs(&src, DEFAULT_TAB_WIDTH);

            let language = language_override.or_else(|| guess(path, &src));
            match language {
                Some(lang) => {
                    let ts_lang = tsp::from_language(lang);
                    let arena = Arena::new();
                    let ast = tsp::parse(&arena, &src, &ts_lang);
                    init_all_info(&ast, &[]);
                    println!("{:#?}", ast);
                }
                None => {
                    eprintln!("No tree-sitter parser for file: {:?}", path);
                }
            }
        }
        Mode::ListLanguages { use_color } => {
            for (language, extensions) in LANG_EXTENSIONS {
                let mut name = language_name(*language).to_string();
                if use_color {
                    name = name.bold().to_string();
                }
                print!("{}", name);

                let mut extensions: Vec<&str> = (*extensions).into();
                extensions.sort_unstable();

                for extension in extensions {
                    print!(" .{}", extension);
                }
                println!();
            }
        }
        Mode::Diff {
            graph_limit,
            byte_limit,
            display_options,
            missing_as_empty,
            language_override,
            lhs_path,
            rhs_path,
            lhs_display_path,
            rhs_display_path,
        } => {
            if lhs_path == rhs_path {
                let is_dir = match &lhs_path {
                    FileArgument::NamedPath(path) => path.is_dir(),
                    _ => false,
                };

                eprintln!(
                    "warning: You've specified the same {} twice.\n",
                    if is_dir { "directory" } else { "file" }
                );
            }

            match (&lhs_path, &rhs_path) {
                (FileArgument::NamedPath(lhs_path), FileArgument::NamedPath(rhs_path))
                    if lhs_path.is_dir() && rhs_path.is_dir() =>
                {
                    diff_directories(
                        &lhs_path,
                        &rhs_path,
                        &display_options,
                        graph_limit,
                        byte_limit,
                        language_override,
                    )
                    .for_each(|diff_result| {
                        print_diff_result(&display_options, &diff_result);
                    });
                }
                _ => {
                    let diff_result = diff_file(
                        &lhs_display_path,
                        &rhs_display_path,
                        &lhs_path,
                        &rhs_path,
                        &display_options,
                        missing_as_empty,
                        graph_limit,
                        byte_limit,
                        language_override,
                    );
                    print_diff_result(&display_options, &diff_result);
                }
            }
        }
    };
}

/// Return a copy of `str` with all the tab characters replaced by
/// `tab_width` strings.
///
/// TODO: This break parsers that require tabs, such as Makefile
/// parsing. We shouldn't do this transform until after parsing.
fn replace_tabs(src: &str, tab_width: usize) -> String {
    let tab_as_spaces = " ".repeat(tab_width);
    src.replace('\t', &tab_as_spaces)
}

/// Print a diff between two files.
fn diff_file(
    lhs_display_path: &str,
    rhs_display_path: &str,
    lhs_path: &FileArgument,
    rhs_path: &FileArgument,
    display_options: &DisplayOptions,
    missing_as_empty: bool,
    graph_limit: usize,
    byte_limit: usize,
    language_override: Option<Language>,
) -> DiffResult {
    let (lhs_bytes, rhs_bytes) = read_files_or_die(lhs_path, rhs_path, missing_as_empty);
    diff_file_content(
        lhs_display_path,
        rhs_display_path,
        lhs_path,
        rhs_path,
        &lhs_bytes,
        &rhs_bytes,
        display_options.tab_width,
        graph_limit,
        byte_limit,
        language_override,
    )
}

/// Given two directories that contain the files, compare them
/// pairwise. Returns an iterator, so we can print results
/// incrementally.
///
/// When more than one file is modified, the hg extdiff extension passes directory
/// paths with the all the modified files.
fn diff_directories<'a>(
    lhs_dir: &'a Path,
    rhs_dir: &'a Path,
    display_options: &DisplayOptions,
    graph_limit: usize,
    byte_limit: usize,
    language_override: Option<Language>,
) -> impl ParallelIterator<Item = DiffResult> + 'a {
    let display_options = display_options.clone();

    // We greedily list all files in the directory, and then diff them
    // in parallel. This is assuming that diffing is slower than
    // enumerating files, so it benefits more from parallelism.
    let paths = relative_paths_in_either(lhs_dir, rhs_dir);

    paths.into_par_iter().map(move |rel_path| {
        info!("Relative path is {:?} inside {:?}", rel_path, lhs_dir);

        let lhs_path = Path::new(lhs_dir).join(&rel_path);
        let rhs_path = Path::new(rhs_dir).join(&rel_path);

        diff_file(
            &rel_path.to_string_lossy(),
            &rel_path.to_string_lossy(),
            &FileArgument::NamedPath(lhs_path),
            &FileArgument::NamedPath(rhs_path),
            &display_options,
            true,
            graph_limit,
            byte_limit,
            language_override,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;
    use crate::options::{DEFAULT_BYTE_LIMIT, DEFAULT_TAB_WIDTH};
    use difftastic::option_types::DEFAULT_GRAPH_LIMIT;

    #[test]
    fn test_diff_identical_content() {
        let s = "foo";
        let res = diff_file_content(
            "foo.el",
            "foo.el",
            &FileArgument::from_path_argument(OsStr::new("foo.el")),
            &FileArgument::from_path_argument(OsStr::new("foo.el")),
            s.as_bytes(),
            s.as_bytes(),
            DEFAULT_TAB_WIDTH,
            DEFAULT_GRAPH_LIMIT,
            DEFAULT_BYTE_LIMIT,
            None,
        );

        assert_eq!(res.lhs_positions, vec![]);
        assert_eq!(res.rhs_positions, vec![]);
    }
}
