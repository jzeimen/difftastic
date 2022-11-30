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

mod constants;
pub mod diff;
pub mod display;
pub mod files;
pub mod json;
mod line_parser;
pub mod lines;
pub mod option_types;
pub mod parse;
mod positions;
pub mod summary;

#[macro_use]
extern crate log;

use crate::diff::{dijkstra, unchanged};
use crate::display::hunks::{matched_pos_to_hunks, merge_adjacent};
use crate::option_types::{DisplayMode, DisplayOptions};
use crate::parse::syntax;
use diff::changes::ChangeMap;
use diff::dijkstra::ExceededGraphLimit;
use display::context::opposite_positions;
use files::{guess_content, ProbableFileKind};
use mimalloc::MiMalloc;
use option_types::DEFAULT_GRAPH_LIMIT;
use parse::guess_language::{guess, language_name};

/// The global allocator used by difftastic.
///
/// Diffing allocates a large amount of memory, and `MiMalloc` performs
/// better.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use crate::option_types::FileArgument;
use diff::sliders::fix_all_sliders;
use std::{env, path::Path};
pub use summary::{DiffResult, FileContent};
use syntax::init_next_prev;
use typed_arena::Arena;

use crate::{
    dijkstra::mark_syntax, lines::MaxLine, parse::syntax::init_all_info,
    parse::tree_sitter_parser as tsp,
};

extern crate pretty_env_logger;

pub fn get_file_diffs(lhs: &[u8], rhs: &[u8]) -> String {
    let diff = diff_file_content(
        &"n/a",
        &"n/a",
        &FileArgument::DevNull,
        &FileArgument::DevNull,
        lhs,
        rhs,
        8,
        DEFAULT_GRAPH_LIMIT,
        1_000_000,
        Some(crate::parse::guess_language::Language::Html),
    );
    serde_json::to_string(&json::to_file(diff)).expect("failed to serialize file")
}

