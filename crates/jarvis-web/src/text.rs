//! Reducing a fetched body to text a model can read.
//!
//! Deliberately small. It removes what is never visible text — comments, `script`, `style` — turns block-level
//! tags into line breaks, drops every other tag, decodes character references, and collapses whitespace. It is not
//! an HTML parser and makes no claim to be one: the output is fenced untrusted text either way, so a page this
//! mangles is a worse summary, never a security problem.

/// What a response body is, judged from its `Content-Type`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// HTML, reduced to visible text.
    Html,
    /// Other text, used as it is.
    Text,
    /// Not text. Reported by type and size, never decoded.
    Other,
}

/// Classifies a `Content-Type` header value. A missing header is [`Kind::Other`]: guessing would decode bytes the
/// server did not say were text.
#[must_use]
pub fn classify(content_type: Option<&str>) -> Kind {
    let Some(value) = content_type else {
        return Kind::Other;
    };
    let media = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match media.as_str() {
        "text/html" | "application/xhtml+xml" => Kind::Html,
        "application/json" | "application/xml" => Kind::Text,
        other if other.starts_with("text/") => Kind::Text,
        other if other.ends_with("+json") || other.ends_with("+xml") => Kind::Text,
        _ => Kind::Other,
    }
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "br",
    "dd",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "nav",
    "ol",
    "p",
    "pre",
    "section",
    "table",
    "td",
    "th",
    "tr",
    "ul",
];

/// Reduces HTML to its visible text.
#[must_use]
pub fn html_to_text(html: &str) -> String {
    let mut stripped = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(open) = next_tag(rest) {
        stripped.push_str(&rest[..open]);
        rest = &rest[open..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else {
            // An unterminated tag swallows the remainder, which is what a browser does too.
            rest = "";
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let closing = tag.starts_with('/');
        let name = tag_name(tag);
        if !closing && (name == "script" || name == "style") {
            rest = skip_past_closing(rest, &name);
            continue;
        }
        if BLOCK_TAGS.contains(&name.as_str()) {
            stripped.push('\n');
        }
    }
    stripped.push_str(rest);
    collapse(&decode_references(&stripped))
}

/// Normalizes plain text: line endings and runs of blank space.
#[must_use]
pub fn plain_to_text(text: &str) -> String {
    collapse(text)
}

fn next_tag(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = text[from..].find('<') {
        let at = from + offset;
        // `<` followed by a space or digit is text ("a < b"), not the start of a tag.
        let next = text[at + 1..].chars().next();
        if next.is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!') {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn skip_past_closing<'a>(text: &'a str, name: &str) -> &'a str {
    // ASCII lowercasing keeps byte offsets, so an index into the lowered copy is valid in the original.
    let lowered = text.to_ascii_lowercase();
    let needle = format!("</{name}");
    let Some(start) = lowered.find(&needle) else {
        return "";
    };
    match text[start..].find('>') {
        Some(end) => &text[start + end + 1..],
        None => "",
    }
}

fn decode_references(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest[1..]
            .find(';')
            .filter(|end| *end <= 10)
            .and_then(|end| decode_one(&rest[1..=end]).map(|c| (c, end + 2)));
        if let Some((character, consumed)) = decoded {
            out.push(character);
            rest = &rest[consumed..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn decode_one(reference: &str) -> Option<char> {
    let reference = reference.strip_suffix(';').unwrap_or(reference);
    if let Some(number) = reference.strip_prefix('#') {
        let value = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(value).filter(|c| !c.is_control());
    }
    match reference {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => None,
    }
}

fn collapse(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let words = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !words.is_empty() {
            lines.push(words);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_are_classified_and_a_missing_one_is_not_guessed() {
        assert_eq!(classify(Some("text/html; charset=utf-8")), Kind::Html);
        assert_eq!(classify(Some("Text/HTML")), Kind::Html);
        assert_eq!(classify(Some("text/plain")), Kind::Text);
        assert_eq!(classify(Some("application/json")), Kind::Text);
        assert_eq!(classify(Some("application/ld+json")), Kind::Text);
        assert_eq!(classify(Some("application/atom+xml")), Kind::Text);
        assert_eq!(classify(Some("application/pdf")), Kind::Other);
        assert_eq!(classify(Some("image/png")), Kind::Other);
        assert_eq!(classify(None), Kind::Other);
    }

    #[test]
    fn scripts_styles_and_comments_are_removed_and_blocks_break_lines() {
        let html = "<html><head><style>p{color:red}</style><script type=\"x\">alert('<b>')</script>\
                    </head><body><!-- hidden --><h1>Title</h1><p>He<b>llo</b> &amp; welcome</p>\
                    <ul><li>one</li><li>two</li></ul></body></html>";
        assert_eq!(html_to_text(html), "Title\nHello & welcome\none\ntwo");
    }

    #[test]
    fn a_closing_script_tag_is_matched_without_regard_to_case() {
        assert_eq!(html_to_text("a<SCRIPT>var x = 1;</ScRiPt>b"), "ab");
        // No closing tag: everything after the opening tag is script, which is what a browser does.
        assert_eq!(html_to_text("a<script>never closed"), "a");
    }

    #[test]
    fn a_bare_less_than_is_text_and_an_unterminated_tag_is_dropped() {
        assert_eq!(html_to_text("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
        assert_eq!(html_to_text("kept <div class=\"never ends"), "kept");
    }

    #[test]
    fn character_references_are_decoded_once_and_only_when_well_formed() {
        assert_eq!(
            html_to_text("&lt;b&gt; &#65;&#x42; &quot;q&quot; &nbsp;x"),
            "<b> AB \"q\" x"
        );
        assert_eq!(
            html_to_text("AT&T &unknown; &#0; & done"),
            "AT&T &unknown; &#0; & done"
        );
        // Decoding is last, so a reference cannot reintroduce a tag that was then stripped.
        assert_eq!(
            html_to_text("&lt;script&gt;x&lt;/script&gt;"),
            "<script>x</script>"
        );
    }

    #[test]
    fn multibyte_text_survives_every_offset_calculation() {
        assert_eq!(
            html_to_text("<p>héllo — 世界</p><SCRIPT>ünï</SCRIPT><p>fin</p>"),
            "héllo — 世界\nfin"
        );
    }
}
