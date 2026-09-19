//! Width-bounded presentation shared by the terminal screens.

use unicode_segmentation::UnicodeSegmentation;

use super::palette::Palette;
use crate::text;

const BRAND: &str = concat!("agf v", env!("CARGO_PKG_VERSION"));
const HINT_GAP: usize = 2;

/// Render exactly one row, reserving essential keys before labels or branding.
/// Enter, Esc, and F1 have selection priority; visible hints retain input order.
/// Their labels may be omitted, but neither keys nor labels are ever truncated.
pub(super) fn footer(ui: &mut slt::Context, hints: &[(&str, &str)]) {
    let palette = Palette::from_ui(ui);
    let hints = clean_bindings(hints);
    // Context::width() is the terminal width, not the enclosing container's.
    // Defer fitting until layout supplies the actual clipped region.
    ui.container().h(1).min_h(1).draw(move |buffer, rect| {
        let layout = footer_layout(rect.width as usize, &hints);
        if layout.show_brand {
            buffer.set_string(
                rect.x + layout.inset as u32,
                rect.y,
                BRAND,
                slt::Style::new().fg(palette.muted).bg(palette.background),
            );
        }
        let mut x = rect.x + layout.hint_x as u32;
        for (position, &index) in layout.selected.iter().enumerate() {
            if position > 0 {
                x += HINT_GAP as u32;
            }
            let (key, action) = &hints[index];
            buffer.set_string(
                x,
                rect.y,
                key,
                slt::Style::new()
                    .fg(palette.accent)
                    .bg(palette.background)
                    .bold(),
            );
            x += text::width(key) as u32;
            if layout.show_actions[index] {
                x += 1;
                buffer.set_string(
                    x,
                    rect.y,
                    action,
                    slt::Style::new()
                        .fg(palette.secondary)
                        .bg(palette.background),
                );
                x += text::width(action) as u32;
            }
        }
    });
}

/// Render a separator, a sanitized single-line title/detail, and a separator.
/// The title takes precedence over the optional detail when space is scarce.
pub(super) fn header(ui: &mut slt::Context, title: &str, detail: Option<&str>) {
    let palette = Palette::from_ui(ui);
    let title = text::sanitize_terminal(title);
    let detail = detail.map(text::sanitize_terminal).unwrap_or_default();
    ui.container().h(3).min_h(3).draw(move |buffer, rect| {
        let rule = "\u{2500}".repeat(rect.width as usize);
        let rule_style = slt::Style::new().fg(palette.border).bg(palette.background);
        buffer.set_string(rect.x, rect.y, &rule, rule_style);
        if rect.height > 2 {
            buffer.set_string(rect.x, rect.y + 2, &rule, rule_style);
        }
        if rect.height > 1 {
            let inset = horizontal_inset(rect.width as usize);
            let width = (rect.width as usize).saturating_sub(inset * 2);
            let (title, detail) = header_parts(width, &title, &detail);
            let x = rect.x + inset as u32;
            buffer.set_string(
                x,
                rect.y + 1,
                &title,
                slt::Style::new()
                    .fg(palette.text)
                    .bg(palette.background)
                    .bold(),
            );
            buffer.set_string(
                x + text::width(&title) as u32,
                rect.y + 1,
                &detail,
                slt::Style::new()
                    .fg(palette.secondary)
                    .bg(palette.background),
            );
        }
    });
}

/// Word-wrap sanitized metadata in display columns, never splitting graphemes.
/// Empty text or a zero-column budget produces no rows. An indivisible grapheme
/// wider than the entire budget is replaced with `?` so wrapping still advances.
pub(super) fn wrap_text(value: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let clean = text::sanitize_terminal(value);
    let mut rows = Vec::new();
    let mut line = String::new();
    let mut start = 0;
    // Only split at whole space graphemes: a space with combining marks must
    // stay intact just like a CJK or ZWJ cluster.
    for (offset, grapheme) in clean.grapheme_indices(true) {
        if grapheme == " " {
            wrap_word(&clean[start..offset], width, &mut line, &mut rows);
            start = offset + grapheme.len();
        }
    }
    wrap_word(&clean[start..], width, &mut line, &mut rows);
    if !line.is_empty() {
        rows.push(line);
    }
    rows
}

