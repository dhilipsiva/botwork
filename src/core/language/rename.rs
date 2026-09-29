//! Renaming definitions and variables without changing what any call or read
//! reaches. A rename that could change resolution is refused with a reason.
use super::{Analysis, Language, Target};
use crate::core::{
    ast::{Program, StatementKind},
    format::SourceKind,
};
use std::{collections::BTreeMap, fs, path::Path};

/// Replace the bytes `start..end` with `text`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// What a rename at a position changes: the range it covers there, and the
/// current name to edit, a variable name or a statement header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameTarget {
    pub start: usize,
    pub end: usize,
    pub placeholder: String,
}

enum Symbol {
    Variable { frame: usize, name: String },
    Definition { file: String, header_start: usize },
}

/// Built-ins that look variables up by a name given at run time.
const BY_NAME: [&str; 2] = ["getvariable|param|", "variableexists|param|"];

/// Characters that separate the words of a sentence.
fn separator(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n' | '\\')
}

/// The byte range of the words in `start..end`, without surrounding spaces
/// and continuations.
fn words(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let region = &text[start..end];
    let first = region.find(|character| !separator(character))?;
    let last = region.rfind(|character| !separator(character))?;
    let width = region[last..].chars().next().map_or(0, char::len_utf8);
    Some((start + first, start + last + width))
}

/// The ranges before, between, and after the parameters of a call or header
/// spanning `span`, whose parameters' contents span `parameters`.
fn regions(
    text: &str,
    span: (usize, usize),
    parameters: &[(usize, usize)],
) -> Option<Vec<(usize, usize)>> {
    let mut regions = Vec::with_capacity(parameters.len() + 1);
    let mut start = span.0;
    for &(parameter_start, parameter_end) in parameters {
        let open = start + text.get(start..parameter_start)?.rfind('|')?;
        let close = parameter_end + text.get(parameter_end..span.1)?.find('|')?;
        regions.push((start, open));
        start = close + 1;
    }
    regions.push((start, span.1));
    Some(regions)
}

/// Edits that give a call or header the words `parts`, one per region. A
/// qualified call keeps its namespace.
fn word_edits(
    text: &str,
    span: (usize, usize),
    parameters: &[(usize, usize)],
    parts: &[String],
) -> Result<Vec<Edit>, String> {
    let regions = regions(text, span, parameters)
        .filter(|regions| regions.len() == parts.len())
        .ok_or_else(|| format!("Cannot edit `{}`", &text[span.0..span.1]))?;
    let last = regions.len() - 1;
    let mut edits = Vec::new();
    for (index, ((start, end), part)) in regions.into_iter().zip(parts).enumerate() {
        let old = words(text, start, end);
        let mut new = part.clone();
        if let (0, Some((first, end))) = (index, old) {
            if let Some(at) = text[first..end].rfind("::") {
                if new.is_empty() {
                    return Err("A qualified call needs words before its first parameter".into());
                }
                new = format!("{}{new}", &text[first..first + at + 2]);
            }
        }
        match old {
            Some((first, end)) if text[first..end] == new => {}
            Some((first, end)) if !new.is_empty() => edits.push(Edit {
                start: first,
                end,
                text: new,
            }),
            Some(_) => edits.push(Edit {
                start,
                end,
                text: if index == 0 || index == last {
                    String::new()
                } else {
                    " ".into()
                },
            }),
            None if new.is_empty() => {}
            None => edits.push(Edit {
                start,
                end,
                text: match (index == 0, index == last) {
                    (true, true) => new,
                    (true, false) => format!("{new} "),
                    (false, true) => format!(" {new}"),
                    (false, false) => format!(" {new} "),
                },
            }),
        }
    }
    Ok(edits)
}

/// A new header: its signature, parameter names, and words.
struct Header {
    signature: String,
    parameters: Vec<String>,
    parts: Vec<String>,
}

