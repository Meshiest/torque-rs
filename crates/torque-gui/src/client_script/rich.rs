//! The rich-text markup of the `richtext` control: `<b>`, `<i>`, `<color:rrggbb[aa]>`, `<size:N>`, `<align:left|center|right>`
//! and `<br>`, with `<<` for a literal `<`. The format checks text before it is applied; this reader still takes any text and
//! never fails: a tag it does not know is dropped and its text stays.

/// How a line sits in the control's width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// A stretch of text drawn one way. Bitmap fonts have no italic, so `<i>` changes nothing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    pub text: String,
    pub color: Option<[u8; 4]>,
    pub size: Option<i32>,
    pub bold: bool,
}

/// One line of runs and the width of each.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Line {
    pub align: Align,
    pub runs: Vec<(Run, i32)>,
}

impl Line {
    pub(crate) fn width(&self) -> i32 {
        self.runs.iter().map(|(_, w)| *w).sum()
    }

    /// The tallest run's size, or `default` for an empty line.
    pub(crate) fn size(&self, default: i32) -> i32 {
        self.runs
            .iter()
            .map(|(run, _)| run.size.unwrap_or(default))
            .max()
            .unwrap_or(default)
    }
}

/// The text's style at one point: the open tags.
#[derive(Clone, Debug, Default)]
struct Style {
    color: Option<[u8; 4]>,
    size: Option<i32>,
    bold: bool,
}

enum Open {
    Bold,
    Italic,
    Color([u8; 4]),
    Size(i32),
}

fn hex_color(hex: &str) -> Option<[u8; 4]> {
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if hex.len() == 8 { byte(6)? } else { 255 },
    ])
}

struct Layout<F> {
    lines: Vec<Line>,
    /// The width of the last line so far.
    line_width: i32,
    width: Option<i32>,
    measure: F,
    stack: Vec<Open>,
}

impl<F: FnMut(&str, i32) -> i32> Layout<F> {
    fn style(&self) -> Style {
        let mut style = Style::default();
        for open in &self.stack {
            match open {
                Open::Bold => style.bold = true,
                Open::Italic => {}
                Open::Color(c) => style.color = Some(*c),
                Open::Size(s) => style.size = Some(*s),
            }
        }
        style
    }

    fn line(&mut self) -> &mut Line {
        if self.lines.is_empty() {
            self.lines.push(Line::default());
        }
        let last = self.lines.len() - 1;
        &mut self.lines[last]
    }

    fn new_line(&mut self) {
        let align = self.line().align;
        self.lines.push(Line {
            align,
            runs: Vec::new(),
        });
        self.line_width = 0;
    }

    /// Adds `word` in the current style, wrapping to a new line when it does not fit. The word is measured once.
    fn word(&mut self, word: &str, default_size: i32) {
        let style = self.style();
        let size = style.size.unwrap_or(default_size);
        let width = (self.measure)(word, size);
        if let Some(limit) = self.width
            && self.line_width.saturating_add(width) > limit
            && !self.line().runs.is_empty()
        {
            self.new_line();
        }
        self.line_width = self.line_width.saturating_add(width);
        self.line().runs.push((
            Run {
                text: word.to_string(),
                color: style.color,
                size: style.size,
                bold: style.bold,
            },
            width,
        ));
    }

    fn text(&mut self, text: &str, default_size: i32) {
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                self.new_line();
            }
            for word in part.split_inclusive(' ') {
                self.word(word, default_size);
            }
        }
    }

    fn tag(&mut self, tag: &str) {
        if let Some(name) = tag.strip_prefix('/') {
            let wanted = |open: &Open| {
                matches!(
                    (name, open),
                    ("b", Open::Bold)
                        | ("i", Open::Italic)
                        | ("color", Open::Color(_))
                        | ("size", Open::Size(_))
                )
            };
            if let Some(at) = self.stack.iter().rposition(wanted) {
                self.stack.remove(at);
            }
            return;
        }
        let (name, arg) = tag.split_once(':').unwrap_or((tag, ""));
        match name {
            "b" => self.stack.push(Open::Bold),
            "i" => self.stack.push(Open::Italic),
            "color" => {
                if let Some(c) = hex_color(arg) {
                    self.stack.push(Open::Color(c));
                }
            }
            "size" => {
                if let Ok(n) = arg.parse::<i32>() {
                    self.stack.push(Open::Size(n.clamp(8, 72)));
                }
            }
            "align" => {
                let align = match arg {
                    "center" => Align::Center,
                    "right" => Align::Right,
                    _ => Align::Left,
                };
                self.line().align = align;
            }
            "br" => self.new_line(),
            _ => {}
        }
    }
}

/// Lays `text` out in lines: broken at `<br>` and newlines and, with a `wrap` width, between words. `measure` gives the width
/// of a word at a size; `default_size` is the control's own.
pub(crate) fn layout(
    text: &str,
    wrap: Option<i32>,
    default_size: i32,
    measure: impl FnMut(&str, i32) -> i32,
) -> Vec<Line> {
    let mut layout = Layout {
        lines: vec![Line::default()],
        line_width: 0,
        width: wrap,
        measure,
        stack: Vec::new(),
    };
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        layout.text(&rest[..at], default_size);
        rest = &rest[at + 1..];
        if let Some(after) = rest.strip_prefix('<') {
            layout.text("<", default_size);
            rest = after;
            continue;
        }
        let Some(end) = rest.find('>') else {
            break;
        };
        layout.tag(&rest[..end]);
        rest = &rest[end + 1..];
    }
    layout.text(rest, default_size);
    layout.lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.runs.iter().map(|(r, _)| r.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn markup_sets_style_and_breaks_lines() {
        let lines = layout("a<b>b</b><br>c<color:ff0000>d", None, 14, |t, _| {
            t.len() as i32
        });
        assert_eq!(flat(&lines), ["ab", "cd"]);
        assert!(lines[0].runs[1].0.bold && !lines[0].runs[0].0.bold);
        assert_eq!(lines[1].runs[1].0.color, Some([255, 0, 0, 255]));
    }

    #[test]
    fn a_double_bracket_is_one_bracket_and_unknown_tags_vanish() {
        let lines = layout("1 << 2 <a:x>link", None, 14, |t, _| t.len() as i32);
        assert_eq!(flat(&lines), ["1 < 2 link"]);
    }

    #[test]
    fn words_wrap_at_the_width() {
        let lines = layout("aa bb cc", Some(6), 14, |t, _| t.len() as i32);
        assert_eq!(flat(&lines), ["aa bb ", "cc"]);
    }

    #[test]
    fn each_word_is_measured_once() {
        let words = 2000;
        let text = "ab ".repeat(words);
        let mut calls = 0;
        let lines = layout(&text, Some(30), 14, |t, _| {
            calls += 1;
            t.len() as i32
        });
        assert!(lines.len() > 100);
        assert!(calls <= words + lines.len(), "{calls} measures");
    }

    #[test]
    fn any_text_lays_out() {
        for text in [
            "<",
            "<<<",
            "<size:9999>x",
            "</b>",
            "<align:center>a<br><align:right>b",
            "<color:zz>",
        ] {
            assert!(!layout(text, Some(10), 14, |t, _| t.len() as i32).is_empty());
        }
    }
}
