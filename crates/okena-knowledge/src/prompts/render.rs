//! Filling `{placeholder}` templates.
//!
//! The same syntax `harness.agent_args` already uses, so somebody who has
//! written one of those can write a template without learning a second thing.
//! Deliberately not a template *language*: no conditionals, no loops, no
//! expressions. Three forms, and each is about *where text comes from*, never
//! about computing it:
//!
//! - `{name}` — a value okena fills.
//! - `{>partial}` — the text of a shared partial, so an instruction every
//!   agent needs lives in one file instead of being copied into seven.
//! - `{name|partial}` — the value, or the partial when the value is empty, so
//!   "there is no description" is worded in knowledge rather than in code.
//!
//! A brief that genuinely needs a branch computes the branch in Rust and picks
//! a partial by name — see [`super::fragment`] — because the moment a template
//! can compute, reading one stops telling you what the agent will be told.

use std::collections::BTreeMap;

/// The values a template is filled from.
pub type Vars<'a> = BTreeMap<&'a str, String>;

/// How deep partials may include partials. Enough for a partial to share a
/// sentence with another; low enough that a cycle ends quickly.
const MAX_INCLUDE_DEPTH: usize = 4;

/// A rendered template, plus anything the caller should know about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Placeholders and partials the template asked for that do not exist.
    /// A missing partial is listed as `>name`.
    ///
    /// Left verbatim in `text` rather than emptied: a brief that silently
    /// loses the task it is about is worse than one that visibly says
    /// `{key}`, and this list is what lets the UI say so.
    pub unknown: Vec<String>,
}

impl Rendered {
    pub fn is_complete(&self) -> bool {
        self.unknown.is_empty()
    }
}

/// Fill `template` from `vars`, with no partials available.
pub fn render(template: &str, vars: &Vars<'_>) -> Rendered {
    render_with(template, vars, &|_| None)
}

/// Fill `template` from `vars`, reading partials through `partial`.
///
/// `{{` and `}}` are literal braces, so a template can talk about JSON or
/// about placeholders themselves without them being filled in.
pub fn render_with(
    template: &str,
    vars: &Vars<'_>,
    partial: &dyn Fn(&str) -> Option<String>,
) -> Rendered {
    let mut out = Rendered {
        text: String::with_capacity(template.len()),
        unknown: Vec::new(),
    };
    render_into(template, vars, partial, 0, &mut out);
    out
}

fn render_into(
    template: &str,
    vars: &Vars<'_>,
    partial: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut Rendered,
) {
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.text.push('}');
            }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if !closed {
                    out.text.push('{');
                    out.text.push_str(&inner);
                    continue;
                }
                match parse(&inner) {
                    Some(Token::Include(name)) => include(name, vars, partial, depth, out),
                    Some(Token::Value { name, fallback }) => match (vars.get(name), fallback) {
                        (Some(value), Some(fallback)) if value.trim().is_empty() => {
                            include(fallback, vars, partial, depth, out)
                        }
                        (Some(value), _) => out.text.push_str(value),
                        (None, _) => {
                            note(out, name);
                            out.text.push('{');
                            out.text.push_str(&inner);
                            out.text.push('}');
                        }
                    },
                    // Not a placeholder — `{"a": 1}`. Put back exactly what
                    // was there.
                    None => {
                        out.text.push('{');
                        out.text.push_str(&inner);
                        out.text.push('}');
                    }
                }
            }
            _ => out.text.push(c),
        }
    }
}

fn include(
    name: &str,
    vars: &Vars<'_>,
    partial: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut Rendered,
) {
    match (depth < MAX_INCLUDE_DEPTH).then(|| partial(name)).flatten() {
        Some(body) => render_into(&body, vars, partial, depth + 1, out),
        None => {
            note(out, &format!(">{name}"));
            out.text.push_str(&format!("{{>{name}}}"));
        }
    }
}

fn note(out: &mut Rendered, name: &str) {
    if !out.unknown.iter().any(|u| u == name) {
        out.unknown.push(name.to_string());
    }
}

enum Token<'a> {
    Include(&'a str),
    Value {
        name: &'a str,
        fallback: Option<&'a str>,
    },
}

/// Read what is between the braces, or `None` for incidental braces.
fn parse(inner: &str) -> Option<Token<'_>> {
    if let Some(name) = inner.strip_prefix('>') {
        return is_partial_name(name).then_some(Token::Include(name));
    }
    match inner.split_once('|') {
        Some((name, fallback)) => {
            (is_placeholder(name) && is_partial_name(fallback)).then_some(Token::Value {
                name,
                fallback: Some(fallback),
            })
        }
        None => is_placeholder(inner).then_some(Token::Value {
            name: inner,
            fallback: None,
        }),
    }
}

