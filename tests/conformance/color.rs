//! Colour policy, depth, theme and palette (AC-007, AC-008, AC-009; §A10).

use crate::support::*;

fn has_esc_seq(b: &[u8]) -> bool {
    b.windows(2).any(|w| w == b"\x1b[")
}

#[test]
fn ac007_no_color_wins_over_clicolor_force() {
    let sb = Sandbox::new();
    for no_color in ["1", " "] {
        let runs = [
            sb.glen(["doctor", "--human"]).fake_omp(),
            sb.glen(["doctor", "--human", "--color", "auto"]).fake_omp(),
            sb.glen(["doctor", "--human"])
                .fake_omp()
                .env("FAKE_OMP_VERSION", "18.0.9"),
            sb.glen(["doctor", "--human"]),
            sb.glen(["ls", "--human"]),
            sb.glen(["--human", "bogus"]),
            sb.glen(["--help"]),
        ];
        for run in runs {
            let out = run
                .env("NO_COLOR", no_color)
                .env("CLICOLOR_FORCE", "1")
                .env("TERM", "xterm-256color")
                .run();
            out.assert_no_ansi();
        }
    }
}

#[test]
fn ac008_clicolor_force_colours_human_output_on_a_pipe() {
    let sb = Sandbox::new();
    let runs = [
        sb.glen(["doctor", "--human"]).env("CLICOLOR_FORCE", "1"),
        sb.glen(["doctor", "--human", "--color", "auto"])
            .env("CLICOLOR_FORCE", "1"),
        // An empty NO_COLOR counts as unset.
        sb.glen(["doctor", "--human"])
            .env("CLICOLOR_FORCE", "1")
            .env("NO_COLOR", ""),
        // §A10 rule 3 precedes rule 4.
        sb.glen(["doctor", "--human"])
            .env("CLICOLOR_FORCE", "1")
            .env("CLICOLOR", "0"),
        // §A10: any non-empty literal value other than "0", including whitespace.
        sb.glen(["doctor", "--human"]).env("CLICOLOR_FORCE", " "),
    ];
    for run in runs {
        let out = run.fake_omp().run();
        out.assert_exit(0);
        assert!(has_esc_seq(&out.stdout), "expected colour\n{}", out.ctx());
    }
}

#[test]
fn a10_colour_is_decided_per_stream() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["inbox", "--human"])
        .env("CLICOLOR_FORCE", "1")
        .run();
    out.assert_exit(8);
    assert!(
        has_esc_seq(&out.stderr),
        "forced colour applies to stderr diagnostics too\n{}",
        out.ctx()
    );
}

#[test]
fn a10_clicolor_force_zero_does_not_force() {
    let sb = Sandbox::new();
    sb.glen(["doctor", "--human"])
        .fake_omp()
        .env("CLICOLOR_FORCE", "0")
        .run()
        .assert_exit(0)
        .assert_no_ansi();
}

#[test]
fn a10_no_colour_on_a_pipe_without_force() {
    let sb = Sandbox::new();
    // Rule 5 needs a TTY: TERM, CLICOLOR and CI alone never colour a pipe.
    sb.glen(["doctor", "--human"])
        .fake_omp()
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("CLICOLOR", "1")
        .env("CI", "1")
        .run()
        .assert_exit(0)
        .assert_no_ansi();
}

#[test]
fn a10_color_flag_takes_precedence() {
    let sb = Sandbox::new();
    let always = sb
        .glen(["doctor", "--human", "--color", "always"])
        .fake_omp()
        .env("NO_COLOR", "1")
        .run();
    always.assert_exit(0);
    assert!(has_esc_seq(&always.stdout), "{}", always.ctx());

    for run in [
        sb.glen(["doctor", "--human", "--color", "never"])
            .fake_omp(),
        sb.glen(["ls", "--human", "--color", "never"]),
        sb.glen(["--color", "never", "--help"]),
    ] {
        run.env("CLICOLOR_FORCE", "1").run().assert_no_ansi();
    }
}

#[test]
fn ac009_json_output_never_contains_ansi() {
    let sb = Sandbox::new();
    let runs = [
        sb.glen(["doctor", "--json", "--color", "always"])
            .fake_omp(),
        sb.glen(["doctor", "--color", "always"]).fake_omp(),
        sb.glen(["doctor", "--json", "--color", "always"])
            .fake_omp()
            .env("FAKE_OMP_VERSION", "18.0.9"),
        sb.glen(["doctor", "--json", "--color", "always"]),
        sb.glen(["ls", "--json", "--color", "always"]),
        sb.glen(["--json", "--color", "always", "bogus"]),
        sb.glen(["completions", "zsh", "--json", "--color", "always"]),
    ];
    for run in runs {
        let out = run
            .env("CLICOLOR_FORCE", "1")
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .run();
        out.json();
        out.assert_no_ansi();
    }
}

/// §A10 depth and palette: the ok glyph is painted with the `success` token.
#[test]
fn a10_colour_depth_and_theme_select_the_palette() {
    let sb = Sandbox::new();
    let cases: &[(&[(&str, &str)], &str)] = &[
        // TrueColor, dark (default theme): success #00CA50.
        (&[("COLORTERM", "truecolor")], "38;2;0;202;80"),
        (&[("COLORTERM", "24bit")], "38;2;0;202;80"),
        // TrueColor, light: success #007F3D.
        (
            &[("COLORTERM", "truecolor"), ("GLEN_THEME", "light")],
            "38;2;0;127;61",
        ),
        (
            &[("COLORTERM", "truecolor"), ("COLORFGBG", "0;15")],
            "38;2;0;127;61",
        ),
        (
            &[("COLORTERM", "truecolor"), ("COLORFGBG", "15;0")],
            "38;2;0;202;80",
        ),
        // 256 colours: success 41 (dark) / 29 (light).
        (&[("TERM", "xterm-256color")], "38;5;41"),
        (
            &[("TERM", "xterm-256color"), ("GLEN_THEME", "light")],
            "38;5;29",
        ),
        // 16 colours: green.
        (&[("TERM", "xterm")], "\x1b[32m"),
    ];
    for (env, needle) in cases {
        let mut run = sb
            .glen(["doctor", "--human", "--color", "always"])
            .fake_omp();
        for (k, v) in *env {
            run = run.env(k, v);
        }
        let out = run.run();
        out.assert_exit(0);
        assert!(
            out.out().contains(needle),
            "expected {needle:?} for {env:?}\n{}",
            out.ctx()
        );
    }
}

#[test]
fn a10_warning_token_paints_the_untested_version() {
    let sb = Sandbox::new();
    let out = sb
        .glen(["doctor", "--human", "--color", "always"])
        .fake_omp()
        .env("FAKE_OMP_VERSION", "18.0.9")
        .env("COLORTERM", "truecolor")
        .run();
    out.assert_exit(0);
    // warning (dark) #FF9300.
    assert!(out.out().contains("38;2;255;147;0"), "{}", out.ctx());
}
