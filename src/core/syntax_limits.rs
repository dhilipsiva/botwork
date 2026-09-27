//! Local preflight guards before entering the recursive parser or AST lowering.

use super::{diagnostic::Diagnostic, grammar::BWErr};

pub const DEFAULT_SOURCE_BYTES: usize = 1024 * 1024;
pub const MAX_SYNTAX_NESTING: usize = 32;
pub const MAX_EXPRESSION_OPERATORS: usize = 64;
pub const MAX_SYNTAX_COMPLEXITY: usize = 66;

/// Hosts may tighten the parser guard. Raising its fixed ceilings requires a
/// reviewed parser/stack change; this configuration never changes process globals.
#[derive(Clone, Debug)]
pub struct SyntaxLimits {
    pub nesting: usize,
    pub operators: usize,
}

impl Default for SyntaxLimits {
    fn default() -> Self {
        Self {
            nesting: MAX_SYNTAX_NESTING,
            operators: MAX_EXPRESSION_OPERATORS,
        }
    }
}

pub(crate) struct Violation {
    pub offset: usize,
    pub error: BWErr,
}

pub(crate) fn check_size(source: &str, source_bytes: usize) -> Result<(), Violation> {
    if source.len() > source_bytes {
        let mut offset = source_bytes;
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        return Err(Violation::limit(offset, "source bytes", source_bytes));
    }
    Ok(())
}

impl Violation {
    fn limit(offset: usize, resource: &'static str, limit: usize) -> Self {
        Self {
            offset,
            error: BWErr::ResourceLimit {
                resource,
                limit: limit as u64,
            },
        }
    }
}

#[derive(Default)]
struct Frame {
    delimiter: u8,
    expression: bool,
    operators: usize,
    else_ifs: usize,
    after_block: bool,
}

/// Lexical bounds are intentionally conservative. Operator symbols and and/or
/// words count throughout each pipe expression, including nested call arguments.
/// The grammar remains responsible for checking malformed tokens/delimiters.
pub(crate) fn check(
    source: &str,
    source_bytes: usize,
    limits: &SyntaxLimits,
    expression_root: bool,
) -> Result<(), Violation> {
    if limits.nesting > MAX_SYNTAX_NESTING || limits.operators > MAX_EXPRESSION_OPERATORS {
        return Err(Violation { offset: 0, error: Diagnostic::formatted(BWErr::RunConfiguration, format_args!(
            "Syntax limits cannot exceed nesting {MAX_SYNTAX_NESTING} or operators {MAX_EXPRESSION_OPERATORS}"
        )).into_error() });
    }
    check_size(source, source_bytes)?;
    let bytes = source.as_bytes();
    let mut frames = vec![Frame {
        expression: expression_root,
        ..Frame::default()
    }];
    let (mut index, mut pending_else) = (0, None);
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"###") {
            index += 3;
            while index < bytes.len() && !bytes[index..].starts_with(b"###") {
                index += 1;
            }
            index = (index + 3).min(bytes.len());
            continue;
        }
        if byte == b'#' {
            while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                index += 1;
            }
            continue;
        }
        let in_expression = frames.last().expect("root frame").expression;
        // Quotes in sentence text are ordinary characters, not string delimiters.
        if byte == b'"' && in_expression {
            index += 1;
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index = (index + 2).min(bytes.len()),
                    b'"' => {
                        index += 1;
                        break;
                    }
                    _ => index += 1,
                }
            }
            pending_else = None;
            continue;
        }
        let start = index;
        let mut word = None;
        if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 128 {
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || bytes[index] == b'_'
                    || bytes[index] >= 128)
            {
                index += 1;
            }
            word = Some(&source[start..index]);
        } else {
            index += 1;
        }
        if !expression_root && !frames.iter().any(|frame| frame.delimiter == b'|') {
            let frame = frames.last_mut().expect("root frame");
            let is_else = word.is_some_and(|word| word.eq_ignore_ascii_case("else"));
            if frame.after_block && !is_else {
                frame.else_ifs = 0;
                frame.after_block = false;
            }
            if is_else {
                pending_else = Some(frames.len());
                frames.last_mut().unwrap().after_block = false;
                continue;
            }
            if word.is_some_and(|word| word.eq_ignore_ascii_case("if")) {
                let chained = pending_else == Some(frames.len());
                let frame = frames.last_mut().unwrap();
                if chained {
                    frame.else_ifs += 1;
                } else {
                    frame.else_ifs = 0;
                }
            }
        }
        pending_else = None;
        let operator = word.is_some_and(|word| matches!(word, "and" | "or"))
            || (word.is_none()
                && matches!(
                    byte,
                    b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'!' | b'=' | b'<' | b'>'
                ));
        if operator && in_expression {
            for (depth, frame) in frames.iter_mut().enumerate() {
                if frame.delimiter == b'|' || (depth == 0 && expression_root) {
                    frame.operators += 1;
                    if frame.operators > limits.operators {
                        return Err(Violation::limit(
                            start,
                            "expression operators",
                            limits.operators,
                        ));
                    }
                }
            }
        }
        if word.is_none() {
            match byte {
                b'|' if frames.last().is_some_and(|frame| frame.delimiter == b'|') => {
                    frames.pop();
                }
                b'|' | b'(' | b'[' | b'{' => frames.push(Frame {
                    delimiter: byte,
                    // @{ switches to sentence text until an argument pipe opens.
                    expression: byte == b'|'
                        || (in_expression
                            && !(byte == b'{' && start > 0 && bytes[start - 1] == b'@')),
                    ..Frame::default()
                }),
                b')' | b']' | b'}' => {
                    let opener = match byte {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    if frames.len() > 1 && frames.last().unwrap().delimiter == opener {
                        frames.pop();
                        if byte == b'}' {
                            frames.last_mut().unwrap().after_block = true;
                        }
                    }
                }
                _ => {}
            }
        }
        let depth = frames.len() - 1 + frames.iter().map(|frame| frame.else_ifs).sum::<usize>();
        if depth > limits.nesting {
            return Err(Violation::limit(start, "syntax nesting", limits.nesting));
        }
        let operators = frames
            .iter()
            .map(|frame| frame.operators)
            .max()
            .unwrap_or(0);
        if 2 * depth + operators > MAX_SYNTAX_COMPLEXITY {
            return Err(Violation::limit(
                start,
                "combined syntax complexity",
                MAX_SYNTAX_COMPLEXITY,
            ));
        }
    }
    Ok(())
}
