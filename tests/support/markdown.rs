//! Small, strict fenced-block reader for this repository's documentation tests.

#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub language: String,
    pub id: Option<String>,
    pub line: usize,
    pub source: String,
}

fn fence(line: &str) -> Option<(char, usize, &str)> {
    let content = line.trim_start_matches(' ');
    if line.len() - content.len() > 3 {
        return None;
    }
    let marker = content.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let length = content
        .chars()
        .take_while(|character| *character == marker)
        .count();
    (length >= 3).then(|| (marker, length, content[length..].trim()))
}

pub fn blocks(document: &str) -> Result<Vec<Block>, String> {
    let mut result = vec![];
    let mut pending_id = None;
    let mut current: Option<(char, usize, usize, Block)> = None;
    for (index, raw_line) in document.split_inclusive('\n').enumerate() {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if let Some((marker, length, indent, block)) = &mut current {
            if fence(line).is_some_and(|(end_marker, end_length, info)| {
                end_marker == *marker && end_length >= *length && info.is_empty()
            }) {
                result.push(current.take().unwrap().3);
            } else {
                let spaces = raw_line
                    .chars()
                    .take_while(|character| *character == ' ')
                    .count();
                block.source.push_str(&raw_line[spaces.min(*indent)..]);
            }
            continue;
        }
        if let Some(id) = line.trim().strip_prefix("<!-- botwork-test:") {
            let id = id
                .strip_suffix("-->")
                .ok_or_else(|| format!("line {}: unclosed example marker", index + 1))?
                .trim();
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(format!("line {}: invalid example id", index + 1));
            }
            if pending_id.replace(id.to_owned()).is_some() {
                return Err(format!("line {}: unused example marker", index + 1));
            }
        } else if let Some((marker, length, info)) = fence(line) {
            let language = info.split([',', ' ', '\t']).next().unwrap_or_default();
            if language == "rust" && info != "rust" {
                return Err(format!(
                    "line {}: Rust examples must run without fence options",
                    index + 1
                ));
            }
            if language == "botwork" && (info != "botwork" || pending_id.is_none()) {
                return Err(format!(
                    "line {}: botwork block requires a test marker and no fence options",
                    index + 1
                ));
            }
            if language != "botwork" && pending_id.is_some() {
                return Err(format!(
                    "line {}: example marker must precede botwork code",
                    index + 1
                ));
            }
            current = Some((
                marker,
                length,
                line.len() - line.trim_start_matches(' ').len(),
                Block {
                    language: language.into(),
                    id: pending_id.take(),
                    line: index + 2,
                    source: String::new(),
                },
            ));
        } else if !line.trim().is_empty() && pending_id.is_some() {
            return Err(format!(
                "line {}: example marker must immediately precede its fence",
                index + 1
            ));
        }
    }
    if current.is_some() || pending_id.is_some() {
        return Err("unclosed code fence or unused example marker".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_contents_and_crlf_are_preserved_without_interpreting_inner_markers() {
        let source = "<!-- botwork-test: sample -->\r\n```botwork\r\n# 🙂\r\n<!-- botwork-test: literal -->\r\n```\r\n";
        let parsed = blocks(source).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id.as_deref(), Some("sample"));
        assert_eq!(parsed[0].line, 3);
        assert_eq!(
            parsed[0].source,
            "# 🙂\r\n<!-- botwork-test: literal -->\r\n"
        );
    }

    #[test]
    fn tilde_longer_closing_fences_and_commonmark_indentation_are_supported() {
        let parsed =
            blocks("<!-- botwork-test: sample -->\n  ~~~botwork\n  Log |1|\n   ~~~~~\n").unwrap();
        assert_eq!(parsed[0].source, "Log |1|\n");
        assert_eq!(
            blocks("````text\n```\nbody\n`````\n").unwrap()[0].source,
            "```\nbody\n"
        );
    }

    #[test]
    fn unannotated_or_ignored_botwork_fences_fail() {
        for source in [
            "```botwork\n```",
            "```botwork,ignore\n```",
            "<!-- botwork-test: sample -->\n```botwork ignore\n```",
        ] {
            assert!(blocks(source).is_err(), "{source}");
        }
    }

    #[test]
    fn malformed_dangling_duplicate_and_misplaced_markers_fail() {
        for source in [
            "<!-- botwork-test: ../unsafe -->",
            "<!-- botwork-test: -->",
            "<!-- botwork-test: sample",
            "<!-- botwork-test: sample -->",
            "<!-- botwork-test: sample -->\n<!-- botwork-test: second -->",
            "<!-- botwork-test: sample -->\nprose\n```botwork\n```",
            "<!-- botwork-test: sample -->\n```rust\n```",
            "```text\nno closing fence",
        ] {
            assert!(blocks(source).is_err(), "{source}");
        }
    }

    #[test]
    fn shell_and_rust_examples_are_identified_without_execution() {
        let parsed = blocks("```sh\nprintf hi\n```\n```rust\nfn main() {}\n```\n").unwrap();
        assert_eq!(
            parsed
                .iter()
                .map(|block| block.language.as_str())
                .collect::<Vec<_>>(),
            ["sh", "rust"]
        );
    }

    #[test]
    fn rust_examples_cannot_opt_out_of_execution() {
        for flag in ["ignore", "no_run", "compile_fail"] {
            assert!(blocks(&format!("```rust,{flag}\nfn main() {{}}\n```\n")).is_err());
        }
    }
}
