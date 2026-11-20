//! Minimal HTML → Markdown conversion, matching `markdownify` (the library the
//! Python `scrape` path uses) for the elements that appear in page text.
//!
//! Verified against markdownify's output: `h1` uses `===` underlines, `h2` uses
//! `---`, `h3`+ use `#` prefixes, unordered lists use `*`/`+` by depth, ordered
//! lists use `1.`, and `<br>` becomes a Markdown hard break (two spaces).

/// Converts HTML to Markdown.
pub fn html_to_markdown(html: &str) -> String {
    let mut converter = Converter::new();
    converter.walk(html);
    converter.finish()
}

struct ListFrame {
    ordered: bool,
    counter: usize,
    depth: usize,
}

struct Converter {
    output: String,
    skip_depth: usize,
    lists: Vec<ListFrame>,
    in_pre: bool,
    pending_space: bool,
    pending_href: Option<String>,
    pending_heading: Option<String>,
    heading_level: usize,
}

impl Converter {
    fn new() -> Self {
        Self {
            output: String::new(),
            skip_depth: 0,
            lists: Vec::new(),
            in_pre: false,
            pending_space: false,
            pending_href: None,
            pending_heading: None,
            heading_level: 1,
        }
    }

    fn finish(mut self) -> String {
        if let Some(heading) = self.pending_heading.take() {
            self.emit_heading(&heading);
        }
        while self.output.ends_with("\n\n") {
            self.output.pop();
        }
        self.output.trim().to_string()
    }

    /// Emits `===`/`---`/`###` heading markup for the finished heading text.
    fn emit_heading(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        self.newline(2);
        match self.heading_level {
            1 => {
                self.output.push_str(text);
                self.output.push('\n');
                self.output.push_str(&"=".repeat(text.chars().count()));
            }
            2 => {
                self.output.push_str(text);
                self.output.push('\n');
                self.output.push_str(&"-".repeat(text.chars().count()));
            }
            level => {
                for _ in 0..level {
                    self.output.push('#');
                }
                self.output.push(' ');
                self.output.push_str(text);
            }
        }
        self.newline(2);
    }

    fn push_text(&mut self, text: &str) {
        if self.in_pre {
            self.output.push_str(text);
            return;
        }
        if self.pending_heading.is_some() {
            let collapsed = collapse_whitespace(text);
            if let Some(heading) = self.pending_heading.as_mut() {
                heading.push_str(collapsed.trim());
            }
            return;
        }
        let collapsed = collapse_whitespace(text);
        if collapsed.trim().is_empty() {
            if !self.output.is_empty() && !self.output.ends_with(['\n', ' ']) {
                self.pending_space = true;
            }
            return;
        }
        // A leading space implies a separator when nothing already separates; the
        // separator is emitted here and stripped from the text so it appears once.
        let needs_leading = collapsed.starts_with(' ')
            && !self.output.is_empty()
            && !self.output.ends_with(['\n', ' ']);
        if needs_leading {
            self.output.push(' ');
            self.pending_space = false;
        } else {
            self.flush_pending_space();
        }
        self.output.push_str(collapsed.trim());
        self.pending_space = collapsed.ends_with(' ');
    }

    /// Emits a buffered separator space before an inline element opens.
    fn flush_pending_space(&mut self) {
        if self.pending_space
            && !self.output.is_empty()
            && !self.output.ends_with(['\n', ' '])
        {
            self.output.push(' ');
        }
        self.pending_space = false;
    }

    fn newline(&mut self, count: usize) {
        if self.in_pre || self.skip_depth > 0 {
            return;
        }
        // Drop trailing spaces/tabs but keep newlines: they are structural.
        let trimmed_len = self.output.trim_end_matches([' ', '\t']).len();
        self.output.truncate(trimmed_len);
        self.pending_space = false;
        if self.output.is_empty() {
            return;
        }
        let existing = self.output.chars().rev().take_while(|c| *c == '\n').count();
        for _ in existing..count {
            self.output.push('\n');
        }
    }

