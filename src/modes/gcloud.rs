use base64::Engine;
use tokio::sync::RwLock;

use crate::{Result, transcode::DISCORD_SAMPLE_RATE};

const GOOGLE_API_BASE: &str = "https://texttospeech.googleapis.com/";

#[derive(Clone)]
pub struct State {
    service_account: ServiceAccount,
    expire_time: std::time::SystemTime,
    reqwest: reqwest::Client,
    jwt_token: String,
}

impl State {
    pub(crate) fn new(reqwest: reqwest::Client) -> Result<RwLock<Self>> {
        let service_account: ServiceAccount = serde_json::from_str(&std::fs::read_to_string(
            std::env::var("GOOGLE_APPLICATION_CREDENTIALS").unwrap(),
        )?)?;

        let (jwt_token, expire_time) = generate_jwt(
            service_account.private_key.clone(),
            &service_account.client_email,
            std::time::SystemTime::now(),
        )?;

        Ok(RwLock::new(Self {
            service_account,
            expire_time,
            reqwest,
            jwt_token,
        }))
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

fn generate_google_json(
    content: &str,
    lang: &str,
    speaking_rate: f32,
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
            "audioEncoding": "OGG_OPUS",
            "sampleRateHertz": DISCORD_SAMPLE_RATE,
            "speakingRate": speaking_rate
        }
    }))
}

fn generate_jwt(
    private_key_raw: String,
    client_email: &str,
    current_time: std::time::SystemTime,
) -> Result<(String, std::time::SystemTime)> {
    let private_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_raw.as_bytes())?;

    let mut headers = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    headers.kid = Some(private_key_raw);

    let new_expire_time = current_time + std::time::Duration::from_hours(1);
    let payload = serde_json::json!({
        "exp": new_expire_time.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "iat": current_time.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "aud": GOOGLE_API_BASE,
        "iss": client_email,
        "sub": client_email,
    });

    let jwt_token = jsonwebtoken::encode(&headers, &payload, &private_key)?;
    Ok((jwt_token, new_expire_time))
}

async fn refresh_jwt(state: &RwLock<State>) -> Result<String> {
    let current_time = std::time::SystemTime::now();
    let (expire_time, current_jwt_token, service_account) = {
        let state = state.read().await;
        (
            state.expire_time,
            state.jwt_token.clone(),
            state.service_account.clone(),
        )
    };

    if current_time > expire_time {
        let (jwt_token, new_expire_time) = generate_jwt(
            service_account.private_key.clone(),
            &service_account.client_email,
            current_time,
        )?;

        let mut state = state.write().await;

        state.jwt_token.clone_from(&jwt_token);
        state.expire_time = new_expire_time;

        Ok(jwt_token)
    } else {
        Ok(current_jwt_token)
    }
}

pub async fn get_tts(
    state: &RwLock<State>,
    text: &str,
    lang: &str,
    speaking_rate: f32,
) -> Result<bytes::Bytes> {
    let jwt_token = refresh_jwt(state).await?;
    let reqwest = state.read().await.reqwest.clone();

    let resp = reqwest
        .post(format!("{GOOGLE_API_BASE}v1/text:synthesize"))
        .json(&generate_google_json(text, lang, speaking_rate)?)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {jwt_token}"),
        )
        .send()
        .await?
        .error_for_status()?;

    let resp_raw = resp.bytes().await?;
    let audio_response: AudioResponse = serde_json::from_slice(&resp_raw)?;

    Ok(bytes::Bytes::from(
        base64::engine::general_purpose::STANDARD.decode(audio_response.audio_content)?,
    ))
}

static VOICES: tokio::sync::OnceCell<Vec<GoogleVoice>> = tokio::sync::OnceCell::const_new();
async fn get_voices_(state: &RwLock<State>) -> Result<Vec<GoogleVoice>> {
    #[derive(serde::Deserialize)]
    struct VoiceResponse {
        voices: Vec<GoogleVoice>,
    }

    let jwt_token = refresh_jwt(state).await?;
    let reqwest = state.read().await.reqwest.clone();

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

pub async fn check_voice(state: &RwLock<State>, voice: &str) -> Result<bool> {
    Ok(get_voices(state).await?.iter().any(|s| s.as_str() == voice))
}

pub async fn get_raw_voices(state: &RwLock<State>) -> Result<&'static Vec<GoogleVoice>> {
    VOICES.get_or_try_init(|| get_voices_(state)).await
}

pub async fn get_voices(state: &RwLock<State>) -> Result<Vec<String>> {
    Ok(VOICES
        .get_or_try_init(|| get_voices_(state))
        .await?
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
        .collect())
}
