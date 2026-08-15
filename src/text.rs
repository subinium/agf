//! Display-width helpers shared by every renderer (TUI, `list`, `stats`, `watch`).
//!
//! Rust's `{:<width$}` pads by `char` count, but a terminal lays text out in
//! *display columns* — a Hangul syllable or CJK ideograph occupies two. Mixing
//! the two (measure with `UnicodeWidthStr`, pad with `{:<n$}`) silently skews
//! every column to the right of a CJK cell, so all column work goes through
//! these helpers instead of `format!` padding.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Remove terminal control sequences from untrusted session metadata.
///
/// Project names, branches, and paths originate in agent logs and repositories.
/// Emitting ESC/C0/C1 bytes verbatim lets a crafted name rewrite the terminal,
/// set a title, or copy data through OSC 52.  Single-line renderers do not need
/// any control characters, so whitespace controls become spaces and all other
/// controls are dropped.
pub fn sanitize_terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\n' | '\r' | '\t') {
            out.push(' ');
        } else if ch == '\u{1b}' || ch.is_control() || is_bidi_formatting(ch) {
            continue;
        } else {
            out.push(ch);
        }
    }
    collapse_spaces(&out)
}

fn is_bidi_formatting(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut previous_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !previous_space && !out.is_empty() {
                out.push(' ');
            }
            previous_space = true;
        } else {
            out.push(ch);
            previous_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

pub fn color_enabled() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").ok().as_deref() != Some("dumb")
}

/// Display width of `s` in terminal columns.
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate `s` to at most `max_width` columns, marking the cut with `…`.
pub fn truncate(s: &str, max_width: usize) -> String {
    truncate_with(s, max_width, "…")
}

/// Truncate like [`truncate`], but first collapse embedded newlines/tabs into
/// single spaces and mark the cut with `...`.
///
/// Used for free-form session summaries rendered inside a single TUI row,
/// where a raw `\n` would break the layout.
pub fn truncate_flat(s: &str, max_width: usize) -> String {
    truncate_with(&sanitize_terminal(s), max_width, "...")
}

/// Pad `s` with trailing spaces to `to_width` columns. Wider input is returned
/// unchanged — use [`fit`] when the result must not overflow.
pub fn pad(s: &str, to_width: usize) -> String {
    let current = width(s);
    if current >= to_width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (to_width - current));
    out.push_str(s);
    out.extend(std::iter::repeat_n(' ', to_width - current));
    out
}

/// Render `s` as a fixed column exactly `column_width` wide: truncated if too
/// long, space-padded if too short. This is the CJK-safe replacement for
/// `format!("{:<column_width$}", truncate(s, column_width))`.
pub fn fit(s: &str, column_width: usize) -> String {
    pad(&truncate(&sanitize_terminal(s), column_width), column_width)
}

/// Shared truncation core. The ellipsis is *included* in the budget, so the
/// result never exceeds `max_width` columns. When the ellipsis alone would
/// fill the whole budget there is no room to say anything useful, so the text
/// is hard-cut instead and the marker is dropped.
fn truncate_with(s: &str, max_width: usize, ellipsis: &str) -> String {
    if width(s) <= max_width {
        return s.to_string();
    }
    let ellipsis_width = width(ellipsis);
    let marked = ellipsis_width < max_width;
    let budget = if marked {
        max_width - ellipsis_width
    } else {
        max_width
    };

    let mut out = String::with_capacity(s.len().min(budget.saturating_mul(4)) + ellipsis.len());
    let mut used = 0;
    for grapheme in s.graphemes(true) {
        let grapheme_width = width(grapheme);
        if used + grapheme_width > budget {
            break;
        }
        used += grapheme_width;
        out.push_str(grapheme);
    }
    if marked {
        out.push_str(ellipsis);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_produces_exact_columns_for_ascii_and_cjk() {
        // The regression this module exists for: `format!("{:<10}", name)`
        // pads by char count, so a 5-char / 10-column Hangul name rendered 15
        // columns wide and skewed every column after it.
        for name in ["agf", "프로젝트", "데이터분석", ""] {
            assert_eq!(width(&fit(name, 10)), 10, "name: {name}");
        }
    }

    #[test]
    fn fit_truncates_overlong_input_to_the_column() {
        assert_eq!(width(&fit("a-very-long-project-name", 10)), 10);
        assert_eq!(width(&fit("데이터분석플랫폼", 10)), 10);
    }

    #[test]
    fn truncate_never_splits_a_wide_char_across_the_budget() {
        // Budget 5 = 1 ellipsis + 4 usable columns → exactly two Hangul
        // syllables fit; a third would overflow to 7.
        let out = truncate("데이터분석", 5);
        assert_eq!(out, "데이…");
        assert_eq!(width(&out), 5);
    }

    #[test]
    fn truncate_never_splits_an_emoji_grapheme() {
        let out = truncate("👩‍🔬xyz", 4);
        assert_eq!(out, "👩‍🔬x…");
        assert!(!out.ends_with('\u{200d}'));
    }

    #[test]
    fn truncate_keeps_short_input_verbatim() {
        assert_eq!(truncate("agf", 10), "agf");
        assert_eq!(truncate("프로젝트", 8), "프로젝트");
    }

    #[test]
    fn truncate_drops_the_marker_when_it_would_fill_the_budget() {
        // No room to both mark the cut and show anything: prefer the content.
        assert_eq!(truncate_flat("abcdef", 3), "abc");
        assert_eq!(truncate_flat("abcdef", 4), "a...");
    }

    #[test]
    fn truncate_flat_collapses_newlines_before_measuring() {
        assert_eq!(truncate_flat("first\nsecond", 20), "first second");
        assert_eq!(truncate_flat("  a\t\tb  ", 20), "a b");
    }

    #[test]
    fn pad_leaves_overlong_input_alone() {
        assert_eq!(pad("데이터분석", 4), "데이터분석");
        assert_eq!(pad("ab", 4), "ab  ");
    }

    #[test]
    fn zero_width_budget_yields_empty_string() {
        assert_eq!(truncate("anything", 0), "");
        assert_eq!(fit("anything", 0), "");
    }

    #[test]
    fn sanitize_terminal_strips_escape_and_control_bytes() {
        let hostile = "safe\x1b]52;c;secret\x07\nnext\tpart\u{0085}\u{202e}spoof\u{2066}end";
        let clean = sanitize_terminal(hostile);
        assert_eq!(clean, "safe]52;c;secret next partspoofend");
        assert!(!clean.chars().any(char::is_control));
    }
}