/// Format aligned key/action help with hanging action continuations.
/// Long keys stack above their actions on narrow screens; keys wider than the
/// whole row are omitted with their actions rather than shown as partial chords.
pub(super) fn key_rows(width: usize, bindings: &[(&str, &str)]) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let bindings = clean_bindings(bindings);
    let key_column = bindings
        .iter()
        .map(|(key, _)| text::width(key))
        .filter(|&key_width| key_width <= width)
        .max()
        .unwrap_or(0)
        .min(width / 2);
    let action_column = key_column + HINT_GAP;
    let action_width = width.saturating_sub(action_column);
    let mut rows = Vec::new();
    for (key, action) in bindings {
        let key_width = text::width(&key);
        if key_width > width {
            continue;
        }
        if action.is_empty() {
            rows.push(key);
        } else if key_width <= key_column && action_width >= 8 {
            for (index, line) in wrap_text(&action, action_width).into_iter().enumerate() {
                let prefix = if index == 0 {
                    format!("{}{}", text::pad(&key, key_column), " ".repeat(HINT_GAP))
                } else {
                    " ".repeat(action_column)
                };
                rows.push(format!("{prefix}{line}"));
            }
        } else {
            rows.push(key);
            let indent = if width >= 4 { 2 } else { 0 };
            for line in wrap_text(&action, width - indent) {
                rows.push(format!("{}{line}", " ".repeat(indent)));
            }
        }
    }
    rows
}

