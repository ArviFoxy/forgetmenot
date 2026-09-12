//! YAML frontmatter between `---` lines, followed by a markdown body.
//!
//! The body is carried as the exact bytes that followed the closing delimiter
//! so that reading a file and writing it back changes nothing a person typed.
//! This module is the only place that names the YAML library, which keeps the
//! choice of library replaceable.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The line that opens and closes the frontmatter.
pub const DELIMITER: &str = "---";

/// A file that does not have the shape `---`, YAML, `---`, body.
#[derive(Debug, thiserror::Error)]
pub enum FrontmatterError {
    #[error("file does not start with a `---` line")]
    MissingOpeningDelimiter,
    #[error("frontmatter is not closed by a `---` line")]
    MissingClosingDelimiter,
    #[error("file is not valid UTF-8")]
    NotUtf8,
    #[error("frontmatter is not valid YAML: {0}")]
    Yaml(#[from] yaml_serde::Error),
}

/// The two halves of a file with frontmatter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Halves<'a> {
    /// The YAML text between the delimiter lines, without either of them.
    pub frontmatter: &'a str,
    /// Everything after the closing delimiter line, byte for byte.
    pub body: &'a str,
}

/// Split `text` at the frontmatter delimiters.
pub fn split(text: &str) -> Result<Halves<'_>, FrontmatterError> {
    let after_opening = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or(FrontmatterError::MissingOpeningDelimiter)?;

    let mut offset = 0;
    loop {
        let (line, next_offset, is_last_line) = match after_opening[offset..].find('\n') {
            Some(relative_end) => {
                let end = offset + relative_end;
                (&after_opening[offset..end], end + 1, false)
            }
            None => (&after_opening[offset..], after_opening.len(), true),
        };
        if line.trim_end_matches('\r') == DELIMITER {
            return Ok(Halves {
                frontmatter: &after_opening[..offset],
                body: &after_opening[next_offset..],
            });
        }
        if is_last_line {
            return Err(FrontmatterError::MissingClosingDelimiter);
        }
        offset = next_offset;
    }
}

/// Parse the frontmatter of `bytes` into `T` and return it with the body.
pub fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<(T, String), FrontmatterError> {
    let text = std::str::from_utf8(bytes).map_err(|_| FrontmatterError::NotUtf8)?;
    let halves = split(text)?;
    let parsed = yaml_serde::from_str(halves.frontmatter)?;
    Ok((parsed, halves.body.to_string()))
}

/// Render `frontmatter` and `body` back into file text.
pub fn render<T: Serialize>(frontmatter: &T, body: &str) -> Result<String, FrontmatterError> {
    let mut yaml = yaml_serde::to_string(frontmatter)?;
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    Ok(format!("{DELIMITER}\n{yaml}{DELIMITER}\n{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a scan that runs to the end of the file when the closing
    /// delimiter is missing, which would swallow a whole markdown document as
    /// frontmatter instead of reporting a malformed file.
    #[test]
    fn a_file_without_a_closing_delimiter_is_an_error() {
        let error = split("---\nname: a\nstill frontmatter\n").unwrap_err();
        assert!(
            matches!(error, FrontmatterError::MissingClosingDelimiter),
            "got {error:?}"
        );
    }

    /// Detects treating a `---` inside the body as the closing delimiter, or
    /// dropping the newline that separates the delimiter from the body.
    #[test]
    fn the_body_keeps_every_byte_after_the_first_closing_delimiter() {
        let halves = split("---\nname: a\n---\n\nbody line\n\n---\n\nmore\n").unwrap();
        assert_eq!(halves.frontmatter, "name: a\n");
        assert_eq!(halves.body, "\nbody line\n\n---\n\nmore\n");
    }

    /// Detects a file with no frontmatter being parsed as if it had empty
    /// frontmatter, which would give every field its default silently.
    #[test]
    fn a_file_without_an_opening_delimiter_is_an_error() {
        let error = split("name: a\n---\n").unwrap_err();
        assert!(
            matches!(error, FrontmatterError::MissingOpeningDelimiter),
            "got {error:?}"
        );
    }
}
