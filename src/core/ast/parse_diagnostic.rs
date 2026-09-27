//! Stream Pest-compatible syntax details without its intermediate formatting buffers.
//! Parser-owned error lines already exist; only this interpreter message is constructed here.

use super::{Rule, Span};
use pest::error::{Error, ErrorVariant, InputLocation, LineColLocation};
use std::fmt::{self, Write};

#[cfg(test)]
mod tests;

pub(crate) struct ParseDisplay<'a> {
    pub error: &'a Error<Rule>,
    pub span: &'a Span,
}

struct Reason<'a>(&'a ErrorVariant<Rule>);

impl fmt::Display for Reason<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ErrorVariant::CustomError { message } => out.write_str(message),
            ErrorVariant::ParsingError {
                positives,
                negatives,
            } => {
                if !negatives.is_empty() {
                    out.write_str("unexpected ")?;
                    rules(out, negatives)?;
                    if !positives.is_empty() {
                        out.write_str("; ")?;
                    }
                }
                if !positives.is_empty() {
                    out.write_str("expected ")?;
                    rules(out, positives)?;
                }
                if positives.is_empty() && negatives.is_empty() {
                    out.write_str("unknown parsing error")?;
                }
                Ok(())
            }
        }
    }
}

fn rules(out: &mut fmt::Formatter<'_>, rules: &[Rule]) -> fmt::Result {
    for (index, rule) in rules.iter().enumerate() {
        if index != 0 {
            out.write_str(if index + 1 == rules.len() {
                if rules.len() == 2 {
                    " or "
                } else {
                    ", or "
                }
            } else {
                ", "
            })?;
        }
        // Generated Rule debug names are fixed scalar strings, not host formatters.
        write!(out, "{rule:?}")?;
    }
    Ok(())
}

impl fmt::Display for ParseDisplay<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        if out.alternate() {
            // Rejection must not visit source lines, coordinate scans, or underlines.
            return write!(
                out,
                "at byte {} [source excerpt omitted]: {}",
                self.span.start(),
                Reason(&self.error.variant)
            );
        }
        let (line, column, end) = match self.error.line_col {
            LineColLocation::Pos((line, column)) => (line, column, None),
            LineColLocation::Span((line, column), end) => (line, column, Some(end)),
        };
        let last_line = end.map_or(line, |end| line.max(end.0));
        let width = last_line.checked_ilog10().unwrap_or(0) as usize + 1;
        write!(out, "{:width$}--> ", "")?;
        if let Some(path) = self.error.path() {
            write!(out, "{path}:")?;
        }
        write!(out, "{line}:{column}\n{:width$} |\n", "")?;

        // Generated parser failures use positions. Preserve span formatting too;
        // recover the borrowed continued line from the original source, because
        // Pest exposes only the first owned error line through its public API.
        let selected = match self.error.location {
            InputLocation::Pos(_) => None,
            InputLocation::Span((start, end)) => {
                pest::Span::new(self.span.source().text(), start, end)
            }
        };
        let continued = selected
            .as_ref()
            .and_then(|span| span.lines().skip(1).last());
        if let (Some(continued), Some(end)) = (continued, end) {
            writeln!(out, "{line:width$} | {}", self.error.line())?;
            if end.0.saturating_sub(line) > 1 {
                writeln!(out, "{:width$} | ...", "")?;
            }
            write!(out, "{:width$} | ", end.0)?;
            let source = self.span.text();
            let visualize = matches!(source.chars().next(), Some('\r' | '\n'))
                || matches!(source.chars().next_back(), Some('\r' | '\n'));
            // Preserve Pest's continued-line whitespace convention, including
            // raw line endings when the selected span already visualizes them.
            if visualize {
                out.write_str(continued)?;
            } else {
                for character in continued.chars() {
                    match character {
                        '\r' => out.write_str("␍")?,
                        '\n' => out.write_str("␊")?,
                        other => out.write_char(other)?,
                    }
                }
            }
            out.write_str("\n")?;
        } else {
            writeln!(out, "{line} | {}", self.error.line())?;
        }
        write!(out, "{:width$} | ", "")?;
        let mut start = column;
        let end_column = end.map(|(_, mut end)| {
            if start > end {
                std::mem::swap(&mut start, &mut end);
                start = start.saturating_sub(1);
                end = end.saturating_add(1);
            }
            end
        });
        for character in self.error.line().chars().take(start.saturating_sub(1)) {
            out.write_str(if character == '\t' { "\t" } else { " " })?;
        }
        if let Some(end) = end_column {
            out.write_str("^")?;
            if end.saturating_sub(start) > 1 {
                for _ in 2..end - start {
                    out.write_str("-")?;
                }
                out.write_str("^")?;
            }
        } else {
            out.write_str("^---")?;
        }
        write!(
            out,
            "\n{:width$} |\n{:width$} = {}",
            "",
            "",
            Reason(&self.error.variant)
        )
    }
}