fn header(text: &str) -> Option<Header> {
    let program = Program::parse_detailed("rename", &format!("{text} {{\n}}\n")).ok()?;
    let [statement] = program.statements.as_slice() else {
        return None;
    };
    let StatementKind::Define(definition) = statement.kind() else {
        return None;
    };
    let source = definition.header.source().text();
    let span = (definition.header.start(), definition.header.end());
    // A header without parameters ends with the space before its block.
    if source[span.0..span.1].trim_end() != text {
        return None;
    }
    let parameters: Vec<_> = definition
        .parameters
        .iter()
        .map(|name| (name.span.start(), name.span.end()))
        .collect();
    let parts = regions(source, span, &parameters)?
        .into_iter()
        .map(|(start, end)| {
            words(source, start, end)
                .map_or_else(String::new, |(first, end)| source[first..end].to_owned())
        })
        .collect();
    Some(Header {
        signature: definition.signature.clone(),
        parameters: definition
            .parameters
            .iter()
            .map(|name| name.text.clone())
            .collect(),
        parts,
    })
}

impl Analysis {
    /// The frame and its ancestors.
    fn ancestry(&self, frame: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(Some(frame), |frame| self.index.parents[*frame])
    }

    fn symbol(&self, offset: usize) -> Result<(Symbol, RenameTarget), String> {
        if let Some(variable) = self.variable_at(offset) {
            let Some(frame) = variable.frame else {
                return Err(format!(
                    "`{}` is never assigned here: it is an input variable, named by whoever supplies it",
                    variable.name
                ));
            };
            return Ok((
                Symbol::Variable {
                    frame,
                    name: variable.name.clone(),
                },
                RenameTarget {
                    start: variable.span.0,
                    end: variable.span.1,
                    placeholder: variable.name.clone(),
                },
            ));
        }
        let definition = |index: usize, start: usize, end: usize| {
            let definition = &self.index.definitions[index];
            (
                Symbol::Definition {
                    file: definition.file.clone(),
                    header_start: definition.header_start,
                },
                RenameTarget {
                    start,
                    end,
                    placeholder: definition.header.clone(),
                },
            )
        };
        if let Some(index) = self.definition_at(offset) {
            let found = &self.index.definitions[index];
            return Ok(definition(index, found.header_start, found.header_end));
        }
        let call = self
            .call_at(offset)
            .ok_or("There is no statement or variable to rename here")?;
        match &call.target {
            Target::Definition(index) => Ok(definition(*index, call.span.0, call.span.1)),
            Target::Module(location, header) => Ok((
                Symbol::Definition {
                    file: location.file.clone(),
                    header_start: location.start,
                },
                RenameTarget {
                    start: call.span.0,
                    end: call.span.1,
                    placeholder: header.clone(),
                },
            )),
            Target::Builtin => Err("Built-in statements cannot be renamed".into()),
            Target::Unknown => Err("This call reaches no definition to rename".into()),
        }
    }

    /// The range a rename at `offset` covers and the name to start from, or
    /// why nothing there can be renamed.
    pub fn prepare_rename(&self, offset: usize) -> Result<RenameTarget, String> {
        self.symbol(offset).map(|(_, target)| target)
    }

