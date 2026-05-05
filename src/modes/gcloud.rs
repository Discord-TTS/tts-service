use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    http::HeaderValue,
    response::{IntoResponse, Response},
};
use base64::Engine;
use bytes::Bytes;
use serde_json::to_value;
use tokio::sync::RwLock;

use crate::{Result, TTSEngine, TTSParams, TTSVoices};

const GOOGLE_API_BASE: &str = "https://texttospeech.googleapis.com/";
const MAX_GCLOUD_RATE: f32 = 4.0;
static VOICES: tokio::sync::OnceCell<Vec<GoogleVoice>> = tokio::sync::OnceCell::const_new();

#[derive(Clone)]
pub struct State {
    service_account: ServiceAccount,
    reqwest: reqwest::Client,
    jwt_token: Arc<RwLock<JWTToken>>,
}
#[derive(Debug, Clone)]
struct JWTToken {
    token: String,
    expire_time: std::time::SystemTime,
}
impl JWTToken {
    fn has_expired(&self) -> bool {
        let current_time = std::time::SystemTime::now();
        current_time > self.expire_time
    }
}

impl State {
    pub(crate) fn new(reqwest: reqwest::Client) -> Result<Self> {
        let service_account: ServiceAccount = serde_json::from_str(&std::fs::read_to_string(
            std::env::var("GOOGLE_APPLICATION_CREDENTIALS")?,
        )?)?;

        let jwt_token = generate_jwt(
            &service_account.private_key,
            &service_account.client_email,
            std::time::SystemTime::now(),
        )?;

        Ok(Self {
            service_account,
            reqwest,
            jwt_token: Arc::new(RwLock::new(jwt_token)),
        })
    }
    async fn get_voices_(&self) -> Result<Vec<GoogleVoice>> {
        #[derive(serde::Deserialize)]
        struct VoiceResponse {
            voices: Vec<GoogleVoice>,
        }

        let jwt_token = self.refresh_jwt().await?;
        let reqwest = self.reqwest.clone();

        let resp: VoiceResponse = reqwest
            .get(format!("{GOOGLE_API_BASE}v1/voices"))
            .header("Authorization", format!("Bearer {jwt_token}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(resp.voices)
    }

    async fn refresh_jwt(&self) -> Result<String> {
        let current_time = std::time::SystemTime::now();
        if self.jwt_token.read().await.has_expired() {
            let mut write = self.jwt_token.write().await;
            *write = generate_jwt(
                &self.service_account.private_key,
                &self.service_account.client_email,
                current_time,
            )?;
        }
        Ok(self.jwt_token.read().await.token.clone())
    }
}

#[async_trait]
impl TTSEngine for State {
    async fn check_params(&self, params: TTSParams<'_>) -> Option<Response> {
        if params.speaking_rate > MAX_GCLOUD_RATE {
            Some(format!("Speaking rate faster than max of {MAX_GCLOUD_RATE}").into_response())
        } else if params.speaking_rate < 0. {
            Some("Speaking rate cannot be negative".into_response())
        } else if !self
            .get_voices()
            .await
            .ok()?
            .nice
            .iter()
            .any(|s| s.as_str() == params.voice)
        {
            Some(format!("Voice {} is not known", params.voice).into_response())
        } else {
            None
        }
    }
    fn check_length(&self, audio: &[u8], max_length: u64) -> bool {
        true
    }
    async fn speak(
        &self,
        text: &str,
        params: TTSParams<'_>,
        preferred_format: Option<&str>,
    ) -> Result<(Bytes, Option<HeaderValue>)> {
        let jwt_token = self.refresh_jwt().await?;
        let reqwest = self.reqwest.clone();

        let audio_encoding = preferred_format
            .and_then(|pf| AudioEncoding::from_str(&pf.to_uppercase()))
            .unwrap_or(AudioEncoding::OGG_OPUS);

        let resp = reqwest
            .post(format!("{GOOGLE_API_BASE}v1/text:synthesize"))
            .json(&generate_google_json(
                text,
                params.lang,
                params.speaking_rate,
                audio_encoding.as_str(),
            )?)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {jwt_token}"),
            )
            .send()
            .await?
            .error_for_status()?;

        let resp_raw = resp.bytes().await?;
        let audio_response: AudioResponse = serde_json::from_slice(&resp_raw)?;

        Ok((
            bytes::Bytes::from(
                base64::engine::general_purpose::STANDARD.decode(audio_response.audio_content)?,
            ),
            Some(reqwest::header::HeaderValue::from_static(
                audio_encoding.content_type(),
            )),
        ))
    }
    async fn get_voices(&self) -> Result<TTSVoices> {
        let raw_voices = VOICES.get_or_try_init(|| self.get_voices_()).await?;
        Ok(TTSVoices {
            raw: to_value(raw_voices)?,
            nice: raw_voices
                .iter()
                .filter_map(|gvoice| {
                    gvoice
                        .name
                        .splitn(3, '-')
                        .nth(2)?
                        .split_once('-')
                        .filter(|(mode, _)| *mode == "Standard")
                        .map(|(_, variant)| {
                            let [mut language] = gvoice.languageCodes.clone();
                            language.push(' ');
                            language.push_str(variant);
                            language
                        })
                })
                .collect(),
        })
    }
    fn get_defaults(&self) -> TTSParams<'static> {
        todo!()
    }
}