fn clean_bindings(bindings: &[(&str, &str)]) -> Vec<(String, String)> {
    bindings
        .iter()
        .map(|(key, action)| {
            (
                text::sanitize_terminal(key),
                text::sanitize_terminal(action),
            )
        })
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn horizontal_inset(width: usize) -> usize {
    usize::from(width >= 3)
}

struct FooterLayout {
    selected: Vec<usize>,
    show_actions: Vec<bool>,
    show_brand: bool,
    inset: usize,
    hint_x: usize,
}

fn footer_layout(width: usize, hints: &[(String, String)]) -> FooterLayout {
    let inset = horizontal_inset(width);
    let budget = width.saturating_sub(inset * 2);
    let mut selected = vec![false; hints.len()];
    let mut show_actions = vec![false; hints.len()];
    let mut used = 0;
    let mut count = 0;

    // Reserve essential key names first, so labels cannot displace Esc or F1
    // at narrow widths. Selection order and display order are independent.
    for priority in 0..3 {
        for (index, (key, _)) in hints.iter().enumerate() {
            if key_priority(key) != priority {
                continue;
            }
            let gap = if count > 0 { HINT_GAP } else { 0 };
            let cost = text::width(key) + gap;
            if cost <= budget - used {
                selected[index] = true;
                used += cost;
                count += 1;
            }
        }
    }
    for priority in 0..3 {
        for (index, (key, action)) in hints.iter().enumerate() {
            if selected[index] && key_priority(key) == priority && !action.is_empty() {
                let cost = 1 + text::width(action);
                if cost <= budget - used {
                    show_actions[index] = true;
                    used += cost;
                }
            }
        }
    }
    for (index, (key, action)) in hints.iter().enumerate() {
        if key_priority(key) != 3 {
            continue;
        }
        let cost = text::width(key)
            + usize::from(!action.is_empty())
            + text::width(action)
            + if count > 0 { HINT_GAP } else { 0 };
        if cost <= budget - used {
            selected[index] = true;
            show_actions[index] = !action.is_empty();
            used += cost;
            count += 1;
        }
    }
    let brand_gap = if count > 0 { HINT_GAP } else { 0 };
    let show_brand = text::width(BRAND) + brand_gap <= budget - used;
    FooterLayout {
        selected: selected
            .into_iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect(),
        show_actions,
        show_brand,
        inset,
        hint_x: if show_brand {
            width - inset - used
        } else {
            inset
        },
    }
}

fn key_priority(key: &str) -> usize {
    match key {
        "Enter" => 0,
        "Esc" => 1,
        "F1" => 2,
        _ => 3,
    }
}

fn header_parts(width: usize, title: &str, detail: &str) -> (String, String) {
    let title = text::truncate(title, width);
    let remaining = width.saturating_sub(text::width(&title));
    let separator = if title.is_empty() { "" } else { " | " };
    let detail = if !detail.is_empty() && remaining > separator.len() {
        let clipped = text::truncate(detail, remaining - separator.len());
        if clipped.is_empty() {
            String::new()
        } else {
            format!("{separator}{clipped}")
        }
    } else {
        String::new()
    };
    (title, detail)
}

fn wrap_word(word: &str, width: usize, line: &mut String, rows: &mut Vec<String>) {
    if word.is_empty() {
        return;
    }
    let word_width = text::width(word);
    let used = text::width(line);
    if !line.is_empty() && used + 1 + word_width <= width {
        line.push(' ');
        line.push_str(word);
        return;
    }
    if !line.is_empty() {
        rows.push(std::mem::take(line));
    }
    if word_width <= width {
        line.push_str(word);
        return;
    }
    let mut used = 0;
    for grapheme in word.graphemes(true) {
        let grapheme = if text::width(grapheme) > width {
            "?"
        } else {
            grapheme
        };
        let columns = text::width(grapheme);
        if used + columns > width {
            rows.push(std::mem::take(line));
            used = 0;
        }
        line.push_str(grapheme);
        used += columns;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slt::TestBackend;

    const HINTS: &[(&str, &str)] = &[
        ("Ctrl+Shift+Tab", "previous"),
        ("F1", "help"),
        ("Enter", "open"),
        ("Esc", "back"),
        ("Tab", "agent"),
        ("Ctrl+D", "delete"),
    ];
    const BROWSE_HINTS: &[(&str, &str)] = &[
        ("\u{2191}\u{2193}", "Move"),
        ("Enter", "Actions"),
        ("Tab", "Agent"),
        ("F2", "Scope"),
        ("F3/F4", "Summary"),
        ("Ctrl+L", "Details"),
        ("Ctrl+S", "Sort"),
        ("Ctrl+G", "Projects"),
        ("Ctrl+D", "Delete"),
        ("F1", "Help"),
        ("Esc", "Quit"),
    ];
    const CJK: &str = "\u{D504}\u{B85C}\u{C81D}\u{D2B8}";
    const SCIENTIST: &str = "\u{1F469}\u{200D}\u{1F52C}";

    fn hint_text(key: &str, action: &str) -> String {
        if action.is_empty() {
            key.to_string()
        } else {
            format!("{key} {action}")
        }
    }

    fn expected_footer(width: usize, hints: &[(&str, &str)]) -> String {
        let hints = clean_bindings(hints);
        let layout = footer_layout(width, &hints);
        let selected: Vec<String> = layout
            .selected
            .iter()
            .map(|&index| {
                hint_text(
                    &hints[index].0,
                    if layout.show_actions[index] {
                        &hints[index].1
                    } else {
                        ""
                    },
                )
            })
            .collect();
        let mut line = " ".repeat(layout.inset);
        if layout.show_brand {
            line.push_str(BRAND);
            line.push_str(&" ".repeat(layout.hint_x - text::width(&line)));
        }
        line.push_str(&selected.join(&" ".repeat(HINT_GAP)));
        line.trim_end().to_string()
    }

    fn assert_bounded(rows: &[String], width: usize) {
        for row in rows {
            assert!(text::width(row) <= width, "width {width}: {row:?}");
            assert!(!row.chars().any(char::is_control), "{row:?}");
            assert!(
                !row.chars()
                    .any(|ch| matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')),
                "{row:?}"
            );
        }
    }

    #[test]
    fn footer_selects_essential_hints_before_secondary_and_brand() {
        let hints = clean_bindings(HINTS);
        let essential_width = text::width("F1 help  Enter open  Esc back") + 2;
        let layout = footer_layout(essential_width, &hints);
        assert_eq!(layout.selected, vec![1, 2, 3]);
        assert!(!layout.show_brand);
        assert_eq!(
            expected_footer(essential_width, HINTS),
            " F1 help  Enter open  Esc back"
        );
        assert_eq!(footer_layout(12, &hints).selected, vec![2, 3]);
        assert_eq!(footer_layout(10, &hints).selected, vec![2]);
        assert_eq!(footer_layout(9, &hints).selected, vec![2]);
    }

    #[test]
    fn footer_retains_provided_order_and_consistent_gaps() {
        let expected =
            "Ctrl+Shift+Tab previous  F1 help  Enter open  Esc back  Tab agent  Ctrl+D delete";
        let line = expected_footer(160, HINTS);
        assert!(line.starts_with(&format!(" {BRAND}")));
        assert!(line.ends_with(expected));
        assert_eq!(
            footer_layout(160, &clean_bindings(HINTS)).selected,
            (0..HINTS.len()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn browse_footer_keeps_enter_f1_esc_at_qa_widths() {
        for width in [20, 39, 40, 80, 120] {
            let mut backend = TestBackend::new(width, 2);
            backend.render(|ui| {
                let _ = ui.col(|ui| {
                    footer(ui, BROWSE_HINTS);
                    ui.text("Z");
                });
            });
            let line = backend.line(0);
            assert!(line.contains("Enter"), "width {width}: {line:?}");
            assert!(line.contains("F1"), "width {width}: {line:?}");
            assert!(line.contains("Esc"), "width {width}: {line:?}");
            assert!(line.find("Enter").unwrap() < line.find("F1").unwrap());
            assert!(line.find("F1").unwrap() < line.find("Esc").unwrap());
            assert!(text::width(&line) <= width as usize);
            for part in line.trim().split("  ") {
                if part.starts_with("Ctrl+") {
                    assert!(
                        BROWSE_HINTS
                            .iter()
                            .any(|(key, action)| part == hint_text(key, action))
                    );
                }
            }
            backend.assert_line(1, "Z");
            if width == 20 {
                assert_eq!(line, " Enter  F1  Esc");
                assert!(!line.contains(BRAND));
            }
        }
    }

    #[test]
    fn footer_never_reserves_brand_space_at_a_hints_expense() {
        let hints = [("Enter", "open"), ("Ctrl+D", "delete")];
        let without_brand = text::width("Enter open  Ctrl+D delete") + 2;
        let with_brand = without_brand + text::width(BRAND) + HINT_GAP;
        for width in without_brand..with_brand {
            let layout = footer_layout(width, &clean_bindings(&hints));
            assert_eq!(layout.selected, vec![0, 1]);
            assert!(!layout.show_brand);
        }
        assert!(footer_layout(with_brand, &clean_bindings(&hints)).show_brand);
        assert!(!footer_layout(text::width(BRAND) + 1, &[]).show_brand);
        assert!(footer_layout(text::width(BRAND) + 2, &[]).show_brand);
    }

    #[test]
    fn footer_only_emits_complete_keys_and_actions_for_every_width() {
        let hints = [
            ("Ctrl+Shift+Tab", CJK),
            ("Enter", SCIENTIST),
            ("Esc", "back"),
            ("F1", "help"),
        ];
        let clean = clean_bindings(&hints);
        for width in 0..=160 {
            let layout = footer_layout(width, &clean);
            let line = expected_footer(width, &hints);
            assert!(text::width(&line) <= width);
            assert!(layout.selected.windows(2).all(|pair| pair[0] < pair[1]));
            for (index, (key, action)) in hints.iter().enumerate() {
                assert_eq!(line.contains(key), layout.selected.contains(&index));
                if layout.show_actions[index] {
                    assert!(line.contains(&hint_text(key, action)));
                }
            }
            assert!(!line.contains("Ctrl+") || line.contains("Ctrl+Shift+Tab"));
        }
    }

    #[test]
    fn empty_bindings_and_empty_actions_are_supported() {
        assert!(key_rows(0, HINTS).is_empty());
        assert!(key_rows(40, &[]).is_empty());
        assert!(key_rows(40, &[("\n\t\x1b", "orphan")]).is_empty());
        assert_eq!(key_rows(40, &[("F1", "")]), vec!["F1"]);
        assert_eq!(expected_footer(4, &[("F1", "")]), " F1");
    }

    #[test]
    fn wrapping_empty_and_zero_width_text_has_no_rows() {
        assert!(wrap_text("anything", 0).is_empty());
        assert!(wrap_text("", 10).is_empty());
        assert!(wrap_text("\n\t \r\x1b\x07", 10).is_empty());
    }

    #[test]
    fn wrapping_prefers_words_but_splits_long_words_at_graphemes() {
        assert_eq!(
            wrap_text("alpha beta gamma", 10),
            vec!["alpha beta", "gamma"]
        );
        assert_eq!(
            wrap_text("ab abcdefgh ij", 5),
            vec!["ab", "abcde", "fgh", "ij"]
        );
        assert_eq!(wrap_text("abcde fghij", 5), vec!["abcde", "fghij"]);
        assert_eq!(
            wrap_text(CJK, 4),
            vec!["\u{D504}\u{B85C}", "\u{C81D}\u{D2B8}"]
        );
    }

    #[test]
    fn wrapping_sanitizes_terminal_controls_and_bidi_formatting() {
        let value = "  one\n\ttwo\rthree\x1b]52;c;secret\x07\u{0085}\u{202e}\u{2066}end  ";
        let expected = text::sanitize_terminal(value);
        assert_eq!(wrap_text(value, 160), vec![expected]);
        for width in 0..=160 {
            assert_bounded(&wrap_text(value, width), width);
        }
    }

    #[test]
    fn wrapping_preserves_cjk_zwj_flags_and_combining_clusters() {
        let value = format!("{CJK}{SCIENTIST}e\u{301}\u{1F1F0}\u{1F1F7}\u{1F44D}\u{1F3FD}");
        let clusters: Vec<&str> = value.graphemes(true).collect();
        for width in 2..=160 {
            let rows = wrap_text(&value, width);
            assert_bounded(&rows, width);
            assert_eq!(rows.concat(), value, "width {width}");
            assert_eq!(
                rows.iter()
                    .flat_map(|row| row.graphemes(true))
                    .collect::<Vec<_>>(),
                clusters
            );
        }
        assert_eq!(
            wrap_text(&format!("{SCIENTIST}a{CJK}"), 1),
            vec!["?", "a", "?", "?", "?", "?"]
        );
    }

    #[test]
    fn wrapping_does_not_split_a_space_combining_cluster() {
        let value = "a \u{301}b";
        let rows = wrap_text(value, 1);
        assert_eq!(rows, vec!["a", " \u{301}", "b"]);
        assert_eq!(rows.concat(), value);
    }

    #[test]
    fn long_unbroken_metadata_is_bounded_without_losing_content() {
        let value = "abcdefghij".repeat(200);
        for width in 1..=160 {
            let rows = wrap_text(&value, width);
            assert_bounded(&rows, width);
            assert_eq!(rows.concat(), value);
        }
    }

    #[test]
    fn help_aligns_display_columns_and_hanging_lines_at_forty_columns() {
        let bindings = [
            ("Ctrl+Shift+Tab", "Cycle to the previous agent filter"),
            (CJK, "Open project"),
            ("F1", "Show help"),
        ];
        assert_eq!(text::width(bindings[0].0), 14);
        assert_eq!(text::width(CJK), 8);
        let rows = key_rows(40, &bindings);
        assert_eq!(
            rows,
            vec![
                "Ctrl+Shift+Tab  Cycle to the previous",
                "                agent filter",
                "\u{D504}\u{B85C}\u{C81D}\u{D2B8}        Open project",
                "F1              Show help",
            ]
        );
        assert_bounded(&rows, 40);
        for (row, action) in [(0, "Cycle"), (1, "agent"), (2, "Open"), (3, "Show")] {
            assert_eq!(
                text::width(&rows[row][..rows[row].find(action).unwrap()]),
                16,
                "row {row}: {:?}",
                rows[row]
            );
        }
    }

    #[test]
    fn help_stacks_long_keys_and_never_clips_chords() {
        let bindings = [("Ctrl+Shift+Tab", "Previous agent"), ("Esc", "Close")];
        assert_eq!(text::width(bindings[0].0), 14);
        let keys_only = [(bindings[0].0, ""), (bindings[1].0, "")];
        let rows = key_rows(16, &bindings);
        assert_eq!(
            rows,
            vec!["Ctrl+Shift+Tab", "  Previous agent", "Esc", "  Close"]
        );
        for width in 0..=160 {
            let rows = key_rows(width, &bindings);
            assert_bounded(&rows, width);
            let expected_keys: Vec<String> = bindings
                .iter()
                .filter(|(key, _)| text::width(key) <= width)
                .map(|(key, _)| key.to_string())
                .collect();
            assert_eq!(
                key_rows(width, &keys_only),
                expected_keys,
                "width {width}: keys must be complete, in order, or absent"
            );
            for (key, _) in bindings {
                let complete_key_rows = rows
                    .iter()
                    .filter(|row| {
                        row.as_str() == key
                            || row
                                .strip_prefix(key)
                                .is_some_and(|suffix| suffix.starts_with("  "))
                    })
                    .count();
                assert_eq!(
                    complete_key_rows,
                    usize::from(width >= text::width(key)),
                    "width {width}, key {key:?}: {rows:?}"
                );
            }
            assert!(!rows.iter().any(|row| row.trim_end().ends_with("Ctrl+")));
        }
    }

    #[test]
    fn help_bounds_sanitized_unicode_actions_at_every_width() {
        let action = format!("{CJK}\n{SCIENTIST}\tlonglonglongword\x1b\x07");
        for width in 0..=160 {
            assert_bounded(
                &key_rows(width, &[("Enter", &action), (CJK, "Open")]),
                width,
            );
        }
    }

    #[test]
    fn help_rows_render_without_wrapping_or_overwriting_following_rows() {
        let bindings = [
            ("Ctrl+Shift+Tab", "Previous agent filter"),
            (CJK, SCIENTIST),
            ("Esc", "Close"),
        ];
        for width in [20, 39, 40, 80, 120] {
            let rows = key_rows(width as usize, &bindings);
            let mut backend = TestBackend::new(width, rows.len() as u32 + 2);
            backend.render(|ui| {
                let _ = ui.col(|ui| {
                    for row in &rows {
                        ui.text(row.clone());
                    }
                    ui.text("Z");
                });
            });
            for (index, row) in rows.iter().enumerate() {
                backend.assert_line(index as u32, row.trim_end());
            }
            backend.assert_line(rows.len() as u32, "Z");
            backend.assert_line(rows.len() as u32 + 1, "");
        }
    }

    #[test]
    fn header_parts_fit_and_prefer_title_without_dangling_separator() {
        for width in 0..=160 {
            let (title, detail) = header_parts(width, CJK, SCIENTIST);
            assert!(text::width(&title) + text::width(&detail) <= width);
            assert!(!detail.ends_with(" | "));
        }
        assert_eq!(
            header_parts(5, "Title", "detail"),
            ("Title".into(), String::new())
        );
        assert_eq!(
            header_parts(8, "Title", "detail"),
            ("Title".into(), String::new())
        );
        assert_eq!(
            header_parts(12, "Title", "detail"),
            ("Title".into(), " | det\u{2026}".into())
        );
        assert_eq!(
            header_parts(20, "", "detail"),
            (String::new(), "detail".into())
        );
    }

    #[test]
    fn footer_renders_one_row_without_overwriting_neighbors_at_every_width() {
        for width in 0..=160 {
            let mut backend = TestBackend::new(width, 4);
            backend.render(|ui| {
                let _ = ui.col(|ui| {
                    ui.text("A");
                    footer(ui, HINTS);
                    ui.text("Z");
                });
            });
            backend.assert_line(0, if width == 0 { "" } else { "A" });
            backend.assert_line(1, &expected_footer(width as usize, HINTS));
            backend.assert_line(2, if width == 0 { "" } else { "Z" });
            backend.assert_line(3, "");
        }
    }

    #[test]
    fn header_renders_exactly_three_rows_and_sanitizes_metadata() {
        let title = format!("{CJK}\nTitle\x1b\x07");
        let detail = format!("{SCIENTIST}\tbranch\u{202e}");
        for width in 0..=160 {
            let mut backend = TestBackend::new(width, 6);
            backend.render(|ui| {
                let _ = ui.col(|ui| {
                    ui.text("A");
                    header(ui, &title, Some(&detail));
                    ui.text("Z");
                });
            });
            backend.assert_line(0, if width == 0 { "" } else { "A" });
            backend.assert_line(1, &"\u{2500}".repeat(width as usize));
            backend.assert_line(3, &"\u{2500}".repeat(width as usize));
            backend.assert_line(4, if width == 0 { "" } else { "Z" });
            backend.assert_line(5, "");
            let inset = horizontal_inset(width as usize);
            let (title, detail) = header_parts(
                (width as usize).saturating_sub(2 * inset),
                &text::sanitize_terminal(&title),
                &text::sanitize_terminal(&detail),
            );
            let expected = format!("{}{title}{detail}", " ".repeat(inset));
            backend.assert_line(2, expected.trim_end());
            assert!(text::width(&backend.line(2)) <= width as usize);
        }
    }

    #[test]
    fn unicode_footer_renders_complete_hints_in_the_actual_container_width() {
        let hints = [
            ("Ctrl+Shift+Tab", CJK),
            ("Enter", SCIENTIST),
            ("Esc", "back"),
            ("F1", "help"),
        ];
        for width in 0..=160 {
            let mut backend = TestBackend::new(164, 6);
            backend.render(|ui| {
                let _ = ui.row(|ui| {
                    let _ = ui.container().w(2).col(|ui| {
                        ui.text("L");
                    });
                    let _ = ui.container().w(width).col(|ui| {
                        header(ui, CJK, Some(SCIENTIST));
                        footer(ui, &hints);
                        ui.text("Z");
                    });
                    let _ = ui.container().w(2).col(|ui| {
                        for _ in 0..5 {
                            ui.text("R");
                        }
                    });
                });
            });
            for y in 0..5 {
                assert_eq!(
                    backend.buffer().get(width + 2, y).symbol.as_str(),
                    "R",
                    "width {width}, y {y}"
                );
            }
            let mut footer_row = String::new();
            for x in 2..width + 2 {
                footer_row.push_str(&backend.buffer().get(x, 3).symbol);
            }
            assert_eq!(
                footer_row.trim_end(),
                expected_footer(width as usize, &hints),
                "width {width}"
            );
            if width > 0 {
                assert_eq!(backend.buffer().get(2, 4).symbol.as_str(), "Z");
            }
            backend.assert_line(5, "");
        }
    }

    #[test]
    fn helpers_remain_clipped_in_short_viewports() {
        for height in 0..=4 {
            let mut backend = TestBackend::new(40, height);
            backend.render(|ui| {
                let _ = ui.col(|ui| {
                    header(ui, "Title", None);
                    footer(ui, HINTS);
                });
            });
            if height > 0 {
                backend.assert_line(0, &"\u{2500}".repeat(40));
            }
            if height > 1 {
                backend.assert_line(1, " Title");
            }
            if height > 2 {
                backend.assert_line(2, &"\u{2500}".repeat(40));
            }
            if height > 3 {
                backend.assert_line(3, &expected_footer(40, HINTS));
            }
        }
    }

    #[test]
    fn raw_header_footer_keep_background_for_every_cell_in_both_themes() {
        let mut backend = TestBackend::new(80, 4);
        for (theme, palette) in [
            (slt::Theme::dark(), Palette::dark()),
            (slt::Theme::light(), Palette::light()),
            (slt::Theme::dark(), Palette::dark()),
        ] {
            backend.render(|ui| {
                ui.set_theme(theme);
                let _ = ui.container().h(4).bg(palette.background).col(|ui| {
                    header(ui, "Details", Some("Claude Code"));
                    footer(ui, &[("Enter", "open")]);
                });
            });
            backend.assert_line_contains(1, "Details | Claude Code");
            backend.assert_line_contains(3, "Enter open");
            backend.assert_line_contains(3, BRAND);
            for y in 0..4 {
                for x in 0..80 {
                    assert_eq!(
                        backend.buffer().get(x, y).style.bg,
                        Some(palette.background),
                        "raw drawing lost the explicit background at ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn footer_uses_semantic_palette_roles_after_theme_changes() {
        let mut backend = TestBackend::new(80, 1);
        for (theme, palette) in [
            (slt::Theme::dark(), Palette::dark()),
            (slt::Theme::light(), Palette::light()),
            (slt::Theme::dark(), Palette::dark()),
        ] {
            backend.render(|ui| {
                ui.set_theme(theme);
                footer(ui, &[("Enter", "open")]);
            });
            let row = backend.line(0);
            assert_eq!(
                backend.buffer().get(1, 0).style,
                slt::Style::new().fg(palette.muted).bg(palette.background)
            );
            let key_x = row.find("Enter").unwrap() as u32;
            assert_eq!(
                backend.buffer().get(key_x, 0).style,
                slt::Style::new()
                    .fg(palette.accent)
                    .bg(palette.background)
                    .bold()
            );
            assert_eq!(
                backend.buffer().get(key_x + 6, 0).style,
                slt::Style::new()
                    .fg(palette.secondary)
                    .bg(palette.background)
            );
            for x in 0..80 {
                let style = backend.buffer().get(x, 0).style;
                assert_eq!(
                    style.fg == Some(palette.accent),
                    (key_x..key_x + 5).contains(&x),
                    "only footer key names may use accent at column {x}"
                );
            }
        }
    }

    #[test]
    fn header_uses_readable_title_and_quiet_metadata_in_both_themes() {
        let mut backend = TestBackend::new(40, 3);
        for (theme, palette) in [
            (slt::Theme::dark(), Palette::dark()),
            (slt::Theme::light(), Palette::light()),
            (slt::Theme::dark(), Palette::dark()),
        ] {
            backend.render(|ui| {
                ui.set_theme(theme);
                header(ui, "Details", Some("Claude Code"));
            });
            backend.assert_line(1, " Details | Claude Code");
            assert_eq!(backend.buffer().get(0, 0).style.fg, Some(palette.border));
            assert_eq!(backend.buffer().get(0, 2).style.fg, Some(palette.border));
            assert_eq!(
                backend.buffer().get(1, 1).style,
                slt::Style::new()
                    .fg(palette.text)
                    .bg(palette.background)
                    .bold()
            );
            assert_eq!(
                backend.buffer().get(11, 1).style.fg,
                Some(palette.secondary)
            );
            for x in 8..21 {
                assert_eq!(
                    backend.buffer().get(x, 1).style,
                    slt::Style::new()
                        .fg(palette.secondary)
                        .bg(palette.background)
                );
            }
            for y in 0..3 {
                for x in 0..40 {
                    assert_ne!(backend.buffer().get(x, y).style.fg, Some(palette.accent));
                }
            }
        }
    }
}