    fn walk(&mut self, html: &str) {
        let lower = html.to_lowercase();
        let bytes = html.as_bytes();
        let mut cursor = 0usize;

        while cursor < bytes.len() {
            if bytes[cursor] != b'<' {
                let next = html[cursor..]
                    .find('<')
                    .map(|offset| cursor + offset)
                    .unwrap_or(bytes.len());
                let decoded = decode_entities(&html[cursor..next]);
                self.push_text(&decoded);
                cursor = next;
                continue;
            }

            let Some(close) = html[cursor..].find('>') else {
                let decoded = decode_entities(&html[cursor..]);
                self.push_text(&decoded);
                break;
            };
            let tag_text = &html[cursor + 1..cursor + close];
            let consumed = cursor + close + 1;

            if tag_text.starts_with('!') || tag_text.starts_with('?') {
                cursor = consumed;
                continue;
            }

            let (is_close, tag_name, remainder) = parse_tag(tag_text);

            match tag_name.as_str() {
                "script" | "style" | "noscript" | "head" => {
                    if !is_close {
                        let closing = format!("</{tag_name}");
                        match lower[consumed..].find(&closing) {
                            Some(offset) => {
                                let end = consumed + offset;
                                cursor = html[end..]
                                    .find('>')
                                    .map(|value| end + value + 1)
                                    .unwrap_or(html.len());
                            }
                            None => break,
                        }
                        continue;
                    }
                }
                "pre" => {
                    if is_close {
                        self.in_pre = false;
                        self.newline(1);
                        self.output.push_str("```");
                        self.newline(2);
                    } else {
                        self.newline(2);
                        self.output.push_str("```\n");
                        self.in_pre = true;
                    }
                }
                "br" => {
                    // Markdown hard break: two trailing spaces before the newline.
                    while self.output.ends_with(' ') {
                        self.output.pop();
                    }
                    self.output.push_str("  \n");
                }
                "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav"
                | "blockquote" | "table" => {
                    if is_close {
                        self.newline(2);
                    }
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    let level = tag_name[1..].parse::<usize>().unwrap_or(1);
                    if is_close {
                        if let Some(text) = self.pending_heading.take() {
                            self.heading_level = level;
                            self.emit_heading(&text);
                        }
                    } else {
                        self.newline(2);
                        self.pending_heading = Some(String::new());
                        self.heading_level = level;
                    }
                }
                "ul" | "ol" => {
                    if is_close {
                        self.lists.pop();
                        if self.lists.is_empty() {
                            self.newline(2);
                        } else {
                            self.newline(1);
                        }
                    } else {
                        // A list after text starts a new block; a nested list does not.
                        if self.lists.is_empty() {
                            self.newline(2);
                        } else {
                            self.newline(1);
                        }
                        let depth = self.lists.len();
                        self.lists.push(ListFrame {
                            ordered: tag_name == "ol",
                            counter: 0,
                            depth,
                        });
                    }
                }
                "li" => {
                    if is_close {
                        self.newline(1);
                    } else {
                        self.newline(1);
                        if let Some(frame) = self.lists.last_mut() {
                            let indent = "  ".repeat(frame.depth);
                            let marker = if frame.ordered {
                                frame.counter += 1;
                                format!("{}. ", frame.counter)
                            } else {
                                // Nested unordered lists alternate `+` per depth.
                                if frame.depth % 2 == 0 { "* " } else { "+ " }.to_string()
                            };
                            self.output.push_str(&indent);
                            self.output.push_str(&marker);
                            self.pending_space = false;
                        }
                    }
                }
                "tr" => {
                    if is_close {
                        self.output.push('\n');
                    }
                }
                "td" | "th" => {
                    if is_close {
                        self.output.push_str(" | ");
                    } else {
                        self.output.push_str("| ");
                    }
                }
                "a" => {
                    if is_close {
                        if let Some(href) = self.pending_href.take() {
                            self.output.push_str(&format!("]({href})"));
                        }
                    } else if let Some(href) = attribute(&remainder, "href") {
                        self.flush_pending_space();
                        self.output.push('[');
                        self.pending_href = Some(href);
                    }
                }
                "strong" | "b" => {
                    if is_close {
                        self.output.push_str("**");
                    } else {
                        self.flush_pending_space();
                        self.output.push_str("**");
                    }
                }
                "em" | "i" => {
                    if is_close {
                        self.output.push('*');
                    } else {
                        self.flush_pending_space();
                        self.output.push('*');
                    }
                }
                "code" => {
                    if !self.in_pre {
                        if !is_close {
                            self.flush_pending_space();
                        }
                        self.output.push('`');
                    }
                }
                "img" => {
                    if let Some(alt) = attribute(&remainder, "alt") {
                        if !alt.is_empty() {
                            self.flush_pending_space();
                            self.output.push_str(&alt);
                        }
                    }
                }
                _ => {}
            }
            cursor = consumed;
        }
    }
}

fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut previous_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !previous_space {
                result.push(' ');
                previous_space = true;
            }
        } else {
            result.push(character);
            previous_space = false;
        }
    }
    if text.starts_with(|character: char| character.is_whitespace()) {
        result
    } else {
        result.trim_start_matches(' ').to_string()
    }
}