pub fn diff_file_content(
    lhs_display_path: &str,
    rhs_display_path: &str,
    _lhs_path: &FileArgument,
    rhs_path: &FileArgument,
    lhs_bytes: &[u8],
    rhs_bytes: &[u8],
    tab_width: usize,
    graph_limit: usize,
    byte_limit: usize,
    language_override: Option<crate::parse::guess_language::Language>,
) -> DiffResult {
    let (mut lhs_src, mut rhs_src) = match (guess_content(lhs_bytes), guess_content(rhs_bytes)) {
        (ProbableFileKind::Binary, _) | (_, ProbableFileKind::Binary) => {
            return DiffResult {
                lhs_display_path: lhs_display_path.into(),
                rhs_display_path: rhs_display_path.into(),
                language: None,
                detected_language: None,
                lhs_src: FileContent::Binary(lhs_bytes.to_vec()),
                rhs_src: FileContent::Binary(rhs_bytes.to_vec()),
                lhs_positions: vec![],
                rhs_positions: vec![],
            };
        }
        (ProbableFileKind::Text(lhs_src), ProbableFileKind::Text(rhs_src)) => (lhs_src, rhs_src),
    };

    // TODO: don't replace tab characters inside string literals.
    lhs_src = replace_tabs(&lhs_src, tab_width);
    rhs_src = replace_tabs(&rhs_src, tab_width);

    // Ignore the trailing newline, if present.
    // TODO: highlight if this has changes (#144).
    // TODO: factor out a string cleaning function.
    if lhs_src.ends_with('\n') {
        lhs_src.pop();
    }
    if rhs_src.ends_with('\n') {
        rhs_src.pop();
    }

    let (guess_src, guess_path) = match rhs_path {
        FileArgument::NamedPath(_) => (&rhs_src, Path::new(&rhs_display_path)),
        FileArgument::Stdin => (&rhs_src, Path::new(&lhs_display_path)),
        FileArgument::DevNull => (&lhs_src, Path::new(&lhs_display_path)),
    };

    let language = language_override.or_else(|| guess(guess_path, guess_src));
    let lang_config = language.map(tsp::from_language);

    if lhs_bytes == rhs_bytes {
        // If the two files are completely identical, return early
        // rather than doing any more work.
        return DiffResult {
            lhs_display_path: lhs_display_path.into(),
            rhs_display_path: rhs_display_path.into(),
            language: language.map(|l| language_name(l).into()),
            detected_language: language,
            lhs_src: FileContent::Text("".into()),
            rhs_src: FileContent::Text("".into()),
            lhs_positions: vec![],
            rhs_positions: vec![],
        };
    }

    let (lang_name, lhs_positions, rhs_positions) = match lang_config {
        _ if lhs_bytes.len() > byte_limit || rhs_bytes.len() > byte_limit => {
            let lhs_positions = line_parser::change_positions(&lhs_src, &rhs_src);
            let rhs_positions = line_parser::change_positions(&rhs_src, &lhs_src);
            (
                Some("Text (exceeded DFT_BYTE_LIMIT)".into()),
                lhs_positions,
                rhs_positions,
            )
        }
        Some(ts_lang) => {
            let arena = Arena::new();
            let lhs = tsp::parse(&arena, &lhs_src, &ts_lang);
            let rhs = tsp::parse(&arena, &rhs_src, &ts_lang);

            init_all_info(&lhs, &rhs);

            let mut change_map = ChangeMap::default();
            let possibly_changed = if env::var("DFT_DBG_KEEP_UNCHANGED").is_ok() {
                vec![(lhs.clone(), rhs.clone())]
            } else {
                unchanged::mark_unchanged(&lhs, &rhs, &mut change_map)
            };

            let mut exceeded_graph_limit = false;

            for (lhs_section_nodes, rhs_section_nodes) in possibly_changed {
                init_next_prev(&lhs_section_nodes);
                init_next_prev(&rhs_section_nodes);

                match mark_syntax(
                    lhs_section_nodes.get(0).copied(),
                    rhs_section_nodes.get(0).copied(),
                    &mut change_map,
                    graph_limit,
                ) {
                    Ok(()) => {}
                    Err(ExceededGraphLimit {}) => {
                        exceeded_graph_limit = true;
                        break;
                    }
                }
            }

            if exceeded_graph_limit {
                let lhs_positions = line_parser::change_positions(&lhs_src, &rhs_src);
                let rhs_positions = line_parser::change_positions(&rhs_src, &lhs_src);
                (
                    Some("Text (exceeded DFT_GRAPH_LIMIT)".into()),
                    lhs_positions,
                    rhs_positions,
                )
            } else {
                // TODO: Make this .expect() unnecessary.
                let language =
                    language.expect("If we had a ts_lang, we must have guessed the language");
                fix_all_sliders(language, &lhs, &mut change_map);
                fix_all_sliders(language, &rhs, &mut change_map);

                let lhs_positions = parse::syntax::change_positions(&lhs, &change_map);
                let rhs_positions = parse::syntax::change_positions(&rhs, &change_map);
                (
                    Some(language_name(language).into()),
                    lhs_positions,
                    rhs_positions,
                )
            }
        }
        None => {
            let lhs_positions = line_parser::change_positions(&lhs_src, &rhs_src);
            let rhs_positions = line_parser::change_positions(&rhs_src, &lhs_src);
            (None, lhs_positions, rhs_positions)
        }
    };

    DiffResult {
        lhs_display_path: lhs_display_path.into(),
        rhs_display_path: rhs_display_path.into(),
        language: lang_name,
        detected_language: language,
        lhs_src: FileContent::Text(lhs_src),
        rhs_src: FileContent::Text(rhs_src),
        lhs_positions,
        rhs_positions,
    }
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

pub fn print_diff_result(display_options: &DisplayOptions, summary: &DiffResult) {
    match (&summary.lhs_src, &summary.rhs_src) {
        (FileContent::Text(lhs_src), FileContent::Text(rhs_src)) => {
            let opposite_to_lhs = opposite_positions(&summary.lhs_positions);
            let opposite_to_rhs = opposite_positions(&summary.rhs_positions);

            let hunks = matched_pos_to_hunks(&summary.lhs_positions, &summary.rhs_positions);
            let hunks = merge_adjacent(
                &hunks,
                &opposite_to_lhs,
                &opposite_to_rhs,
                lhs_src.max_line(),
                rhs_src.max_line(),
                display_options.num_context_lines as usize,
            );

            let lang_name = summary.language.clone().unwrap_or_else(|| "Text".into());
            if hunks.is_empty() {
                if display_options.print_unchanged {
                    println!(
                        "{}",
                        display::style::header(
                            &summary.lhs_display_path,
                            &summary.rhs_display_path,
                            1,
                            1,
                            &lang_name,
                            display_options
                        )
                    );
                    if lang_name == "Text" || summary.lhs_src == summary.rhs_src {
                        // TODO: there are other Text names now, so
                        // they will hit the second case incorrectly.
                        println!("No changes.\n");
                    } else {
                        println!("No syntactic changes.\n");
                    }
                }
                return;
            }

            match display_options.display_mode {
                DisplayMode::Inline => {
                    display::inline::print(
                        lhs_src,
                        rhs_src,
                        display_options,
                        &summary.lhs_positions,
                        &summary.rhs_positions,
                        &hunks,
                        &summary.lhs_display_path,
                        &summary.rhs_display_path,
                        &lang_name,
                        summary.detected_language,
                    );
                }
                DisplayMode::SideBySide | DisplayMode::SideBySideShowBoth => {
                    display::side_by_side::print(
                        &hunks,
                        display_options,
                        &summary.lhs_display_path,
                        &summary.rhs_display_path,
                        &lang_name,
                        summary.detected_language,
                        lhs_src,
                        rhs_src,
                        &summary.lhs_positions,
                        &summary.rhs_positions,
                    );
                }
            }
        }
        (FileContent::Binary(lhs_bytes), FileContent::Binary(rhs_bytes)) => {
            let changed = lhs_bytes != rhs_bytes;
            if display_options.print_unchanged || changed {
                println!(
                    "{}",
                    display::style::header(
                        &summary.lhs_display_path,
                        &summary.rhs_display_path,
                        1,
                        1,
                        "binary",
                        display_options
                    )
                );
                if changed {
                    println!("Binary contents changed.");
                } else {
                    println!("No changes.");
                }
            }
        }
        (_, FileContent::Binary(_)) | (FileContent::Binary(_), _) => {
            // We're diffing a binary file against a text file.
            println!(
                "{}",
                display::style::header(
                    &summary.lhs_display_path,
                    &summary.rhs_display_path,
                    1,
                    1,
                    "binary",
                    display_options
                )
            );
            println!("Binary contents changed.");
        }
    }
}