    fn rename_variable(&self, frame: usize, old: &str, new: &str) -> Result<Vec<Edit>, String> {
        if new == old {
            return Err(format!("The variable is already named `{new}`"));
        }
        let valid = Program::parse_detailed("rename", &format!("|{new}| = |1|\n"))
            .ok()
            .is_some_and(|program| {
                matches!(program.statements.as_slice(), [statement]
                    if matches!(statement.kind(), StatementKind::Assign { name, .. } if name.text == new))
            });
        if !valid {
            return Err(format!("`{new}` is not a variable name"));
        }
        if self.index.variables.iter().any(|other| other.name == new) {
            return Err(format!(
                "`{new}` is already a variable in this file, so reads could reach the wrong one"
            ));
        }
        // Another scope that encloses this one, or that this one encloses, and
        // also assigns the name: which assignment a read reaches depends on
        // the order the run assigns them in.
        let shadowed = self.index.variables.iter().any(|other| {
            other.binding
                && other.name == old
                && other.frame.is_some_and(|scope| {
                    scope != frame
                        && (self.ancestry(scope).any(|parent| parent == frame)
                            || self.ancestry(frame).any(|parent| parent == scope))
                })
        });
        if shadowed {
            return Err(format!(
                "`{old}` is also assigned in an enclosing or nested scope, so which assignment a read reaches depends on the run"
            ));
        }
        let by_name = self.index.calls.iter().any(|call| {
            call.target == Target::Builtin
                && BY_NAME.contains(&call.signature.as_str())
                && call.literals.iter().any(|literal| {
                    literal
                        .as_deref()
                        .is_none_or(|name| name == old || name == new)
                })
        });
        if by_name {
            return Err(format!(
                "A variable is looked up by name here, and it may be `{old}` or `{new}`"
            ));
        }
        Ok(self
            .index
            .variables
            .iter()
            .filter(|variable| variable.name == old && variable.frame == Some(frame))
            .map(|variable| Edit {
                start: variable.span.0,
                end: variable.span.1,
                text: new.to_owned(),
            })
            .collect())
    }
}

