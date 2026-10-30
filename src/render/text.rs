use std::time::SystemTime;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
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
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w
    }
    out.push('…');
    out
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
    let indent = truncate(&indent, max.saturating_sub(1));
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
        for c in word.chars() {
            let w = c.width().unwrap_or(0);
            if width(&current) + w > max {
                out.push(current);
                current = indent.clone()
            }
            current.push(c)
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
    let s = now.duration_since(then).unwrap_or_default().as_secs();
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        86400..604800 => format!("{}d ago", s / 86400),
        _ => jiff::Timestamp::try_from(then)
            .map(|t| {
                t.to_zoned(jiff::tz::TimeZone::system())
                    .strftime("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_else(|_| "unknown".into()),
    }
}
pub fn local_hm(ts: &jiff::Timestamp) -> String {
    ts.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%H:%M")
        .to_string()
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
        for line in wrap("  abc abcdefghijkl 界界界", Some(8)) {
            assert!(width(&line) <= 8);
            assert!(line.starts_with("  "));
        }
        assert_eq!(age(SystemTime::UNIX_EPOCH, SystemTime::now()), "just now");
    }
}
