//! Human tables respect COLUMNS (AC-022; §A10 width).

use crate::support::*;

/// An absolute path outside HOME, so it is printed in full and must wrap.
const LONG_DIR: &str = "/nonexistent/a-rather-long-directory-name/with-several-segments/that-will-not-fit/in-a-narrow-terminal/agent";

fn visible_width(line: &str) -> usize {
    strip_ansi(line).chars().count()
}

fn doctor_at(cols: &str) -> Out {
    let sb = Sandbox::new();
    sb.glen(["doctor", "--human"])
        .fake_omp()
        .env("PI_CODING_AGENT_DIR", LONG_DIR)
        .env("CLICOLOR_FORCE", "1")
        .env("COLUMNS", cols)
        .run()
}

#[test]
fn ac022_no_output_line_exceeds_columns_40_or_80() {
    for cols in [40usize, 80] {
        let out = doctor_at(&cols.to_string());
        out.assert_exit(0);
        for line in out.out().lines() {
            assert!(
                visible_width(line) <= cols,
                "line wider than COLUMNS={cols}: {:?}\n{}",
                strip_ansi(line),
                out.ctx()
            );
        }
    }
}

#[test]
fn ac022_table_lines_fit_the_minimum_width_20() {
    let out = doctor_at("20");
    out.assert_exit(0);
    for line in out.out().lines() {
        let plain = strip_ansi(line);
        // The title and the footer are prose, not table rows.
        if plain.trim() == "glen doctor" || plain.contains("tested against") {
            continue;
        }
        assert!(
            visible_width(line) <= 20,
            "table line wider than COLUMNS=20: {plain:?}\n{}",
            out.ctx()
        );
    }
}

#[test]
fn ac022_out_of_range_columns_are_ignored() {
    for cols in ["5", "19", "1001", "abc", ""] {
        let out = doctor_at(cols);
        out.assert_exit(0);
        let plain = strip_ansi(&out.out());
        // Non-TTY fallback is unbounded: the long path stays on one line.
        assert!(
            plain.contains(LONG_DIR),
            "COLUMNS={cols:?} should be ignored\n{}",
            out.ctx()
        );
    }
}
