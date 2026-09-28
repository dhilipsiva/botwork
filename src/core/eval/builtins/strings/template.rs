use super::*;
use crate::core::grammar::DisplayValue;
use build::RenderError;
use std::fmt;

pub(super) fn render<'a>(
    template: &'a str,
    values: &Literal,
    output: &mut dyn fmt::Write,
    sorted: bool,
) -> Result<(), RenderError<'a>> {
    let mut rest = template;
    while let Some(at) = rest.find(['{', '}']) {
        output.write_str(&rest[..at])?;
        rest = &rest[at..];
        if rest.starts_with("{{") || rest.starts_with("}}") {
            output.write_str(&rest[..1])?;
            rest = &rest[2..];
        } else if rest.starts_with('}') {
            return Err(RenderError::Invalid(
                "Unescaped closing brace in format template",
            ));
        } else {
            let end = rest
                .find('}')
                .ok_or(RenderError::Invalid("Unclosed field in format template"))?;
            let name = &rest[1..end];
            if name.contains('{') {
                return Err(RenderError::Invalid("Nested braces in format field"));
            }
            let selected = match values {
                Literal::Array(values) => {
                    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(RenderError::Field(name));
                    }
                    name.parse::<usize>()
                        .ok()
                        .and_then(|index| values.get(index))
                }
                Literal::Map(values) => values.get(name),
                _ => unreachable!("validated format values"),
            }
            .ok_or(RenderError::Field(name))?;
            write!(output, "{}", DisplayValue(selected, sorted))?;
            rest = &rest[end + 1..];
        }
    }
    output.write_str(rest)?;
    Ok(())
}
