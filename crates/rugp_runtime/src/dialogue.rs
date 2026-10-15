//! Message controls are independent of fonts, windowing and VM scheduling.
//! Vm60's parser (0x10014370) translates 1/2/3/5/newline into its ring
//! opcodes 2/4/5/3/1; its output loop is at 0x10015b10.
use anyhow::{Result, bail, ensure};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextPart {
    Glyph(char),
    Newline,
    Speaker(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Text(TextPart),
    Wait { clear: bool },
    Pause,
    Clear,
}
fn parse(text: &str) -> Result<VecDeque<Token>> {
    ensure!(text.len() <= 1024 * 1024, "oversized dialogue text");
    let mut tokens = VecDeque::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        let token = match ch {
            '\u{1}' => Token::Wait { clear: true },
            '\u{2}' => Token::Wait { clear: false },
            '\u{3}' => Token::Pause,
            '\u{5}' => Token::Clear,
            '\n' => Token::Text(TextPart::Newline),
            '[' => bail!("dialogue object substitution is not implemented"),
            '【' => {
                let mut name = String::new();
                loop {
                    let ch = chars
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("unterminated speaker name"))?;
                    if ch == '】' {
                        break;
                    }
                    ensure!(!ch.is_control() && name.len() < 128, "invalid speaker name");
                    name.push(ch);
                }
                Token::Text(TextPart::Speaker(name))
            }
            ch if ch.is_control() => bail!("unsupported dialogue control U+{:04X}", ch as u32),
            ch => Token::Text(TextPart::Glyph(ch)),
        };
        ensure!(tokens.len() < 65536, "too many dialogue tokens");
        tokens.push_back(token);
    }
    Ok(tokens)
}

pub struct Dialogue {
    tokens: VecDeque<Token>,
    pub page: Vec<TextPart>,
    speed_ms: u32,
    delay_ms: u32,
    wait: Option<bool>,
}
impl Dialogue {
    pub fn new(text: &str, speed_ms: u32, page: Vec<TextPart>) -> Result<Self> {
        ensure!(speed_ms <= 3000, "invalid message output speed");
        ensure!(page.len() <= 65536, "oversized retained message page");
        Ok(Self {
            tokens: parse(text)?,
            page,
            speed_ms,
            delay_ms: 0,
            wait: None,
        })
    }
    pub fn waiting(&self) -> bool {
        self.wait.is_some()
    }
    pub fn finished(&self) -> bool {
        self.wait.is_none() && self.tokens.is_empty() && self.delay_ms == 0
    }
    /// Advance until the clock budget is exhausted or an explicit input wait.
    /// Time spent waiting never carries over into a subsequently confirmed page.
    pub fn tick(&mut self, mut dt_ms: u32) -> Result<bool> {
        let mut dirty = false;
        while self.wait.is_none() {
            if dt_ms < self.delay_ms {
                self.delay_ms -= dt_ms;
                break;
            }
            dt_ms -= self.delay_ms;
            self.delay_ms = 0;
            let Some(token) = self.tokens.pop_front() else {
                break;
            };
            match token {
                Token::Text(part) => {
                    ensure!(
                        self.page.len() < 65536,
                        "retained message page budget exceeded"
                    );
                    let glyph = matches!(part, TextPart::Glyph(_));
                    self.page.push(part);
                    dirty = true;
                    if glyph {
                        self.delay_ms = self.speed_ms;
                    }
                }
                Token::Wait { clear } => {
                    self.wait = Some(clear);
                }
                Token::Pause => {
                    if self.speed_ms != 0 {
                        self.delay_ms = 250;
                    }
                }
                Token::Clear => {
                    self.page.clear();
                    dirty = true;
                }
            }
        }
        Ok(dirty)
    }
    /// A click during reveal finishes only the current page. It cannot also
    /// confirm a wait encountered by that same click.
    pub fn activate(&mut self) -> Result<bool> {
        let mut dirty = false;
        if let Some(clear) = self.wait.take() {
            if clear {
                self.page.clear();
                dirty = true;
            }
            dirty |= self.tick(0)?;
        } else {
            dirty |= self.tick(u32::MAX)?;
        }
        Ok(dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(d: &Dialogue) -> String {
        d.page
            .iter()
            .filter_map(|p| {
                if let TextPart::Glyph(c) = p {
                    Some(*c)
                } else {
                    None
                }
            })
            .collect()
    }
    #[test]
    fn native_page_wait_continue_wait_clear_and_unterminated_output() -> Result<()> {
        let mut d = Dialogue::new("甲\u{3}乙\n\u{1}丙\u{2}丁\u{5}戊", 0, vec![])?;
        assert!(d.tick(0)?);
        assert_eq!(text(&d), "甲乙");
        assert!(d.waiting());
        d.tick(u32::MAX)?;
        assert_eq!(text(&d), "甲乙");
        d.activate()?;
        assert_eq!(text(&d), "丙");
        assert!(d.waiting());
        d.activate()?;
        assert_eq!(text(&d), "戊");
        assert!(d.finished());
        Ok(())
    }
    #[test]
    fn clock_edges_short_pause_and_reveal_click_do_not_confirm() -> Result<()> {
        let mut d = Dialogue::new("A\u{3}B\u{1}C\u{1}", 10, vec![])?;
        d.tick(0)?;
        assert_eq!(text(&d), "A");
        d.tick(9)?;
        d.tick(1)?;
        assert_eq!(text(&d), "A");
        d.tick(249)?;
        assert_eq!(text(&d), "A");
        d.tick(1)?;
        assert_eq!(text(&d), "AB");
        d.activate()?; // reveal to first wait, not through it
        assert_eq!(text(&d), "AB");
        assert!(d.waiting());
        d.activate()?;
        assert_eq!(text(&d), "C");
        assert!(!d.waiting());
        d.tick(10)?;
        assert!(d.waiting());
        assert!(!d.finished());
        d.activate()?;
        assert!(d.finished());
        assert!(d.page.is_empty());
        Ok(())
    }
    #[test]
    fn names_and_unsupported_controls_are_not_printed_as_glyphs() -> Result<()> {
        let d = Dialogue::new("【話者】「例」\u{1}", 0, vec![])?;
        assert!(matches!(d.tokens.front(), Some(Token::Text(TextPart::Speaker(s))) if s == "話者"));
        for text in ["\0", "\u{4}", "[variable]", "【unfinished", "【bad\nname】"] {
            assert!(Dialogue::new(text, 0, vec![]).is_err());
        }
        assert!(Dialogue::new("", 3001, vec![]).is_err());
        assert!(Dialogue::new(&"a".repeat(65537), 0, vec![]).is_err());
        assert!(Dialogue::new(&"a".repeat(1024 * 1024 + 1), 0, vec![]).is_err());
        Ok(())
    }
    #[test]
    fn continuation_wait_retains_page_across_message_records() -> Result<()> {
        let mut first = Dialogue::new("A\u{2}", 0, vec![])?;
        first.tick(0)?;
        first.activate()?;
        assert!(first.finished());
        let mut next = Dialogue::new("B\u{1}", 0, first.page)?;
        next.tick(0)?;
        assert_eq!(text(&next), "AB");
        assert!(next.waiting());
        next.activate()?;
        assert!(next.finished());
        assert!(next.page.is_empty());
        Ok(())
    }
}
