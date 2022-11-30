use serde::Serialize;
use std::collections::HashMap;

use crate::{
    display::context::{all_matched_lines_filled, opposite_positions},
    display::hunks::{matched_lines_indexes_for_hunk, matched_pos_to_hunks, merge_adjacent},
    display::side_by_side::lines_with_novel,
    lines::LineNumber,
    syntax::{AtomKind, MatchKind, MatchedPos, TokenKind as SyntaxTokenKind},
    DiffResult, FileContent, MaxLine,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Status {
    Changed,
    Created,
    Deleted,
    Unchanged,
}

#[derive(Debug, Serialize)]
pub struct File {
    language: String,
    path: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    chunks: Vec<Vec<Line>>,
    status: Status,
}

impl File {
    fn with_sections<L, P>(language: L, path: P, chunks: Vec<Vec<Line>>) -> File
    where
        L: Into<String>,
        P: Into<String>,
    {
        File {
            language: language.into(),
            path: path.into(),
            chunks,
            status: Status::Changed,
        }
    }

    fn with_status<L, P>(language: L, path: P, status: Status) -> File
    where
        L: Into<String>,
        P: Into<String>,
    {
        File {
            language: language.into(),
            path: path.into(),
            chunks: Vec::new(),
            status,
        }
    }
}

impl From<DiffResult> for File {
    fn from(summary: DiffResult) -> Self {
        match (summary.lhs_src, summary.rhs_src) {
            (FileContent::Text(lhs_src), FileContent::Text(rhs_src)) => {
                // TODO: move into function as it effectively duplicates lines 365-375 of main::print_diff_result
                let opposite_to_lhs = opposite_positions(&summary.lhs_positions);
                let opposite_to_rhs = opposite_positions(&summary.rhs_positions);

                let hunks = matched_pos_to_hunks(&summary.lhs_positions, &summary.rhs_positions);
                let hunks = merge_adjacent(
                    &hunks,
                    &opposite_to_lhs,
                    &opposite_to_rhs,
                    lhs_src.max_line(),
                    rhs_src.max_line(),
                    3,
                );

                let language = summary.language.clone().unwrap_or_else(|| "Text".into());
                if hunks.is_empty() {
                    return File::with_status(
                        &language,
                        &summary.lhs_display_path,
                        Status::Unchanged,
                    );
                }

                if lhs_src.is_empty() {
                    return File::with_status(
                        &language,
                        &summary.lhs_display_path,
                        Status::Created,
                    );
                }
                if rhs_src.is_empty() {
                    return File::with_status(
                        &language,
                        &summary.rhs_display_path,
                        Status::Deleted,
                    );
                }

                let lhs_lines = lhs_src.split('\n').collect::<Vec<&str>>();
                let rhs_lines = rhs_src.split('\n').collect::<Vec<&str>>();

                let (lhs_lines_with_novel, rhs_lines_with_novel) =
                    lines_with_novel(&summary.lhs_positions, &summary.rhs_positions);

                let matched_lines = all_matched_lines_filled(
                    &summary.lhs_positions,
                    &summary.rhs_positions,
                    &lhs_lines,
                    &rhs_lines,
                );

                let mut chunks = Vec::with_capacity(hunks.len());
                for hunk in hunks.iter() {
                    let mut lines = HashMap::<(Option<u32>, Option<u32>), Line>::with_capacity(
                        hunk.lines.len(),
                    );

                    let (start_i, end_i) = matched_lines_indexes_for_hunk(&matched_lines, hunk, 3);

                    let aligned_lines = matched_lines[start_i..end_i].to_vec();

                    for (lhs_line_num, rhs_line_num) in aligned_lines {
                        if !lhs_lines_with_novel.contains(&lhs_line_num.unwrap_or(LineNumber(0)))
                            && !rhs_lines_with_novel
                                .contains(&rhs_line_num.unwrap_or(LineNumber(0)))
                        {
                            continue;
                        }

                        let line = lines
                            .entry((lhs_line_num.map(|l| l.0), rhs_line_num.map(|l| l.0)))
                            .or_insert_with(|| {
                                Line::new(lhs_line_num.map(|l| l.0), rhs_line_num.map(|l| l.0))
                            });

                        if let Some(line_num) = lhs_line_num {
                            add_changes_to_side(
                                line.lhs.as_mut().unwrap(),
                                line_num,
                                &lhs_lines,
                                &summary.lhs_positions,
                            );
                        }
                        if let Some(line_num) = rhs_line_num {
                            add_changes_to_side(
                                line.rhs.as_mut().unwrap(),
                                line_num,
                                &rhs_lines,
                                &summary.rhs_positions,
                            );
                        }
                    }

                    chunks.push(lines.into_values().collect());
                }

                File::with_sections(&language, &summary.lhs_display_path, chunks)
            }
            (FileContent::Binary(lhs_byes), FileContent::Binary(rhs_bytes)) => File::with_status(
                "binary",
                &summary.lhs_display_path,
                if lhs_byes == rhs_bytes {
                    Status::Unchanged
                } else {
                    Status::Changed
                },
            ),
            (_, FileContent::Binary(_)) | (FileContent::Binary(_), _) => {
                File::with_status("binary", &summary.lhs_display_path, Status::Changed)
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct Line {
    #[serde(skip_serializing_if = "Option::is_none")]
    lhs: Option<Side>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rhs: Option<Side>,
}

impl Line {
    fn new(lhs_number: Option<u32>, rhs_number: Option<u32>) -> Line {
        Line {
            lhs: lhs_number.map(Side::new),
            rhs: rhs_number.map(Side::new),
        }
    }
}

#[derive(Debug, Serialize)]
struct Side {
    number: u32,
    changes: Vec<Change>,
}

impl Side {
    fn new(number: u32) -> Side {
        Side {
            number,
            changes: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct Change {
    start: u32,
    end: u32,
    content: String,
    kind: TokenKind,
    match_kind: JsonMatchKind,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
// TODO: use syntax::TokenKind and syntax::AtomKind instead of this merged enum, blocked by https://github.com/serde-rs/serde/issues/1402
enum TokenKind {
    Delimiter,
    Normal,
    String,
    Type,
    Comment,
    Keyword,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
// TODO: use syntax::TokenKind and syntax::AtomKind instead of this merged enum, blocked by https://github.com/serde-rs/serde/issues/1402
enum JsonMatchKind {
    UnchangedToken,
    Novel,
    NovelLinePart,
    NovelWord,
}

impl From<SyntaxTokenKind> for TokenKind {
    fn from(kind: SyntaxTokenKind) -> Self {
        match kind {
            SyntaxTokenKind::Delimiter => TokenKind::Delimiter,
            SyntaxTokenKind::Atom(a) => match a {
                AtomKind::String => TokenKind::String,
                AtomKind::Keyword => TokenKind::Keyword,
                AtomKind::Comment => TokenKind::Comment,
                AtomKind::Type => TokenKind::Type,
                AtomKind::Normal => TokenKind::Normal,
                AtomKind::TreeSitterError => todo!(),
            },
        }
    }
}

pub fn print_directory(diffs: Vec<DiffResult>) {
    let files = diffs
        .into_iter()
        .map(DiffResult::into)
        .collect::<Vec<File>>();
    println!(
        "{}",
        serde_json::to_string(&files).expect("failed to serialize files")
    );
}

pub fn print(diff: DiffResult) {
    let file: File = diff.into();
    println!(
        "{}",
        serde_json::to_string(&file).expect("failed to serialize file")
    );
}

pub fn to_file(diff: DiffResult) -> File {
    diff.into()
}

fn matches_for_line(matches: &[MatchedPos], line_num: LineNumber) -> Vec<&MatchedPos> {
    matches
        .iter()
        .filter(|m| m.pos.line == line_num)
        .filter(|m| m.kind.is_novel())
        .collect()
}

fn add_changes_to_side(
    side: &mut Side,
    line_num: LineNumber,
    src_lines: &[&str],
    all_matches: &[MatchedPos],
) {
    let src_line = src_lines[line_num.0 as usize];

    let matches = matches_for_line(all_matches, line_num);
    for m in matches {
        side.changes.push(Change {
            start: m.pos.start_col,
            end: m.pos.end_col,
            content: src_line[m.pos.start_col as usize..m.pos.end_col as usize].into(),
            kind: match m.kind {
                MatchKind::UnchangedToken { highlight, .. } => highlight,
                MatchKind::Novel { highlight, .. } => highlight,
                MatchKind::NovelLinePart { highlight, .. } => highlight,
                MatchKind::NovelWord { highlight, .. } => highlight,
            }
            .into(),
            match_kind: match m.kind {
                MatchKind::UnchangedToken { .. } => JsonMatchKind::UnchangedToken,
                MatchKind::Novel { .. } => JsonMatchKind::Novel,
                MatchKind::NovelLinePart { .. } => JsonMatchKind::NovelLinePart,
                MatchKind::NovelWord { .. } => JsonMatchKind::NovelWord,
            },
        })
    }
}