impl Language {
    /// Rename the symbol at `offset` in the document `name` to `new_name`: a
    /// variable name, or a header with the same parameters. `documents` holds
    /// every file to update, by path; a module that a document imports is
    /// read from disk when it is not among them. Returns edits by file, or
    /// why the rename could change what a call or read reaches.
    pub fn rename(
        &self,
        documents: &BTreeMap<String, String>,
        name: &str,
        offset: usize,
        new_name: &str,
    ) -> Result<BTreeMap<String, Vec<Edit>>, String> {
        let text = documents.get(name).ok_or("The document is not open")?;
        let analysis = self.analyze(name, text, SourceKind::of_path(Path::new(name)));
        if !analysis.parsed {
            return Err("The file does not parse; fix its syntax errors first".into());
        }
        let new_name = new_name.trim();
        let edits = match analysis.symbol(offset)?.0 {
            Symbol::Variable { frame, name: old } => BTreeMap::from([(
                name.to_owned(),
                analysis.rename_variable(frame, &old, new_name)?,
            )]),
            Symbol::Definition { file, header_start } => {
                self.rename_definition(documents, &file, header_start, new_name)?
            }
        };
        let mut sorted = BTreeMap::new();
        for (file, mut edits) in edits {
            edits.sort();
            if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
                return Err("The edits would overlap".into());
            }
            if !edits.is_empty() {
                sorted.insert(file, edits);
            }
        }
        Ok(sorted)
    }

    fn rename_definition(
        &self,
        documents: &BTreeMap<String, String>,
        file: &str,
        header_start: usize,
        new: &str,
    ) -> Result<BTreeMap<String, Vec<Edit>>, String> {
        let mut texts = documents.clone();
        if !texts.contains_key(file) {
            let text = fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
            texts.insert(file.to_owned(), text);
        }
        let analyses: BTreeMap<&str, Analysis> = texts
            .iter()
            .map(|(name, text)| {
                let kind = SourceKind::of_path(Path::new(name));
                (name.as_str(), self.analyze(name, text, kind))
            })
            .collect();
        let target = &analyses[file];
        if !target.parsed {
            return Err(format!("`{file}` does not parse; fix it first"));
        }
        let (index, definition) = target
            .index
            .definitions
            .iter()
            .enumerate()
            .find(|(_, definition)| {
                definition.file == file && definition.header_start == header_start
            })
            .ok_or("The definition was not found")?;
        let header = header(new)
            .ok_or_else(|| format!("`{new}` is not a statement header, such as `Double |x|`"))?;
        if header.signature.contains("::") {
            return Err("A statement name cannot contain `::`".into());
        }
        let parameters: Vec<&str> = definition
            .parameters
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        if header.parameters != parameters {
            let expected: Vec<String> = parameters.iter().map(|name| format!("|{name}|")).collect();
            return Err(format!(
                "Keep the parameters {} in order; rename a parameter as a variable",
                expected.join(" ")
            ));
        }
        if new == definition.header {
            return Err(format!("The statement is already `{new}`"));
        }
        if definition.nested {
            return Err(
                "The definition sits inside a block, so only some runs register it and which definition a call reaches depends on the run"
                    .into(),
            );
        }
        let duplicated = target.index.definitions.iter().any(|other| {
            other.header_start != header_start
                && other.frame == definition.frame
                && other.signature == definition.signature
        });
        if duplicated {
            return Err("Another definition in the same scope has this signature".into());
        }
        // Only the root's definitions are exported to importers.
        let exported = target.index.parents[definition.frame].is_none();
        if header.signature != definition.signature {
            if self.builtins.contains_key(&header.signature) {
                return Err(format!("`{new}` is a built-in statement"));
            }
            if target
                .index
                .definitions
                .iter()
                .any(|other| other.signature == header.signature)
            {
                return Err(format!("`{new}` is already defined in this file"));
            }
            if target
                .index
                .calls
                .iter()
                .any(|call| call.signature == header.signature)
            {
                return Err(format!(
                    "`{new}` is already called in this file, and those calls would reach the renamed definition"
                ));
            }
            for (name, analysis) in &analyses {
                let collides = exported
                    && analysis.index.calls.iter().any(|call| {
                        call.module.as_ref().is_some_and(|(module, signature)| {
                            module.to_str() == Some(file) && *signature == header.signature
                        })
                    });
                if collides {
                    return Err(format!(
                        "`{name}` already calls `{new}` in this module, and those calls would reach the renamed definition"
                    ));
                }
            }
        }
        let module_name = Path::new(file)
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        for (name, analysis) in &analyses {
            if exported && !analysis.parsed && texts[*name].contains(&module_name) {
                return Err(format!(
                    "`{name}` does not parse and may import this module; fix it first"
                ));
            }
        }
        let mut edits: BTreeMap<String, Vec<Edit>> = BTreeMap::new();
        let text = &texts[file];
        let spans: Vec<_> = definition
            .parameters
            .iter()
            .map(|(_, span)| *span)
            .collect();
        edits.entry(file.to_owned()).or_default().extend(word_edits(
            text,
            (definition.header_start, definition.header_end),
            &spans,
            &header.parts,
        )?);
        for (name, analysis) in &analyses {
            for call in &analysis.index.calls {
                let reaches = match &call.target {
                    Target::Definition(other) => *name == file && *other == index,
                    Target::Module(location, _) => {
                        location.file == file && location.start == header_start
                    }
                    Target::Builtin | Target::Unknown => false,
                };
                if reaches {
                    edits
                        .entry((*name).to_owned())
                        .or_default()
                        .extend(word_edits(
                            &texts[*name],
                            call.span,
                            &call.arguments,
                            &header.parts,
                        )?);
                }
            }
        }
        // Other files resolve modules as they are on disk.
        if edits.keys().any(|name| name != file) {
            for name in edits.keys() {
                if fs::read_to_string(name).is_ok_and(|disk| disk != texts[name]) {
                    return Err(format!(
                        "Save `{name}` first: other files reach it as it is on disk"
                    ));
                }
            }
        }
        Ok(edits)
    }
}

/// Apply sorted, non-overlapping edits to `text`.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        result.push_str(&text[at..edit.start]);
        result.push_str(&edit.text);
        at = edit.end;
    }
    result.push_str(&text[at..]);
    result
}

#[cfg(test)]
mod tests;
