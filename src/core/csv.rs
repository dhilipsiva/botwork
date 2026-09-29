//! Header-row CSV fields: RFC 4180 quoting with doubled quotes, and records
//! ending in LF or CRLF. Field text is borrowed unless it contains escapes.
use std::borrow::Cow;

/// A field with its unescaped text and whether it ends its record.
pub(crate) struct Field<'a> {
    pub text: Cow<'a, str>,
    pub last: bool,
}

pub(crate) struct Fields<'a> {
    text: &'a str,
    index: usize,
    pub line: usize,
    line_start: usize,
    finished: bool,
}

/// A malformed input position: one-based line and Unicode scalar column.
pub(crate) struct Malformed {
    pub line: usize,
    pub column: usize,
    /// Byte offset of the problem within the input.
    pub offset: usize,
    pub reason: &'static str,
}

impl<'a> Fields<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            text,
            index: 0,
            line: 1,
            line_start: 0,
            finished: text.is_empty(),
        }
    }

    fn fail(&self, at: usize, reason: &'static str) -> Malformed {
        let (line, line_start) = self.text[..at]
            .bytes()
            .enumerate()
            .skip(self.line_start)
            .filter(|(_, byte)| *byte == b'\n')
            .fold((self.line, self.line_start), |(line, _), (index, _)| {
                (line + 1, index + 1)
            });
        Malformed {
            line,
            column: self.text[line_start..at].chars().count() + 1,
            offset: at,
            reason,
        }
    }

    /// Consume a field delimiter or record terminator at the current index.
    fn delimiter(&mut self) -> Result<bool, Malformed> {
        let bytes = self.text.as_bytes();
        match bytes.get(self.index) {
            None => {
                self.finished = true;
                Ok(true)
            }
            Some(b',') => {
                self.index += 1;
                Ok(false)
            }
            Some(b'\n') => {
                self.index += 1;
                self.newline();
                Ok(true)
            }
            Some(b'\r') if bytes.get(self.index + 1) == Some(&b'\n') => {
                self.index += 2;
                self.newline();
                Ok(true)
            }
            Some(b'\r') => Err(self.fail(
                self.index,
                "a carriage return must be followed by a line feed",
            )),
            Some(_) => Err(self.fail(self.index, "a quoted field must end at a comma or line end")),
        }
    }

    fn newline(&mut self) {
        self.line += 1;
        self.line_start = self.index;
        self.finished = self.index == self.text.len();
    }

    /// The byte offset of the next unread field or record.
    pub(crate) fn offset(&self) -> usize {
        self.index
    }

    #[allow(clippy::should_implement_trait)]
    pub(crate) fn next(&mut self) -> Option<Result<Field<'a>, Malformed>> {
        if self.finished {
            return None;
        }
        let bytes = self.text.as_bytes();
        let start = self.index;
        let text = if bytes.get(start) == Some(&b'"') {
            let mut cursor = start + 1;
            let mut escaped = false;
            loop {
                match self.text[cursor..].find('"') {
                    None => return Some(Err(self.fail(start, "a quoted field is not terminated"))),
                    Some(offset) if bytes.get(cursor + offset + 1) == Some(&b'"') => {
                        cursor += offset + 2;
                        escaped = true;
                    }
                    Some(offset) => {
                        cursor += offset;
                        break;
                    }
                }
            }
            let inner = &self.text[start + 1..cursor];
            // Quoted line breaks advance positions for later diagnostics.
            for (offset, byte) in inner.bytes().enumerate() {
                if byte == b'\n' {
                    self.line += 1;
                    self.line_start = start + 1 + offset + 1;
                }
            }
            self.index = cursor + 1;
            if escaped {
                Cow::Owned(inner.replace("\"\"", "\""))
            } else {
                Cow::Borrowed(inner)
            }
        } else {
            let end = bytes[start..]
                .iter()
                .position(|byte| matches!(byte, b',' | b'\n' | b'\r' | b'"'))
                .map_or(bytes.len(), |offset| start + offset);
            if bytes.get(end) == Some(&b'"') {
                return Some(Err(self.fail(end, "a quote must start a quoted field")));
            }
            self.index = end;
            Cow::Borrowed(&self.text[start..end])
        };
        Some(self.delimiter().map(|last| Field { text, last }))
    }
}
