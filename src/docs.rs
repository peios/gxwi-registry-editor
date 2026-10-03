//! What the registry manual says of a key and its values: regman's records,
//! read with libregman, and their Markdown as HTML for the pane.
//!
//! A record says what a value is for, the type it should have, its default,
//! the values it may take and when a change applies; a key's record says
//! what the key is for. The manual is documentation shipped with packages,
//! not the registry: a value can be documented and not set, set and not
//! documented, or set to a type the manual doesn't expect.

use libgxwi::escape;
use libregman::fold::fold;
use libregman::fragment::{Kind, Record};
use libregman::query;
use peios::registry::ValueType;

/// What the manual says of one key.
#[derive(Debug, Clone, Default)]
pub struct Docs {
    pub key: Option<Record>,
    /// Its documented values, each by its name.
    pub values: Vec<(String, Record)>,
}

impl Docs {
    /// What the manual says of the value `name`, if anything.
    pub fn value(&self, name: &str) -> Option<&Record> {
        let name = fold(name);
        self.values.iter().find(|(documented, _)| fold(documented) == name).map(|(_, record)| record)
    }
}

/// What the manual says of the key at `path` and the values it documents
/// for it. A manual that can't be read says nothing.
pub fn of(path: &str) -> Docs {
    let hits = query::resolve_key_default(&fold(path)).unwrap_or_default();
    let mut docs = Docs::default();
    for hit in hits {
        match hit.record.kind() {
            Kind::Key => {
                docs.key.get_or_insert(hit.record);
            }
            Kind::Value => {
                let name = hit.record.value_name(path).to_string();
                if !docs.values.iter().any(|(documented, _)| fold(documented) == fold(&name)) {
                    docs.values.push((name, hit.record));
                }
            }
        }
    }
    docs.values.sort_by_cached_key(|(name, _)| name.to_lowercase());
    docs
}

/// The type a record documents, if it names one of the registry's.
pub fn ty(record: &Record) -> Option<ValueType> {
    let named = record.type_.as_deref()?.trim();
    (0..=0x20).map(ValueType).chain([ValueType::TOMBSTONE]).find(|ty| ty.name() == Some(named))
}

/// When a change to it applies, in words.
pub fn applies(record: &Record) -> Option<&'static str> {
    Some(match record.applies.as_deref()?.trim() {
        "live" => "At once",
        "restart" => "When the program that reads it next starts",
        "reboot" => "When the machine next starts",
        _ => return None,
    })
}

/// A record's first paragraph, as plain text on one line, its markup left
/// out: what a value is for, in a list of them.
pub fn first_paragraph(markdown: &str) -> String {
    let paragraph = markdown.trim().split("\n\n").next().unwrap_or_default();
    paragraph.split_whitespace().collect::<Vec<_>>().join(" ").replace(['`', '*'], "")
}

/// A record's Markdown as HTML: paragraphs, headings, bullet lists, code
/// blocks, and inline bold, emphasis and code. Everything is escaped first;
/// nothing in a fragment becomes markup of its own.
pub fn html(markdown: &str) -> String {
    let mut out = String::new();
    let mut lines = markdown.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("```") {
            let mut code = Vec::new();
            for line in lines.by_ref() {
                if line.trim().starts_with("```") {
                    break;
                }
                code.push(line);
            }
            out += &format!("<pre>{}</pre>", escape(&code.join("\n")));
        } else if let Some(heading) = trimmed.strip_prefix('#') {
            out += &format!("<h4>{}</h4>", inline(heading.trim_start_matches('#').trim()));
        } else if bullet(trimmed).is_some() {
            let mut items: Vec<String> = Vec::new();
            let mut line = Some(trimmed.to_string());
            while let Some(current) = line.take() {
                match bullet(&current) {
                    Some(item) => items.push(item.to_string()),
                    None => {
                        if let Some(last) = items.last_mut() {
                            last.push(' ');
                            last.push_str(current.trim());
                        }
                    }
                }
                if let Some(next) = lines.peek().map(|next| next.trim()).filter(|next| !next.is_empty()) {
                    line = Some(next.to_string());
                    lines.next();
                }
            }
            out += &format!("<ul>{}</ul>", items.iter().map(|item| format!("<li>{}</li>", inline(item))).collect::<String>());
        } else {
            let mut paragraph = vec![trimmed.to_string()];
            while let Some(next) = lines.peek().map(|next| next.trim()) {
                if next.is_empty() || next.starts_with("```") || next.starts_with('#') || bullet(next).is_some() {
                    break;
                }
                paragraph.push(next.to_string());
                lines.next();
            }
            out += &format!("<p>{}</p>", inline(&paragraph.join(" ")));
        }
    }
    out
}

fn bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
}

/// Inline markup in one run of text: `code` first, since nothing inside it
/// is markup, then **bold** and *emphasis*. A delimiter with no closer stays
/// as written.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(close) = rest[open + 1..].find('`') else { break };
        out += &emphasis(&rest[..open]);
        out += &format!("<code>{}</code>", escape(&rest[open + 1..open + 1 + close]));
        rest = &rest[open + 2 + close..];
    }
    out + &emphasis(rest)
}

fn emphasis(text: &str) -> String {
    let wrapped = |text: &str, mark: &str, tag: &str, inner: &dyn Fn(&str) -> String| -> Option<String> {
        let open = text.find(mark)?;
        let close = text[open + mark.len()..].find(mark)? + open + mark.len();
        (close > open + mark.len()).then(|| {
            format!("{}<{tag}>{}</{tag}>{}", inner(&text[..open]), inner(&text[open + mark.len()..close]), emphasis(&text[close + mark.len()..]))
        })
    };
    wrapped(text, "**", "strong", &emphasis)
        .or_else(|| wrapped(text, "*", "em", &|text: &str| escape(text)))
        .unwrap_or_else(|| escape(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_comes_out_as_html_with_everything_escaped() {
        assert_eq!(html("One line\nand more.\n\nTwo <b>."), "<p>One line and more.</p><p>Two &lt;b&gt;.</p>");
        assert_eq!(html("Uses `a<b>` and **bold** and *some*."), "<p>Uses <code>a&lt;b&gt;</code> and <strong>bold</strong> and <em>some</em>.</p>");
        assert_eq!(html("- one\n- two\n  more\n\nAfter."), "<ul><li>one</li><li>two more</li></ul><p>After.</p>");
        assert_eq!(html("## Heading\nText"), "<h4>Heading</h4><p>Text</p>");
        assert_eq!(html("```\nreg set x\n```"), "<pre>reg set x</pre>");
        // A delimiter with no closer stays as written.
        assert_eq!(html("2 * 3 and a ` alone"), "<p>2 * 3 and a ` alone</p>");
        assert_eq!(html("`**not bold**`"), "<p><code>**not bold**</code></p>");
    }

    #[test]
    fn a_record_s_type_and_when_it_applies_are_read() {
        let (records, _) = libregman::fragment::parse(
            "--- machine\\x v\ncanonical: Machine\\x V\ntype: REG_DWORD\napplies: restart\n\nIt is V.\n",
        );
        assert_eq!(ty(&records[0]), Some(ValueType::DWORD));
        assert_eq!(applies(&records[0]), Some("When the program that reads it next starts"));
        let docs = Docs { key: None, values: vec![("V".into(), records[0].clone())] };
        assert!(docs.value("v").is_some());
    }
}