/// One identifier: a letter or underscore, then letters, digits and
/// underscores. The same rule `docs/reference/knowledge.md` states for listing
/// a template's placeholders, so what the Knowledge view shows and what the
/// renderer fills agree.
fn is_placeholder(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A partial's name: its file name under `templates/partials/`, kebab-case.
pub fn is_partial_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The placeholder names `template` uses, in first-seen order. Partials are
/// not followed: this is what the template itself asks for.
pub fn placeholders(template: &str) -> Vec<String> {
    render(template, &Vars::new())
        .unknown
        .into_iter()
        .filter(|u| !u.starts_with('>'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Vars, placeholders, render, render_with};

    fn vars(pairs: &[(&'static str, &str)]) -> Vars<'static> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    fn partials(name: &str) -> Option<String> {
        match name {
            "reporting" => Some("Report when you stop.".into()),
            "no-description" => Some("It has no description.".into()),
            "nested" => Some("Outer, then: {>reporting}".into()),
            "cycle" => Some("again {>cycle}".into()),
            "uses-var" => Some("for {key}".into()),
            _ => None,
        }
    }

    #[test]
    fn fills_every_known_placeholder() {
        let out = render(
            "Work on {key}: {title}",
            &vars(&[("key", "QBL-1"), ("title", "Ship it")]),
        );
        assert_eq!(out.text, "Work on QBL-1: Ship it");
        assert!(out.is_complete());
    }

    #[test]
    fn one_placeholder_can_be_used_twice() {
        let out = render("{key} … {key}", &vars(&[("key", "A-1")]));
        assert_eq!(out.text, "A-1 … A-1");
    }

    #[test]
    fn an_unknown_placeholder_survives_verbatim_and_is_reported() {
        // A brief that silently lost the task it is about would be worse than
        // one visibly saying `{nope}`.
        let out = render("a {nope} b", &vars(&[("key", "x")]));
        assert_eq!(out.text, "a {nope} b");
        assert_eq!(out.unknown, ["nope"]);
        assert!(!out.is_complete());
    }

    #[test]
    fn an_unknown_placeholder_is_reported_once_however_often_it_appears() {
        let out = render("{a} {a} {b}", &Vars::new());
        assert_eq!(out.unknown, ["a", "b"]);
    }

    #[test]
    fn doubled_braces_are_literal() {
        // So a template can show the placeholder syntax, or talk about JSON.
        let out = render("{{key}} stays, {key} fills", &vars(&[("key", "v")]));
        assert_eq!(out.text, "{key} stays, v fills");
        assert!(out.is_complete());
    }

    #[test]
    fn json_in_a_body_is_not_a_placeholder() {
        let out = render(r#"send {"a": 1} to it"#, &Vars::new());
        assert_eq!(out.text, r#"send {"a": 1} to it"#);
        assert!(out.unknown.is_empty(), "{:?}", out.unknown);
    }

    #[test]
    fn a_name_starting_with_a_digit_is_not_a_placeholder() {
        let out = render("{9lives}", &Vars::new());
        assert_eq!(out.text, "{9lives}");
        assert!(out.unknown.is_empty());
    }

    #[test]
    fn an_unclosed_brace_is_left_alone() {
        let out = render("trailing {oops", &Vars::new());
        assert_eq!(out.text, "trailing {oops");
        assert!(out.unknown.is_empty());
    }

    #[test]
    fn a_value_containing_braces_is_not_re_rendered() {
        // Otherwise a task titled "{key}" would rewrite itself.
        let out = render("{title}", &vars(&[("title", "{key} handling")]));
        assert_eq!(out.text, "{key} handling");
        assert!(out.is_complete());
    }

    #[test]
    fn placeholders_lists_what_a_template_asks_for() {
        assert_eq!(
            placeholders("{idea} into {change_dir}, not {{escaped}}, {>reporting}"),
            ["idea", "change_dir"]
        );
    }

    #[test]
    fn a_partial_is_included_in_place() {
        let out = render_with("Do it.\n\n{>reporting}", &Vars::new(), &partials);
        assert_eq!(out.text, "Do it.\n\nReport when you stop.");
        assert!(out.is_complete());
    }

    #[test]
    fn a_partial_sees_the_same_values_as_the_template() {
        let out = render_with("{>uses-var}", &vars(&[("key", "QBL-9")]), &partials);
        assert_eq!(out.text, "for QBL-9");
    }

    #[test]
    fn a_missing_partial_is_left_visible_and_reported() {
        let out = render_with("x {>nowhere} y", &Vars::new(), &partials);
        assert_eq!(out.text, "x {>nowhere} y");
        assert_eq!(out.unknown, [">nowhere"]);
    }

    #[test]
    fn partials_can_include_partials() {
        let out = render_with("{>nested}", &Vars::new(), &partials);
        assert_eq!(out.text, "Outer, then: Report when you stop.");
    }

    #[test]
    fn a_partial_that_includes_itself_stops() {
        // A cycle must end, and must say so rather than hang or vanish.
        let out = render_with("{>cycle}", &Vars::new(), &partials);
        assert!(out.text.ends_with("{>cycle}"), "{}", out.text);
        assert_eq!(out.unknown, [">cycle"]);
    }

    #[test]
    fn a_fallback_is_used_only_for_an_empty_value() {
        let empty = render_with(
            "{description|no-description}",
            &vars(&[("description", "  ")]),
            &partials,
        );
        assert_eq!(empty.text, "It has no description.");
        let present = render_with(
            "{description|no-description}",
            &vars(&[("description", "Real words")]),
            &partials,
        );
        assert_eq!(present.text, "Real words");
    }

    #[test]
    fn a_fallback_does_not_hide_a_variable_the_flow_never_fills() {
        // An empty value means "nothing to say"; a missing one is a typo.
        let out = render_with("{descripton|no-description}", &Vars::new(), &partials);
        assert_eq!(out.unknown, ["descripton"]);
    }
}
