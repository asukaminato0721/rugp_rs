use anyhow::{Context, Result};
use rugp_assets::{
    RioArchive,
    oor::{Oor, SetupHeaders, VorbisDecoder},
};
#[test]
#[ignore = "set RUGP_TEST_GAME to the retail game directory"]
fn retail_voice_and_eos_tail_regressions() -> Result<()> {
    let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
    let root = std::path::Path::new(&root);
    let archive = RioArchive::open(root)?;
    let headers = SetupHeaders::from_plugin(&std::fs::read(root.join("Plugins/riooor.rpo"))?);
    assert_eq!(headers.len(), 5);
    for (offset, size, tail) in [
        (0xf4800c9, 29054, 0),
        (0x5f547642, 25764, 31),
        (0xa5be6b7b, 25477, 11),
    ] {
        let bytes = archive.read_at(offset, size)?;
        let stream = Oor::parse(&bytes)?;
        assert_eq!(stream.trailing_data.len(), tail);
        let mut decoder = VorbisDecoder::new(stream, &headers)?;
        let mut frames = 0;
        let mut nonzero = false;
        while let Some(samples) = decoder.next_samples()? {
            frames += samples.len() / decoder.info.channels as usize;
            nonzero |= samples.iter().any(|v| *v != 0);
        }
        assert!(nonzero && frames > 44100);
        if offset == 0xf4800c9 {
            assert_eq!(frames, 144320);
        }
        println!("{offset:#x}: {frames} PCM frames, {tail} EOS tail bytes");
    }
    Ok(())
}
