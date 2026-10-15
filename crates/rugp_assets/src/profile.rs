//! CStdb schema 1's CProfile text, as serialized by the 2008 engine.
//! This is not INI: entries are semicolon-separated, string values are braced
//! (with balanced nesting), and `#` introduces a numeric value.
use crate::binary::Reader;
use anyhow::{Result, ensure};

#[derive(Clone, Default)]
pub struct Profile {
    text: String,
}
impl Profile {
    pub fn parse_object(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        r.skip(3)?;
        let profile = Self::from_text(r.string()?)?;
        ensure!(r.remaining() == 0, "trailing CStdb bytes");
        Ok(profile)
    }
    pub fn from_text(text: String) -> Result<Self> {
        ensure!(
            text.len() <= 16 * 1024 * 1024 && !text.contains('\0'),
            "invalid/oversized profile text"
        );
        Ok(Self { text })
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    // Native IdentifyProfile scans semicolon boundaries literally, and returns
    // the first exact key match. Do not normalize case or Japanese characters.
    fn locate(&self, key: &str) -> Option<usize> {
        let mut start = 0;
        for part in self.text.split(';') {
            let trimmed = part.trim_start_matches([' ', '\t', '\r', '\n']);
            if trimmed
                .strip_prefix(key)
                .is_some_and(|s| s.starts_with('='))
            {
                return Some(start + part.len() - trimmed.len() + key.len() + 1);
            }
            start += part.len() + 1;
        }
        None
    }
    /// ReadProfileString only accepts string-valued entries. Numeric entries
    /// fall back to the caller's default rather than being stringified.
    pub fn read_string<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        let Some(start) = self.locate(key) else {
            return default;
        };
        let value = self.text[start..].trim_start_matches([' ', '\t', '\r', '\n']);
        let Some(open) = value.chars().next() else {
            return default;
        };
        let close = match open {
            '{' => '}',
            '<' => '>',
            '"' => '"',
            _ => return default,
        };
        let body = &value[1..];
        if open == '"' {
            return body.split('"').next().unwrap_or(body);
        }
        let end = balanced_end(body, open, close).unwrap_or(body.len());
        &body[..end]
    }
    /// CreateProfile's string overload, preserving unrelated native text.
    pub fn write_string(&mut self, key: &str, value: &str) -> Result<()> {
        ensure!(
            !key.is_empty() && key.len() <= 4096 && !key.contains([';', '=', '\0']),
            "invalid profile key"
        );
        ensure!(
            value.len() <= 1024 * 1024 && !value.contains('\0'),
            "invalid/oversized profile value"
        );
        let replacement = format!("{{{value}}}");
        let next = if let Some(start) = self.locate(key) {
            let end = section_end(&self.text, start);
            format!(
                "{}{}{}",
                &self.text[..start],
                replacement,
                &self.text[end..]
            )
        } else if self.text.is_empty() {
            format!("{key}={replacement}")
        } else {
            format!("{};{key}={replacement}", self.text)
        };
        ensure!(
            next.len() <= 16 * 1024 * 1024,
            "profile exceeds memory limit"
        );
        self.text = next;
        Ok(())
    }
}
fn balanced_end(body: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 1usize;
    for (i, c) in body.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}
fn section_end(text: &str, start: usize) -> usize {
    let mut at = start;
    while at < text.len() {
        let c = text[at..].chars().next().unwrap();
        if c == ';' {
            return at;
        }
        if let Some(close) = match c {
            '{' => Some('}'),
            '<' => Some('>'),
            _ => None,
        } {
            let body = at + 1;
            if let Some(end) = balanced_end(&text[body..], c, close) {
                at = body + end + 1;
                continue;
            }
        }
        at += c.len_utf8();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_profile_strings_numbers_first_key_and_nesting() -> Result<()> {
        let mut p = Profile::from_text("\tcount={42};日本語={inside;{nested}};number=#17;quoted=\"hello\";angle=<a<b>>;count={99}".into())?;
        assert_eq!(p.read_string("count", "0"), "42");
        assert_eq!(p.read_string("日本語", ""), "inside;{nested}");
        assert_eq!(p.read_string("number", "0"), "0");
        assert_eq!(p.read_string("quoted", ""), "hello");
        assert_eq!(p.read_string("angle", ""), "a<b>");
        assert_eq!(p.read_string("Count", "missing"), "missing");
        p.write_string("日本語", "updated;{ok}")?;
        assert_eq!(p.read_string("日本語", ""), "updated;{ok}");
        assert_eq!(p.read_string("number", "fallback"), "fallback");
        p.write_string("count", "5")?;
        p.write_string("new", "6")?;
        assert_eq!(p.read_string("count", "0"), "5");
        assert_eq!(p.read_string("new", "0"), "6");
        assert!(p.text().contains(";count={99}"));
        assert!(p.write_string("invalid;key", "1").is_err());
        Ok(())
    }
    #[test]
    fn cstdb_serialized_string_is_bounded() -> Result<()> {
        let data = [0, 0, 0, 5, b'a', b'=', b'{', b'1', b'}'];
        assert_eq!(Profile::parse_object(&data)?.read_string("a", "0"), "1");
        for len in 0..data.len() {
            assert!(Profile::parse_object(&data[..len]).is_err());
        }
        assert!(Profile::parse_object(&[0, 0, 0, 0, 1]).is_err());
        Ok(())
    }
}