#[derive(serde::Deserialize)]
struct AudioResponse<'a> {
    #[serde(borrow, rename = "audioContent")]
    audio_content: &'a str,
}

#[derive(Clone, serde::Deserialize)]
struct ServiceAccount {
    pub private_key: String,
    pub client_email: String,
}

#[derive(serde::Deserialize, serde::Serialize, Default, Clone, Copy)]
pub enum Gender {
    #[serde(rename = "MALE")]
    Male,
    #[serde(rename = "FEMALE")]
    Female,
    #[serde(rename = "SSML_VOICE_GENDER_UNSPECIFIED")]
    #[default]
    Unspecified,
}

#[allow(non_snake_case)]
#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct GoogleVoice {
    pub name: String,
    #[serde(default)]
    pub ssmlGender: Gender,
    pub languageCodes: [String; 1],
}

#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
#[derive(Clone, Copy)]
enum AudioEncoding {
    LINEAR16,
    OGG_OPUS,
    MULAW,
    ALAW,
    MP3,
}

impl AudioEncoding {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "LINEAR16" => Some(AudioEncoding::LINEAR16),
            "OPUS" => Some(AudioEncoding::OGG_OPUS),
            "MULAW" => Some(AudioEncoding::MULAW),
            "ALAW" => Some(AudioEncoding::ALAW),
            "MP3" => Some(AudioEncoding::MP3),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            AudioEncoding::LINEAR16 => "LINEAR16",
            AudioEncoding::OGG_OPUS => "OGG_OPUS",
            AudioEncoding::MULAW => "MULAW",
            AudioEncoding::ALAW => "ALAW",
            AudioEncoding::MP3 => "MP3",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::LINEAR16 | Self::ALAW | Self::MULAW => "audio/wav",
            Self::OGG_OPUS => "audio/opus",
            Self::MP3 => "audio/mpeg",
        }
    }
}

fn generate_google_json(
    content: &str,
    lang: &str,
    speaking_rate: f32,
    audio_encoding: &str,
) -> Result<impl serde::Serialize> {
    let (lang, variant) = lang
        .split_once(' ')
        .ok_or_else(|| anyhow::anyhow!("{lang} cannot be parsed into lang and variant"))?;

    Ok(serde_json::json!({
        "input": {
            "text": content
        },
        "voice": {
            "languageCode": lang,
            "name": format!("{lang}-Standard-{variant}"),
        },
        "audioConfig": {
            "audioEncoding": audio_encoding,
            "speakingRate": speaking_rate
        }
    }))
}

fn generate_jwt(
    private_key_raw: &str,
    client_email: &str,
    current_time: std::time::SystemTime,
) -> Result<JWTToken> {
    let private_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_raw.as_bytes())?;

    let mut headers = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    headers.kid = Some(private_key_raw.to_string());

    let new_expire_time = current_time + std::time::Duration::from_hours(1);
    let payload = serde_json::json!({
        "exp": new_expire_time.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "iat": current_time.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "aud": GOOGLE_API_BASE,
        "iss": client_email,
        "sub": client_email,
    });

    let jwt_token = jsonwebtoken::encode(&headers, &payload, &private_key)?;
    Ok(JWTToken {
        token: jwt_token,
        expire_time: new_expire_time,
    })
}