fn parse_tag(tag: &str) -> (bool, String, String) {
    let trimmed = tag.trim();
    let is_close = trimmed.starts_with('/');
    let content = trimmed.trim_start_matches('/').trim_start();
    let name_end = content
        .find(|character: char| character.is_whitespace() || character == '/')
        .unwrap_or(content.len());
    let name = content[..name_end].to_lowercase();
    let remainder = content[name_end..].to_string();
    (is_close, name, remainder)
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let pattern = format!("{name}=");
    let index = lower.find(&pattern)?;
    let mut rest = tag[index + pattern.len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        rest = &rest[1..];
        let end = rest.find(quote)?;
        Some(decode_entities(&rest[..end]))
    } else {
        let end = rest
            .find(|character: char| character.is_whitespace())
            .unwrap_or(rest.len());
        Some(decode_entities(&rest[..end]))
    }
}

/// Decodes the common named entities and numeric references.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < text.len() {
        let rest = &text[index..];
        if !rest.starts_with('&') {
            let character = rest.chars().next().unwrap();
            result.push(character);
            index += character.len_utf8();
            continue;
        }
        let Some(end) = rest.find(';') else {
            result.push('&');
            index += 1;
            continue;
        };
        if end > 10 {
            result.push('&');
            index += 1;
            continue;
        }
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "copy" => Some('©'),
            "reg" => Some('®'),
            "trade" => Some('™'),
            _ => {
                let number = match entity.strip_prefix('#') {
                    Some(number) => number,
                    None => {
                        result.push('&');
                        index += 1;
                        continue;
                    }
                };
                let code = if let Some(hex) = number.strip_prefix('x').or_else(|| number.strip_prefix('X')) {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    number.parse::<u32>().ok()
                };
                code.and_then(char::from_u32)
            }
        };
        match decoded {
            Some(value) => {
                result.push(value);
                index += end + 1;
            }
            None => {
                result.push('&');
                index += 1;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_follow_markdownify() {
        assert_eq!(html_to_markdown("<h1>One</h1>"), "One\n===");
        assert_eq!(html_to_markdown("<h2>Two</h2>"), "Two\n---");
        assert_eq!(html_to_markdown("<h3>Three</h3>"), "### Three");
        assert_eq!(html_to_markdown("<h6>Six</h6>"), "###### Six");
    }

    #[test]
    fn paragraphs_are_separated_by_blank_lines() {
        assert_eq!(html_to_markdown("<p>A</p><p>B</p>"), "A\n\nB");
    }

    #[test]
    fn inline_elements_keep_spacing() {
        assert_eq!(
            html_to_markdown("<p>Hello <a href=\"https://x.com\">world</a>.</p>"),
            "Hello [world](https://x.com)."
        );
        assert_eq!(html_to_markdown("<p>a <strong>b</strong> c</p>"), "a **b** c");
        assert_eq!(html_to_markdown("<p>a <em>b</em> c</p>"), "a *b* c");
        assert_eq!(html_to_markdown("<p>use <code>x=1</code> now</p>"), "use `x=1` now");
    }

    #[test]
    fn lists_match_markdownify() {
        assert_eq!(html_to_markdown("<ul><li>One</li><li>Two</li></ul>"), "* One\n* Two");
        assert_eq!(html_to_markdown("<ol><li>One</li><li>Two</li></ol>"), "1. One\n2. Two");
        assert_eq!(
            html_to_markdown("<ul><li>A<ul><li>B</li></ul></li></ul>"),
            "* A\n  + B"
        );
    }

    #[test]
    fn pre_blocks_are_fenced() {
        assert_eq!(
            html_to_markdown("<pre>line1\nline2</pre>"),
            "```\nline1\nline2\n```"
        );
    }

    #[test]
    fn br_is_a_hard_break() {
        assert_eq!(html_to_markdown("<p>a<br>b</p>"), "a  \nb");
    }

    #[test]
    fn scripts_are_dropped() {
        assert_eq!(html_to_markdown("<script>x=1</script><p>Body</p>"), "Body");
    }

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &#65;"), "a & b <c> A");
    }

    #[test]
    fn combined_document_matches_markdownify() {
        let html = "<h1>Title</h1><p>Hello <a href=\"https://example.com\">world</a></p><ul><li>One</li><li>Two</li></ul>";
        assert_eq!(
            html_to_markdown(html),
            "Title\n=====\n\nHello [world](https://example.com)\n\n* One\n* Two"
        );
    }
}
