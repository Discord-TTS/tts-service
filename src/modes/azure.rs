use axum::http::HeaderValue;
use azure_speech::{
    stream::StreamExt,
    synthesizer::{self, ssml::ssml},
};

use crate::Result;

type State = synthesizer::Client;

pub async fn get_tts(
    state: &State,
    text: &str,
    voice: &str,
    speaking_rate: f32,
) -> Result<(bytes::Bytes, Option<HeaderValue>)> {
    let mut stream = {
        let text_elem = ssml::Element::Text(ssml::Text::from(text));

        let voice_param = ssml::VoiceConfig::named(voice);
        let voice_elem = ssml::Element::Voice(ssml::Voice::new(voice_param, [text_elem]));

        let rate_param = ssml::ProsodyControl::from(ssml::ProsodyRate::Rate(speaking_rate));
        let rate_elem = ssml::Prosody::new(rate_param, [voice_elem]);

        let ssml = ssml::Speak::new(Some("en-us"), [ssml::Element::Prosody(rate_elem)]);
        state.synthesize(ssml).await?
    };

    let mut audio_bytes = Vec::new();
    loop {
        match stream.next().await {
            Some(Ok(synthesizer::Event::Synthesising(_, audio_chunk))) => {
                if audio_bytes.is_empty() {
                    audio_bytes = audio_chunk;
                } else {
                    audio_bytes.extend(audio_chunk);
                }
            }
            Some(Ok(synthesizer::Event::Synthesised(_))) => {
                let audio_bytes = bytes::Bytes::from_owner(audio_bytes);
                break Ok((audio_bytes, Some(HeaderValue::from_static("audio/ogg"))));
            }
            Some(Ok(_)) => {}
            Some(Err(err)) => {
                let err = anyhow::Error::from(err).context("Audio stream error while synthesizing");
                break Err(err);
            }
            None => {
                let err = anyhow::anyhow!("Audio stream disconnected while synthesizing");
                break Err(err);
            }
        }
    }
}

pub fn get_raw_voices() -> &'static [&'static str] {
    return &["en-US-JennyNeural"];
}

pub fn get_voices() -> &'static [&'static str] {
    get_raw_voices()
}
