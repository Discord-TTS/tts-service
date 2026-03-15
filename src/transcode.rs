//! A module for transcoding audio to Discord's Opus specifications, which will be referred to as dopus.

use bytes::Bytes;
use symphonia::core::{
    codecs::DecoderOptions,
    formats::FormatOptions,
    io::{MediaSourceStream, MediaSourceStreamOptions},
    meta::{Limit, MetadataOptions},
    probe::Hint,
};

pub const DISCORD_SAMPLE_RATE: u16 = 48 * 1000;

pub struct Transcoder {
    pool: rusty_pool::ThreadPool,
}

impl Transcoder {}

static FORMAT_OPTIONS: FormatOptions = FormatOptions {
    prebuild_seek_index: false,
    seek_index_fill_rate: 20,
    enable_gapless: false,
};

static METADATA_OPTIONS: MetadataOptions = MetadataOptions {
    limit_metadata_bytes: Limit::Maximum(0),
    limit_visual_bytes: Limit::Maximum(0),
};

static DECODER_OPTIONS: DecoderOptions = DecoderOptions { verify: false };

fn audio_to_dopus(audio: Vec<u8>) -> anyhow::Result<()> {
    let media = std::io::Cursor::new(audio);
    let result = symphonia::default::get_probe().format(
        &Hint::new(),
        MediaSourceStream::new(Box::new(media), MediaSourceStreamOptions::default()),
        &FORMAT_OPTIONS,
        &METADATA_OPTIONS,
    )?;

    let track = result
        .format
        .default_track()
        .expect("audio file should have a track");

    let decoder = symphonia::default::get_codecs().make(&track.codec_params, &DECODER_OPTIONS)?;

    Ok(())
}
