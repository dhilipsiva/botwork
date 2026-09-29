//! Signature help for the call being typed. It reads the text before the
//! cursor, so it works while the call is incomplete, and matches it against the
//! built-in statements, the file's definitions, and imported modules' exports.
use super::{read_module, Analysis, Language};
use crate::core::ast::{normalize_sentence, Program, StatementKind};
use std::path::Path;

/// Statements the call being typed can still become, and the parameter the
/// cursor is in or before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    /// The parameter being typed, or next; None after the last one.
    pub active_parameter: Option<usize>,
}

/// A statement's header, its documentation in Markdown, and each parameter's
/// byte range in the header, pipes included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    pub documentation: String,
    pub parameters: Vec<(usize, usize)>,
}

/// A header's normalized words, one entry per region before, between, and
/// after its parameters, and its parameters' ranges.
#[derive(Clone, Debug)]
pub(super) struct Shape {
    parts: Vec<String>,
    parameters: Vec<(usize, usize)>,
}

/// The shape of a header such as `Add |a| To |b|` or `m::Double |x|`.
pub(super) fn shape(header: &str) -> Option<Shape> {
    let program = Program::parse_detailed("signature", &format!("{header} {{\n}}\n")).ok()?;
    let [statement] = program.statements.as_slice() else {
        return None;
    };
    let StatementKind::Define(definition) = statement.kind() else {
        return None;
    };
    let mut parts = Vec::new();
    let mut parameters = Vec::new();
    let mut start = 0;
    for name in &definition.parameters {
        let open = header[..name.span.start()].rfind('|')?;
        let close = name.span.end() + header[name.span.end()..].find('|')?;
        parts.push(normalize_sentence(&header[start..open]));
        parameters.push((open, close + 1));
        start = close + 1;
    }
    parts.push(normalize_sentence(header.get(start..)?));
    Some(Shape { parts, parameters })
}

/// The call being typed before the cursor: the normalized words of each
/// region so far, and whether the cursor is inside a parameter.
#[derive(Debug, PartialEq, Eq)]
struct Typed {
    parts: Vec<String>,
    inside: bool,
}

fn typed(text: &str, offset: usize) -> Option<Typed> {
    let line = &text[text[..offset].rfind('\n').map_or(0, |index| index + 1)..offset];
    enum Open {
        Call {
            parts: Vec<String>,
            current: String,
            inside: bool,
        },
        Brace,
    }
    let call = || Open::Call {
        parts: Vec::new(),
        current: String::new(),
        inside: false,
    };
    let mut stack = vec![call()];
    let (mut string, mut escaped) = (false, false);
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            _ if escaped => escaped = false,
            '\\' if string => escaped = true,
            '"' => string = !string,
            _ if string => {}
            '#' => return None,
            '@' if characters.peek() == Some(&'{') => {
                characters.next();
                stack.push(call());
            }
            '{' => stack.push(Open::Brace),
            '}' => {
                stack.pop();
                if stack.is_empty() {
                    return None;
                }
            }
            '|' => {
                if let Some(Open::Call {
                    parts,
                    current,
                    inside,
                }) = stack.last_mut()
                {
                    if !*inside {
                        parts.push(normalize_sentence(current));
                        current.clear();
                    }
                    *inside = !*inside;
                }
            }
            // The innermost call collects its words outside parameters.
            _ => {
                if let Some(Open::Call {
                    current,
                    inside: false,
                    ..
                }) = stack.last_mut()
                {
                    current.push(character);
                }
            }
        }
    }
    let root = stack.len() == 1;
    let Some(Open::Call {
        mut parts,
        current,
        inside,
    }) = stack.pop()
    else {
        return None;
    };
    if string {
        return None;
    }
    if !inside {
        parts.push(normalize_sentence(&current));
    }
    // An assignment's value: `|x| = Call ...`.
    if root && parts.len() >= 2 && parts[0].is_empty() && parts[1].starts_with('=') {
        parts.remove(0);
        parts[0].remove(0);
    }
    // Control statements such as `If |...|` match no header: headers cannot
    // start with their keywords.
    let first = parts.first()?;
    (parts.len() > 1 || !first.is_empty() || inside).then_some(Typed { parts, inside })
}

impl Analysis {
    /// Signature help at `offset` in `text`.
    pub fn signature_help(
        &self,
        language: &Language,
        text: &str,
        offset: usize,
    ) -> Option<SignatureHelp> {
        let typed = typed(text, offset)?;
        let mut candidates: Vec<(String, String)> = language
            .builtins
            .values()
            .map(|builtin| {
                (
                    builtin.header().text().trim().to_owned(),
                    format!("```\n{}\n```", builtin.display_help()),
                )
            })
            .collect();
        candidates.extend(
            self.index
                .definitions
                .iter()
                .map(|definition| (definition.header.clone(), "Defined in this file".to_owned())),
        );
        for import in &self.index.imports {
            if let Some(module) = &import.module {
                exports(module, &import.name, 0, &mut candidates);
            }
        }
        // Regions typed in full must match; the region being typed may be a
        // prefix. Inside a parameter, every region so far is complete.
        let complete = typed.parts.len() - usize::from(!typed.inside);
        let mut matched: Vec<(bool, Signature)> = Vec::new();
        for (label, documentation) in candidates {
            let Some(shape) = shape(&label) else {
                continue;
            };
            let fits = shape.parameters.len() >= complete
                && typed.parts[..complete] == shape.parts[..complete]
                && (typed.inside || shape.parts[complete].starts_with(&typed.parts[complete]));
            if fits
                && !matched
                    .iter()
                    .any(|(_, signature)| signature.label == label)
            {
                let exact = typed.inside || shape.parts[complete] == typed.parts[complete];
                matched.push((
                    exact,
                    Signature {
                        label,
                        documentation,
                        parameters: shape.parameters,
                    },
                ));
            }
        }
        matched.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.label.cmp(&right.1.label))
        });
        matched.truncate(20);
        let active = if typed.inside { complete - 1 } else { complete };
        let signatures: Vec<Signature> = matched
            .into_iter()
            .map(|(_, signature)| signature)
            .collect();
        let first = signatures.first()?;
        Some(SignatureHelp {
            active_parameter: (active < first.parameters.len()).then_some(active),
            signatures,
        })
    }
}

/// A module's root definitions as `alias::Header`, and those of the modules it
/// imports, qualified again.
fn exports(module: &Path, alias: &str, depth: usize, found: &mut Vec<(String, String)>) {
    let Some((name, program)) = read_module(module).filter(|_| depth < 4) else {
        return;
    };
    for statement in &program.statements {
        match statement.kind() {
            StatementKind::Define(definition) => found.push((
                format!("{alias}::{}", definition.header.text().trim()),
                format!("Defined in `{name}`"),
            )),
            StatementKind::Import {
                path, namespace, ..
            } => {
                if let Some(nested) = super::module_path(&name, Path::new("/"), path)
                    .and_then(|requested| std::fs::canonicalize(requested).ok())
                {
                    exports(
                        &nested,
                        &format!("{alias}::{}", namespace.text.trim()),
                        depth + 1,
                        found,
                    );
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
