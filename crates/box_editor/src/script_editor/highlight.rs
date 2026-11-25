//! Luau syntax colouring for the Script pane: a small tokenizer (comments,
//! strings, numbers, keywords, names) turned into an egui `LayoutJob`, with
//! find matches marked.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// The kinds of span the tokenizer finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Keyword,
    /// `true`, `false`, `nil`, `self`.
    Constant,
    String,
    Number,
    Comment,
    /// A name followed by `(`: a call or a definition.
    Function,
}

const KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "export", "for", "function", "if",
    "in", "local", "not", "or", "repeat", "return", "then", "type", "typeof", "until", "while",
];
const CONSTANTS: &[&str] = &["true", "false", "nil", "self"];

/// Byte spans `(start, end, kind)` covering `text` in order.
pub fn tokenize(text: &str) -> Vec<(usize, usize, Kind)> {
    let b = text.as_bytes();
    let mut out: Vec<(usize, usize, Kind)> = Vec::new();
    let push = |s: usize, e: usize, k: Kind, out: &mut Vec<(usize, usize, Kind)>| {
        if e <= s {
            return;
        }
        match out.last_mut() {
            Some(last) if last.2 == k && last.1 == s => last.1 = e,
            _ => out.push((s, e, k)),
        }
    };
    let long_bracket = |i: usize| -> Option<usize> {
        // `[[` or `[=[`: the number of `=`.
        if b.get(i) != Some(&b'[') {
            return None;
        }
        let mut j = i + 1;
        while b.get(j) == Some(&b'=') {
            j += 1;
        }
        (b.get(j) == Some(&b'[')).then_some(j - i - 1)
    };
    let close_long = |from: usize, level: usize| -> usize {
        let close: String = format!("]{}]", "=".repeat(level));
        text[from..]
            .find(&close)
            .map_or(text.len(), |p| from + p + close.len())
    };
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            let end = match long_bracket(i + 2) {
                Some(level) => close_long(i + 4 + level, level),
                None => text[i..].find('\n').map_or(text.len(), |p| i + p),
            };
            push(i, end, Kind::Comment, &mut out);
            i = end;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c && b[j] != b'\n' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(b.len());
            push(i, end, Kind::String, &mut out);
            i = end;
        } else if let Some(level) = long_bracket(i) {
            let end = close_long(i + 2 + level, level);
            push(i, end, Kind::String, &mut out);
            i = end;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_') {
                j += 1;
            }
            push(i, j, Kind::Number, &mut out);
            i = j;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            let word = &text[i..j];
            let mut k = j;
            while k < b.len() && b[k] == b' ' {
                k += 1;
            }
            let kind = if KEYWORDS.contains(&word) {
                Kind::Keyword
            } else if CONSTANTS.contains(&word) {
                Kind::Constant
            } else if b.get(k) == Some(&b'(') {
                Kind::Function
            } else {
                Kind::Plain
            };
            push(i, j, kind, &mut out);
            i = j;
        } else {
            // One character (keeping UTF-8 sequences whole).
            let len = text[i..].chars().next().map_or(1, char::len_utf8);
            push(i, i + len, Kind::Plain, &mut out);
            i += len;
        }
    }
    out
}

/// Colours of the span kinds.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub plain: Color32,
    pub keyword: Color32,
    pub constant: Color32,
    pub string: Color32,
    pub number: Color32,
    pub comment: Color32,
    pub function: Color32,
    pub find: Color32,
}

impl Palette {
    fn color(&self, k: Kind) -> Color32 {
        match k {
            Kind::Plain => self.plain,
            Kind::Keyword => self.keyword,
            Kind::Constant => self.constant,
            Kind::String => self.string,
            Kind::Number => self.number,
            Kind::Comment => self.comment,
            Kind::Function => self.function,
        }
    }
}

/// The coloured layout of `text`; byte ranges in `marks` (find matches) get
/// a background.
pub fn layout_job(text: &str, font: FontId, p: &Palette, marks: &[(usize, usize)]) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (s, e, k) in tokenize(text) {
        // Split the span where marks begin and end.
        let mut cuts = vec![s, e];
        for &(ms, me) in marks {
            for c in [ms, me] {
                if c > s && c < e && text.is_char_boundary(c) {
                    cuts.push(c);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, z) = (w[0], w[1]);
            let marked = marks.iter().any(|&(ms, me)| a >= ms && z <= me);
            job.append(
                &text[a..z],
                0.0,
                TextFormat {
                    font_id: font.clone(),
                    color: p.color(k),
                    background: if marked { p.find } else { Color32::TRANSPARENT },
                    ..Default::default()
                },
            );
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&str, Kind)> {
        tokenize(text)
            .into_iter()
            .filter(|(_, _, k)| *k != Kind::Plain)
            .map(|(s, e, k)| (&text[s..e], k))
            .collect()
    }

    #[test]
    fn luau_tokens() {
        let k = kinds(
            "local x = 3.5 -- note\nif self.ok then print(\"hi\") end --[[ a\nb ]] y = [[raw]]",
        );
        assert_eq!(
            k,
            vec![
                ("local", Kind::Keyword),
                ("3.5", Kind::Number),
                ("-- note", Kind::Comment),
                ("if", Kind::Keyword),
                ("self", Kind::Constant),
                ("then", Kind::Keyword),
                ("print", Kind::Function),
                ("\"hi\"", Kind::String),
                ("end", Kind::Keyword),
                ("--[[ a\nb ]]", Kind::Comment),
                ("[[raw]]", Kind::String),
            ]
        );
        // Spans cover the text exactly, unicode included.
        let text = "-- é\nlocal s = `ü{1}`";
        let spans = tokenize(text);
        assert_eq!(spans.first().unwrap().0, 0);
        assert_eq!(spans.last().unwrap().1, text.len());
        assert!(spans.windows(2).all(|w| w[0].1 == w[1].0));
    }

    #[test]
    fn marks_split_spans() {
        let p = Palette {
            plain: Color32::WHITE,
            keyword: Color32::RED,
            constant: Color32::RED,
            string: Color32::GREEN,
            number: Color32::BLUE,
            comment: Color32::GRAY,
            function: Color32::YELLOW,
            find: Color32::DARK_BLUE,
        };
        let job = layout_job("local speed", FontId::monospace(12.0), &p, &[(8, 11)]);
        let marked: String = job
            .sections
            .iter()
            .filter(|s| s.format.background == Color32::DARK_BLUE)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(marked, "eed");
    }
}
