//! Rich text for providers that store it as HTML.
//!
//! Azure DevOps keeps `System.Description` as HTML written by its own editor —
//! mostly `<div>` per line, `<br>`, lists, bold and links. The harness renders
//! descriptions as Markdown, so this translates the handful of tags that carry
//! meaning and drops the rest. It is not an HTML parser and does not try to be:
//! a description that reads sensibly is the goal, not a faithful round-trip.

use std::borrow::Cow;

/// HTML → Markdown, keeping structure (paragraphs, lists, headings, code,
/// links, emphasis) and discarding presentation.
pub(crate) fn to_markdown(html: &str) -> String {
    let mut w = Writer::default();
    let mut rest = html;
    while !rest.is_empty() {
        match rest.find('<') {
            Some(0) => {
                if let Some(after) = rest.strip_prefix("<!--") {
                    rest = after.find("-->").map_or("", |i| &after[i + 3..]);
                    continue;
                }
                let Some(end) = rest.find('>') else {
                    w.text(rest);
                    break;
                };
                let tag = &rest[1..end];
                rest = &rest[end + 1..];
                if let Some(skipped) = w.tag(tag) {
                    // Script and style bodies are code, not prose.
                    let close = format!("</{skipped}");
                    rest = match rest.to_ascii_lowercase().find(&close) {
                        Some(i) => rest[i..].find('>').map_or("", |j| &rest[i + j + 1..]),
                        None => "",
                    };
                }
            }
            Some(i) => {
                w.text(&rest[..i]);
                rest = &rest[i..];
            }
            None => {
                w.text(rest);
                break;
            }
        }
    }
    w.finish()
}

/// Plain text → the HTML Azure DevOps' own editor would have produced: one
/// `<div>` per line, an empty line as `<div><br></div>`.
pub(crate) fn from_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                "<div><br></div>".to_string()
            } else {
                format!("<div>{}</div>", escape(line))
            }
        })
        .collect()
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[derive(Default)]
struct Writer {
    out: String,
    /// Open lists, innermost last: `None` for bullets, `Some(n)` for a
    /// numbered list whose last item was `n`.
    lists: Vec<Option<u32>>,
    /// Open links, innermost last; `None` for an anchor with no `href`, which
    /// emitted nothing on the way in and must emit nothing on the way out.
    links: Vec<Option<String>>,
    pre: u32,
}

impl Writer {
    fn at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n')
    }

    fn trim_trailing_spaces(&mut self) {
        let kept = self.out.trim_end_matches([' ', '\t']).len();
        self.out.truncate(kept);
    }

    fn text(&mut self, raw: &str) {
        let decoded = decode_entities(raw);
        if self.pre > 0 {
            self.out.push_str(&decoded);
            return;
        }
        // HTML whitespace collapses; a non-breaking space is deliberate.
        let mut pending_space = false;
        for ch in decoded.chars() {
            if ch.is_whitespace() && ch != '\u{a0}' {
                pending_space = true;
                continue;
            }
            if pending_space && !self.at_line_start() && !self.out.ends_with(' ') {
                self.out.push(' ');
            }
            pending_space = false;
            self.out.push(if ch == '\u{a0}' { ' ' } else { ch });
        }
        if pending_space && !self.at_line_start() && !self.out.ends_with(' ') {
            self.out.push(' ');
        }
    }

    /// End the current line.
    fn line(&mut self) {
        self.trim_trailing_spaces();
        if !self.at_line_start() {
            self.out.push('\n');
        }
    }

    /// End the current paragraph. Inside a list a paragraph break would split
    /// the list in two, so it is only a line there.
    fn block(&mut self) {
        if !self.lists.is_empty() {
            return self.line();
        }
        self.trim_trailing_spaces();
        if self.out.is_empty() {
            return;
        }
        let newlines = self.out.len() - self.out.trim_end_matches('\n').len();
        for _ in newlines..2 {
            self.out.push('\n');
        }
    }

    fn hard_break(&mut self) {
        if self.pre > 0 {
            self.out.push('\n');
            return;
        }
        self.trim_trailing_spaces();
        if self.out.is_empty() {
            return;
        }
        if self.out.ends_with('\n') {
            // A second break in a row is an empty line: a paragraph break.
            if !self.out.ends_with("\n\n") {
                self.out.push('\n');
            }
        } else {
            self.out.push_str("  \n");
        }
    }

    /// Apply one tag. Returns the name of an element whose content must be
    /// skipped entirely.
    fn tag(&mut self, raw: &str) -> Option<&'static str> {
        let raw = raw.trim();
        let closing = raw.starts_with('/');
        let body = raw.trim_start_matches('/');
        let name_end = body
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(body.len());
        let name = body[..name_end].to_ascii_lowercase();
        let attrs = &body[name_end..];

        if let Some(level) = heading_level(&name) {
            self.block();
            if !closing {
                self.out.push_str(&"#".repeat(level));
                self.out.push(' ');
            }
            return None;
        }

        match (name.as_str(), closing) {
            ("script", false) => return Some("script"),
            ("style", false) => return Some("style"),
            ("br", _) => self.hard_break(),
            ("p" | "div" | "table" | "blockquote" | "section" | "article", _) => self.block(),
            ("tr", true) => self.line(),
            ("td" | "th", true) if !self.at_line_start() && !self.out.ends_with(' ') => {
                self.out.push(' ');
            }
            ("ul" | "ol", false) => {
                if self.lists.is_empty() {
                    self.block();
                } else {
                    self.line();
                }
                self.lists.push((name == "ol").then_some(0));
            }
            ("ul" | "ol", true) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.block();
                }
            }
            ("li", false) => {
                self.line();
                let depth = self.lists.len().max(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{n}. ")
                    }
                    _ => "- ".to_string(),
                };
                self.out.push_str(&"  ".repeat(depth - 1));
                self.out.push_str(&marker);
            }
            ("strong" | "b", _) => self.out.push_str("**"),
            ("em" | "i", _) => self.out.push('_'),
            ("code", _) if self.pre == 0 => self.out.push('`'),
            ("pre", false) => {
                self.block();
                self.out.push_str("```\n");
                self.pre += 1;
            }
            ("pre", true) => {
                self.pre = self.pre.saturating_sub(1);
                if !self.out.ends_with('\n') {
                    self.out.push('\n');
                }
                self.out.push_str("```");
                self.block();
            }
            ("a", false) => {
                let href = attr(attrs, "href");
                if href.is_some() {
                    self.out.push('[');
                }
                self.links.push(href);
            }
            ("a", true) => {
                if let Some(Some(href)) = self.links.pop() {
                    self.out.push_str("](");
                    self.out.push_str(&href);
                    self.out.push(')');
                }
            }
            ("img", _) => {
                // Attachments sit behind the organization's sign-in, so an
                // inline image would never load; a link to it still opens.
                if let Some(src) = attr(attrs, "src") {
                    let alt = attr(attrs, "alt").unwrap_or_else(|| "image".to_string());
                    self.out.push_str(&format!("[{alt}]({src})"));
                }
            }
            ("hr", _) => {
                self.block();
                self.out.push_str("---");
                self.block();
            }
            _ => {}
        }
        None
    }

    fn finish(self) -> String {
        // Collapse runs of empty lines left behind by nested blocks.
        let mut out = String::with_capacity(self.out.len());
        let mut blank_run = 0;
        for line in self.out.trim().lines() {
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run > 1 {
                    continue;
                }
            } else {
                blank_run = 0;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
        out.trim_end().to_string()
    }
}

