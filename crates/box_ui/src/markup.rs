//! Styled text: inline tags in any node text (literal or from a string
//! table), so translators can style words.
//!
//! | Tag | Effect |
//! |---|---|
//! | `[color=#e07a5f]…[/color]`, `[color=$accent]…[/color]` | text colour (hex or theme palette) |
//! | `[size=32]…[/size]` | text size in reference points |
//! | `[font=display]…[/font]` | a theme font role |
//! | `[i]…[/i]` | italic (slanted) |
//! | `[u]…[/u]` | underline |
//!
//! Tags nest; a closing tag ends the most recent open tag of its name.
//! `[[` writes a literal `[`. Anything else in brackets (`[3/5]`, an
//! unknown or malformed tag) is plain text.

/// One run of text with the same overrides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    /// Colour as written (`#hex` or `$palette`).
    pub color: Option<String>,
    pub size: Option<f32>,
    pub font: Option<String>,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Tag {
    Color(String),
    Size(f32),
    Font(String),
    Italic,
    Underline,
}

impl Tag {
    fn name(&self) -> &'static str {
        match self {
            Tag::Color(_) => "color",
            Tag::Size(_) => "size",
            Tag::Font(_) => "font",
            Tag::Italic => "i",
            Tag::Underline => "u",
        }
    }

    fn parse(inner: &str) -> Option<Tag> {
        let (name, value) = match inner.split_once('=') {
            Some((n, v)) => (n.trim(), Some(v.trim())),
            None => (inner.trim(), None),
        };
        match (name, value) {
            ("color", Some(v)) if v.starts_with('#') || v.starts_with('$') => {
                Some(Tag::Color(v.to_string()))
            }
            ("size", Some(v)) => v.parse::<f32>().ok().filter(|s| *s > 0.0).map(Tag::Size),
            ("font", Some(v)) if !v.is_empty() => Some(Tag::Font(v.to_string())),
            ("i", None) => Some(Tag::Italic),
            ("u", None) => Some(Tag::Underline),
            _ => None,
        }
    }
}

fn is_tag_name(s: &str) -> bool {
    matches!(s, "color" | "size" | "font" | "i" | "u")
}

/// Splits `text` into spans. Text without tags is one span.
pub fn parse(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut stack: Vec<Tag> = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, stack: &[Tag], spans: &mut Vec<Span>| {
        if current.is_empty() {
            return;
        }
        let mut span = Span {
            text: std::mem::take(current),
            ..Default::default()
        };
        for t in stack {
            match t {
                Tag::Color(c) => span.color = Some(c.clone()),
                Tag::Size(s) => span.size = Some(*s),
                Tag::Font(f) => span.font = Some(f.clone()),
                Tag::Italic => span.italic = true,
                Tag::Underline => span.underline = true,
            }
        }
        spans.push(span);
    };
    let mut rest = text;
    while let Some(i) = rest.find('[') {
        current.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(stripped) = after.strip_prefix('[') {
            current.push('[');
            rest = stripped;
            continue;
        }
        let Some(end) = after.find(']') else {
            current.push('[');
            rest = after;
            continue;
        };
        let inner = &after[..end];
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim();
            if is_tag_name(name)
                && let Some(pos) = stack.iter().rposition(|t| t.name() == name)
            {
                flush(&mut current, &stack, &mut spans);
                stack.remove(pos);
                rest = &after[end + 1..];
                continue;
            }
        } else if let Some(tag) = Tag::parse(inner) {
            flush(&mut current, &stack, &mut spans);
            stack.push(tag);
            rest = &after[end + 1..];
            continue;
        }
        // Not a tag: keep the bracket as text.
        current.push('[');
        rest = after;
    }
    current.push_str(rest);
    flush(&mut current, &stack, &mut spans);
    spans
}

/// The text without tags (for measuring, tests and accessibility).
pub fn plain(text: &str) -> String {
    parse(text).into_iter().map(|s| s.text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_nest_and_unknown_brackets_stay_text() {
        let s = parse("Hi [color=#ff0000]red [size=40]big[/size][/color] [3/5] [[x]");
        let texts: Vec<&str> = s.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Hi ", "red ", "big", " [3/5] [x]"]);
        assert_eq!(s[1].color.as_deref(), Some("#ff0000"));
        assert_eq!(s[2].size, Some(40.0));
        assert_eq!(s[2].color.as_deref(), Some("#ff0000"));
        assert_eq!(s[3].color, None);
        assert_eq!(plain("[u]a[/u]b[i]c"), "abc");
        assert_eq!(parse("plain").len(), 1);
        assert_eq!(plain("[color=red]x[/color]"), "[color=red]x[/color]");
    }
}
