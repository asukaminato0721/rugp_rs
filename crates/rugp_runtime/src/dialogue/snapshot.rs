//! Logical text progress only. The enclosing runtime snapshot also needs the
//! target window, retained page, voice actor, forced-clock flag and pending VM
//! instruction. Restoring never reparses/replays already consumed controls.
use super::{Dialogue, TextPart, Token};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DialogueSnapshot {
    version: u32,
    tokens: VecDeque<Token>,
    page: Vec<TextPart>,
    speed_ms: u32,
    delay_ms: u32,
    wait: Option<bool>,
}
impl Dialogue {
    pub fn snapshot(&self) -> Result<DialogueSnapshot> {
        let snapshot = DialogueSnapshot {
            version: 1,
            tokens: self.tokens.clone(),
            page: self.page.clone(),
            speed_ms: self.speed_ms,
            delay_ms: self.delay_ms,
            wait: self.wait,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}
impl DialogueSnapshot {
    pub fn restore(&self) -> Result<Dialogue> {
        self.validate()?;
        Ok(Dialogue {
            tokens: self.tokens.clone(),
            page: self.page.clone(),
            speed_ms: self.speed_ms,
            delay_ms: self.delay_ms,
            wait: self.wait,
        })
    }
    fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported dialogue snapshot version");
        ensure!(
            self.tokens.len() <= 65_536 && self.page.len() <= 65_536,
            "dialogue snapshot exceeds token/page limit"
        );
        ensure!(self.speed_ms <= 3000, "invalid dialogue snapshot speed");
        let maximum_delay = self.speed_ms.max(if self.speed_ms == 0 { 0 } else { 250 });
        ensure!(
            self.delay_ms <= maximum_delay && (self.wait.is_none() || self.delay_ms == 0),
            "invalid dialogue snapshot delay/wait state"
        );
        for part in self
            .page
            .iter()
            .chain(self.tokens.iter().filter_map(|token| {
                if let Token::Text(part) = token {
                    Some(part)
                } else {
                    None
                }
            }))
        {
            match part {
                TextPart::Glyph(ch) => ensure!(
                    !ch.is_control(),
                    "control code stored as dialogue snapshot glyph"
                ),
                // The parser checks the byte count BEFORE the last character;
                // a valid four-byte final character can bring it up to 131.
                TextPart::Speaker(name) => ensure!(
                    name.len() <= 131 && !name.chars().any(char::is_control),
                    "invalid dialogue snapshot speaker"
                ),
                TextPart::Newline => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roundtrip(dialogue: &Dialogue) -> DialogueSnapshot {
        serde_json::from_slice(&serde_json::to_vec(&dialogue.snapshot().unwrap()).unwrap()).unwrap()
    }
    fn equivalent(original: &Dialogue, restored: &Dialogue) {
        assert_eq!(original.snapshot().unwrap(), restored.snapshot().unwrap());
        assert_eq!(original.waiting(), restored.waiting());
        assert_eq!(original.finished(), restored.finished());
    }
    #[test]
    fn partial_glyph_and_pause_delays_continue_without_revealing_or_confirming_text() -> Result<()>
    {
        let mut original = Dialogue::new("AB\u{3}C\u{1}D\u{2}E", 100, vec![])?;
        original.tick(150)?;
        assert_eq!(original.delay_ms, 50);
        let mut restored = roundtrip(&original).restore()?;
        equivalent(&original, &restored);
        for elapsed in [49, 1, 249, 1, 99, 1, 1000, 0] {
            assert_eq!(original.tick(elapsed)?, restored.tick(elapsed)?);
            equivalent(&original, &restored);
        }
        assert!(restored.waiting());
        assert_eq!(original.activate()?, restored.activate()?);
        equivalent(&original, &restored);
        for elapsed in [99, 1, 100] {
            assert_eq!(original.tick(elapsed)?, restored.tick(elapsed)?);
            equivalent(&original, &restored);
        }
        assert!(restored.waiting());
        Ok(())
    }
    #[test]
    fn restored_input_waits_keep_clear_vs_continue_semantics_and_do_not_replay_controls()
    -> Result<()> {
        let mut original = Dialogue::new("A\u{2}B\u{1}C\u{5}D\u{1}", 0, vec![])?;
        original.tick(0)?;
        let mut restored = roundtrip(&original).restore()?;
        assert!(restored.waiting());
        assert!(!restored.tick(u32::MAX)?);
        assert_eq!(restored.page, vec![TextPart::Glyph('A')]);
        original.activate()?;
        restored.activate()?;
        equivalent(&original, &restored);
        assert_eq!(
            restored.page,
            vec![TextPart::Glyph('A'), TextPart::Glyph('B')]
        );
        restored = roundtrip(&restored).restore()?;
        assert_eq!(original.activate()?, restored.activate()?);
        equivalent(&original, &restored);
        assert_eq!(restored.page, vec![TextPart::Glyph('D')]);
        original.activate()?;
        restored.activate()?;
        equivalent(&original, &restored);
        assert!(restored.page.is_empty() && restored.finished());
        Ok(())
    }
    #[test]
    fn last_glyph_delay_empty_finished_stream_and_retained_speakers_roundtrip() -> Result<()> {
        let mut original = Dialogue::new(
            "A",
            100,
            vec![TextPart::Speaker("Example".into()), TextPart::Newline],
        )?;
        original.tick(0)?;
        assert!(original.tokens.is_empty() && !original.finished());
        let mut restored = roundtrip(&original).restore()?;
        original.tick(99)?;
        restored.tick(99)?;
        equivalent(&original, &restored);
        assert!(!restored.finished());
        original.tick(1)?;
        restored.tick(1)?;
        equivalent(&original, &restored);
        assert!(restored.finished());
        assert!(roundtrip(&restored).restore()?.finished());
        let text = format!("【{}{}】\u{1}", "a".repeat(127), '😀');
        let original = Dialogue::new(&text, 0, vec![])?;
        equivalent(&original, &roundtrip(&original).restore()?);
        Ok(())
    }
    #[test]
    fn corrupt_delays_controls_versions_and_unbounded_text_are_rejected() -> Result<()> {
        let original = Dialogue::new("A\u{1}", 100, vec![])?;
        let mut bad = roundtrip(&original);
        bad.version = 9;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.speed_ms = 3001;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.delay_ms = 251;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.wait = Some(true);
        bad.delay_ms = 1;
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.tokens = VecDeque::from(vec![Token::Pause; 65_537]);
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.page = vec![TextPart::Glyph('x'); 65_537];
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.page = vec![TextPart::Glyph('\u{1}')];
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.tokens
            .push_front(Token::Text(TextPart::Speaker("a".repeat(132))));
        assert!(bad.restore().is_err());
        let mut bad = roundtrip(&original);
        bad.tokens
            .push_front(Token::Text(TextPart::Speaker("bad\nname".into())));
        assert!(bad.restore().is_err());
        let original = Dialogue::new("A", 0, vec![])?;
        let mut bad = roundtrip(&original);
        bad.delay_ms = 1;
        assert!(bad.restore().is_err());
        Ok(())
    }
}