fn heading_level(name: &str) -> Option<usize> {
    let digit = name.strip_prefix('h')?;
    match digit {
        "1" | "2" | "3" | "4" | "5" | "6" => digit.parse().ok(),
        _ => None,
    }
}

/// An attribute's value, entity-decoded. `None` when absent or empty.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let needle = format!("{name}=");
    let mut from = 0;
    while let Some(found) = lower[from..].find(&needle) {
        let start = from + found;
        from = start + needle.len();
        // `data-href=` must not match `href=`.
        if start > 0 && !lower.as_bytes()[start - 1].is_ascii_whitespace() {
            continue;
        }
        let value = &attrs[from..];
        let raw = match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or(""),
            _ => value.split(char::is_whitespace).next().unwrap_or(""),
        };
        let decoded = decode_entities(raw).trim().to_string();
        return (!decoded.is_empty()).then_some(decoded);
    }
    None
}

fn decode_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest
            .find(';')
            .filter(|&semi| semi <= 10)
            .and_then(|semi| entity(&rest[1..semi]).map(|ch| (ch, semi)));
        match decoded {
            Some((ch, semi)) => {
                out.push(ch);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{from_text, to_markdown};

    #[test]
    fn the_editors_div_per_line_becomes_paragraphs() {
        let html = "<div>First line</div><div><br></div><div>Second <b>bold</b> line</div>";
        assert_eq!(to_markdown(html), "First line\n\nSecond **bold** line");
    }

    #[test]
    fn a_single_break_is_a_hard_line_break() {
        assert_eq!(to_markdown("one<br>two"), "one  \ntwo");
    }

    #[test]
    fn lists_keep_their_markers_and_nesting() {
        let html = "<ul><li>a</li><li>b<ol><li>x</li><li>y</li></ol></li></ul><p>after</p>";
        assert_eq!(to_markdown(html), "- a\n- b\n  1. x\n  2. y\n\nafter");
    }

    #[test]
    fn links_and_headings_survive() {
        let html = r#"<h2>Plan</h2><p>See <a href="https://x.test/a?b=1&amp;c=2">the doc</a>.</p>"#;
        assert_eq!(
            to_markdown(html),
            "## Plan\n\nSee [the doc](https://x.test/a?b=1&c=2)."
        );
    }

    #[test]
    fn entities_decode_and_whitespace_collapses() {
        assert_eq!(
            to_markdown("<p>a &lt;b&gt;\n   &amp;&nbsp;c &#8217;d&#x21;</p>"),
            "a <b> & c \u{2019}d!"
        );
    }

    #[test]
    fn code_blocks_keep_their_whitespace() {
        let html = "<pre>fn main() {\n    run();\n}</pre>";
        assert_eq!(to_markdown(html), "```\nfn main() {\n    run();\n}\n```");
    }

    #[test]
    fn scripts_styles_and_comments_are_dropped() {
        let html = "<style>p{color:red}</style><!-- note --><p>kept</p><script>alert(1)</script>";
        assert_eq!(to_markdown(html), "kept");
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(to_markdown("just words"), "just words");
        assert_eq!(to_markdown(""), "");
    }

    #[test]
    fn text_becomes_escaped_divs() {
        assert_eq!(
            from_text("a < b\n\nc"),
            "<div>a &lt; b</div><div><br></div><div>c</div>"
        );
    }
}
