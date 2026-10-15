use super::*;
use crate::video::VideoPlayer;
use rugp_assets::crsi::VideoSequence;

impl Game {
    pub(super) fn play_video(
        &mut self,
        target: &ObjectRef,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CVideoSeq" && c.schema == 4),
            "unsupported video-sequence object"
        );
        let id = target.offset.context("video sequence offset missing")?;
        let size = target.size.context("video sequence size missing")?;
        ensure!(size <= 1024 * 1024, "oversized video sequence");
        let defaults = VideoSequence::parse(
            &self.archive.read_at(id, size as usize)?,
            self.archive.address_shift,
        )?;
        ensure!(
            defaults.x == 0 && defaults.y == 0,
            "positioned movie playback is not implemented"
        );
        ensure!(
            number(fields, "同期属性", 0)? == 0x8000,
            "unsupported video synchronization attributes"
        );
        ensure!(
            number(fields, "拡張スタイル", 0)? == 0,
            "unsupported movie playback style"
        );
        ensure!(
            [0, 1].contains(&number(fields, "ループ回数", 0)?),
            "looping movie playback is not implemented"
        );
        let Some(Value::Reference(window)) = fields.get("ウインドウ") else {
            bail!("movie target window missing");
        };
        ensure!(
            window.offset == Some(self.primary_buffer),
            "non-primary movie output is not implemented"
        );
        let Some(Value::Reference(source)) = fields.get("ストリーム映像") else {
            bail!("movie source missing");
        };
        ensure!(
            self.video.as_ref().is_none_or(|(_, p)| p.finished()),
            "concurrent movie playback is not implemented"
        );
        ensure!(
            self.transition.is_none(),
            "concurrent movie/transition is not implemented"
        );
        let player = VideoPlayer::open(Arc::clone(&self.archive), source, defaults.rate)?;
        ensure!(
            player.size() == self.size(),
            "movie output scaling is not implemented"
        );
        self.frame.rgba.copy_from_slice(&player.image().rgba);
        self.video = Some((id, player));
        Ok(())
    }
}
