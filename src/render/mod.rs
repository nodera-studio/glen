pub mod palette;
pub mod spinner;
pub mod style;
pub mod table;
use std::{borrow::Cow, path::Path};
pub use table::Table;
pub fn sanitize(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| c.is_control()) {
        return Cow::Borrowed(text);
    }
    let mut result = String::new();
    for c in text.chars() {
        match c {
            '\t' => result.push_str("\\t"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\0'..='\u{1f}' | '\u{7f}' => {
                use std::fmt::Write;
                let _ = write!(result, "\\x{:02x}", c as u32);
            }
            '\u{80}'..='\u{9f}' => {
                use std::fmt::Write;
                let _ = write!(result, "\\u{{{:02x}}}", c as u32);
            }
            _ => result.push(c),
        }
    }
    Cow::Owned(result)
}
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    let text = if let Some(rest) = home.and_then(|h| path.strip_prefix(h).ok()) {
        if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        }
    } else {
        path.display().to_string()
    };
    sanitize(&text).into_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls() {
        assert_eq!(sanitize("a\x1b[31mb\x07"), r"a\x1b[31mb\x07");
        assert_eq!(sanitize("x\u{9b}y"), r"x\u{9b}y");
        assert_eq!(sanitize("tab\there\n\r\x7f"), r"tab\there\n\r\x7f");
        assert!(matches!(
            sanitize("plain café"),
            Cow::Borrowed("plain café")
        ));
        assert_eq!(
            tilde(Path::new("/h/a\x1b"), Some(Path::new("/h"))),
            r"~/a\x1b"
        );
    }
}
