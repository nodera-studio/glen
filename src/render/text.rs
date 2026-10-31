use std::time::SystemTime;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}
/// Collapse whitespace for a single human-readable field, then escape controls.
pub fn single_line(text: &str) -> String {
    super::sanitize(&text.split_whitespace().collect::<Vec<_>>().join(" ")).into_owned()
}
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.into();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for grapheme in s.graphemes(true) {
        let w = width(grapheme);
        if used + w > max - 1 {
            break;
        }
        out.push_str(grapheme);
        used += w
    }
    out.push('…');
    out
}
/// Keep a path's identifying tail when it does not fit its column budget.
pub fn truncate_left(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.into();
    }
    if max == 0 {
        return String::new();
    }
    let mut suffix = vec![];
    let mut used = 0;
    for grapheme in s.graphemes(true).rev() {
        let cells = width(grapheme);
        if used + cells > max - 1 {
            break;
        }
        suffix.push(grapheme);
        used += cells;
    }
    format!("…{}", suffix.into_iter().rev().collect::<String>())
}
pub fn clean_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(|l| {
            super::sanitize(&l.strip_suffix('\r').unwrap_or(l).replace('\t', "    ")).into_owned()
        })
        .collect()
}
pub fn wrap(line: &str, max: Option<usize>) -> Vec<String> {
    let Some(max) = max.filter(|n| *n > 0) else {
        return vec![line.into()];
    };
    if width(line) <= max {
        return vec![line.into()];
    }
    let indent = line
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    // Carry at most half the width as indent, and none when that would leave
    // fewer than 10 columns, so deep indents cannot wrap one grapheme per line.
    let carried = width(&indent).min(max / 2);
    let indent = if max - carried < 10 {
        String::new()
    } else {
        truncate(&indent, carried)
    };
    let mut out = vec![];
    let mut current = indent.clone();
    for word in line.split_whitespace() {
        if width(&current) > width(&indent) && width(&current) + 1 + width(word) <= max {
            current.push(' ');
            current.push_str(word);
            continue;
        }
        if width(&current) > width(&indent) {
            out.push(current);
            current = indent.clone()
        }
        for grapheme in word.graphemes(true) {
            let w = width(grapheme);
            if width(&current) + w > max {
                out.push(current);
                current = indent.clone()
            }
            current.push_str(grapheme)
        }
    }
    if !current.is_empty() {
        out.push(current)
    }
    if out.is_empty() {
        out.push(String::new())
    }
    out
}
pub fn age(now: SystemTime, then: SystemTime) -> String {
    age_in(now, then, &jiff::tz::TimeZone::system())
}
pub fn age_in(now: SystemTime, then: SystemTime, timezone: &jiff::tz::TimeZone) -> String {
    let s = now.duration_since(then).unwrap_or_default().as_secs();
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        86400..604800 => format!("{}d ago", s / 86400),
        _ => jiff::Timestamp::try_from(then)
            .map(|t| {
                t.to_zoned(timezone.clone())
                    .strftime("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_else(|_| "unknown".into()),
    }
}
pub fn local_hm(ts: &jiff::Timestamp) -> String {
    local_hm_in(ts, &jiff::tz::TimeZone::system())
}
pub fn local_hm_in(ts: &jiff::Timestamp, timezone: &jiff::tz::TimeZone) -> String {
    ts.to_zoned(timezone.clone()).strftime("%H:%M").to_string()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert_eq!(
            single_line("  a\t\n b\r\n\x1b[31m\x07  "),
            "a b \\x1b[31m\\x07"
        );
        assert_eq!(truncate("héllo wörld", 6), "héllo…");
        assert_eq!(truncate("界界界", 5), "界界…");
        assert_eq!(clean_lines("a\tb\n\x1b"), ["a    b", "\\x1b"]);
        // W8-SEC-8: at width 8 the indent would leave fewer than 10 columns, so it is
        // dropped; at width 14 it leaves 12 and every line keeps it.
        for line in wrap("  abc abcdefghijkl 界界界", Some(8)) {
            assert!(width(&line) <= 8);
            assert!(!line.starts_with(' '));
        }
        for line in wrap("  abc abcdefghijklmnopq 界界界界界界界", Some(14)) {
            assert!(width(&line) <= 14);
            assert!(line.starts_with("  "));
        }
        assert_eq!(age(SystemTime::UNIX_EPOCH, SystemTime::now()), "just now");
    }
    #[test]
    fn deep_indents_are_capped_so_wrapping_stays_bounded() {
        let line = format!("{}{}", " ".repeat(300), "a".repeat(100));
        let lines = wrap(&line, Some(40));
        assert!(lines.len() <= 6, "{} lines", lines.len());
        assert!(lines.iter().all(|l| width(l) <= 40));
        let tabs = format!("{}{}", "\t".repeat(75), "b".repeat(100));
        assert!(wrap(&tabs, Some(40)).len() <= 6);
    }
    #[test]
    fn grapheme_truncation_and_wrapping_keep_clusters_whole() {
        assert_eq!(width("👩‍💻"), 2);
        assert_eq!(width("🇷🇴"), 2);
        assert_eq!(truncate("👩‍💻ab", 3), "👩‍💻…");
        assert_eq!(truncate("e\u{301}xy", 2), "e\u{301}…");
        assert_eq!(truncate("你好世界", 5), "你好…");
        assert_eq!(truncate_left("root/你好世界", 5), "…世界");
        assert_eq!(truncate_left("long/👩‍💻x", 4), "…👩‍💻x");
        assert_eq!(truncate_left("abcdef", 0), "");
        assert_eq!(truncate_left("abcdef", 1), "…");
        assert_eq!(truncate_left("abc", 3), "abc");
        assert_eq!(wrap("👩‍💻你好世界", Some(4)), ["👩‍💻你", "好世", "界"]);
    }
    #[test]
    fn explicit_timezone_formatters_do_not_depend_on_the_host_zone() {
        let timezone = jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_hours(9).unwrap());
        let timestamp: jiff::Timestamp = "2026-10-30T23:30:00Z".parse().unwrap();
        let now: jiff::Timestamp = "2026-11-10T00:00:00Z".parse().unwrap();
        assert_eq!(local_hm_in(&timestamp, &timezone), "08:30");
        assert_eq!(
            age_in(now.into(), timestamp.into(), &timezone),
            "2026-10-31"
        );
        assert_eq!(local_hm_in(&timestamp, &jiff::tz::TimeZone::UTC), "23:30");
    }
}
