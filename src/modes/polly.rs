use aws_sdk_polly::types::{Engine, Gender, LanguageCode, OutputFormat, TextType, VoiceId};
use serde::ser::SerializeStruct;
use small_fixed_array::FixedString;

use crate::Result;

pub type State = aws_sdk_polly::Client;

pub struct VoiceLocal {
    pub additional_language_codes: Option<Vec<LanguageCode>>,
    pub supported_engines: Option<Vec<Engine>>,
    pub language_code: Option<LanguageCode>,
    pub language_name: Option<String>,
    pub gender: Option<Gender>,
    pub name: Option<String>,
    pub id: Option<VoiceId>,
}

impl From<aws_sdk_polly::types::Voice> for VoiceLocal {
    fn from(v: aws_sdk_polly::types::Voice) -> Self {
        Self {
            additional_language_codes: v.additional_language_codes,
            supported_engines: v.supported_engines,
            language_code: v.language_code,
            language_name: v.language_name,
            gender: v.gender,
            name: v.name,
            id: v.id,
        }
    }
}

impl serde::Serialize for VoiceLocal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Voice", 7)?;
        state.serialize_field(
            "additional_language_codes",
            &self
                .additional_language_codes
                .as_ref()
                .map(|v| v.iter().map(LanguageCode::as_str).collect::<Vec<&str>>()),
        )?;
        state.serialize_field(
            "supported_engines",
            &self
                .supported_engines
                .as_ref()
                .map(|v| v.iter().map(Engine::as_str).collect::<Vec<&str>>()),
        )?;
        state.serialize_field(
            "language_code",
            &self.language_code.as_ref().map(LanguageCode::as_str),
        )?;
        state.serialize_field("gender", &self.gender.as_ref().map(Gender::as_str))?;
        state.serialize_field("id", &self.id.as_ref().map(VoiceId::as_str))?;
        state.serialize_field("language_name", &self.language_name)?;
        state.serialize_field("name", &self.name)?;
        state.end()
    }
}

pub async fn get_tts(
    state: &State,
    text: FixedString,
    voice: &str,
    speaking_rate: Option<u8>,
    preferred_format: Option<&str>,
) -> Result<(bytes::Bytes, Option<reqwest::header::HeaderValue>)> {
    let text = if let Some(speaking_rate) = speaking_rate {
        format!("<speak><prosody rate=\"{speaking_rate}%\">{text}</prosody></speak>")
    } else {
        text.into_string()
    };

    let resp = state
        .synthesize_speech()
        .set_text_type(Some(if speaking_rate.is_some() {
            TextType::Ssml
        } else {
            TextType::Text
        }))
        .set_output_format(
            preferred_format
                .and_then(|pf| match pf.to_lowercase().as_str() {
                    "mp3" => Some(OutputFormat::Mp3),
                    "pcm" => Some(OutputFormat::Pcm),
                    _ => None,
                })
                .or(Some(OutputFormat::OggVorbis)),
        )
        .set_engine(Some(Engine::Standard))
        .set_voice_id(Some(voice.into()))
        .set_text(Some(text))
        .send()
        .await?;

    Ok((
        resp.audio_stream.collect().await?.into_bytes(),
        resp.content_type
            .map(TryInto::try_into)
            .and_then(Result::ok),
    ))
}

static VOICES: tokio::sync::OnceCell<Vec<VoiceLocal>> = tokio::sync::OnceCell::const_new();
async fn get_voices_(state: &State) -> Result<Vec<VoiceLocal>> {
    let mut voices = Vec::new();
    let mut next_token = None;

    loop {
        let resp = state
            .describe_voices()
            .set_next_token(next_token)
            .send()
            .await?;

        if let Some(v) = resp.voices {
            voices.extend(v.into_iter().map(VoiceLocal::from).filter(|v| {
                v.supported_engines
                    .as_ref()
                    .is_some_and(|engines| engines.contains(&Engine::Standard))
            }));
        }
        if resp.next_token.is_none() {
            break Ok(voices);
        }

        next_token = resp.next_token;
    }
}

pub async fn check_voice(state: &State, voice: &str) -> Result<bool> {
    VOICES
        .get_or_try_init(|| get_voices_(state))
        .await
        .map(|voices| voices.iter().any(|s| s.id == Some(voice.into())))
}

pub async fn get_voices(state: &State) -> Result<Vec<String>> {
    VOICES
        .get_or_try_init(|| get_voices_(state))
        .await
        .map(|voices| {
            voices
                .iter()
                .filter_map(|v| v.id.as_ref())
                .map(VoiceId::as_str)
                .map(String::from)
                .collect()
        })
}

pub async fn get_raw_voices(state: &State) -> Result<&'static Vec<VoiceLocal>> {
    VOICES.get_or_try_init(|| get_voices_(state)).await
}
