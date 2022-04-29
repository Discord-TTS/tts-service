use tokio::sync::RwLock;

use crate::Result;


#[derive(Clone)]
pub(crate) struct State {
    service_account: ServiceAccount,
    expire_time: std::time::SystemTime,
    reqwest: reqwest::Client,
    jwt_token: String,
}

impl State {
    pub(crate) fn new() -> Result<RwLock<Self>> {
        let service_account = serde_json::from_str(&std::fs::read_to_string(std::env::var("GOOGLE_APPLICATION_CREDENTIALS").unwrap())?)?;
        let (jwt_token, expire_time) = generate_jwt(&service_account, &std::time::SystemTime::now())?.unwrap();

        Ok(RwLock::new(Self {
            service_account, expire_time, jwt_token,
            reqwest: reqwest::Client::new()
        }))
    } 
}


#[derive(serde::Deserialize)]
struct AudioResponse<'a> {
    #[serde(borrow, rename="audioContent")]
    audio_content: &'a str,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct ServiceAccount {
    pub private_key: String,
    pub client_email: String,
}

#[cfg(feature="premium")]
#[allow(non_snake_case)]
#[derive(serde::Deserialize, Debug)]
pub struct GoogleVoice<'a> {
    pub name: String,
    pub ssmlGender: &'a str,
    pub languageCodes: [String; 1],
}


fn generate_google_json(content: &str, lang: &str, speaking_rate: f32) -> Result<serde_json::Value> {
    let (lang, variant) = lang.split_once(' ').ok_or_else(|| 
        anyhow::anyhow!("{} cannot be parsed into lang and variant", lang)
    )?;

    Ok(
        serde_json::json!({
            "input": {
                "text": content
            },
            "voice": {
                "languageCode": lang,
                "name": format!("{}-Standard-{}", lang, variant),
            },
            "audioConfig": {
                "audioEncoding": "OGG_OPUS",
                "speakingRate": speaking_rate
            }
        })
    )
}


fn generate_jwt(service_account: &ServiceAccount, expire_time: &std::time::SystemTime) -> Result<Option<(String, std::time::SystemTime)>> {
    let current_time = std::time::SystemTime::now();
    if &current_time > expire_time  {
        let private_key_raw = &service_account.private_key;
        let private_key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_raw.as_bytes())?;

        let mut headers = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        headers.kid = Some(private_key_raw.clone());

        let new_expire_time = current_time + std::time::Duration::from_secs(3600);
        let payload = serde_json::json!({
            "exp": new_expire_time.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
            "iat": current_time.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
            "aud": "https://texttospeech.googleapis.com/",
            "iss": service_account.client_email,
            "sub": service_account.client_email,
        });

        Ok(Some((jsonwebtoken::encode(&headers, &payload, &private_key)?, new_expire_time)))
    } else {
        Ok(None)
    }
}

pub(crate) async fn get_tts(state: &RwLock<State>, text: &str, lang: &str, speaking_rate: f32) -> Result<bytes::Bytes> {
    let State{jwt_token, expire_time, reqwest, service_account} = state.read().await.clone();

    let jwt_token = {
        if let Some((new_token, new_expire)) = generate_jwt(
            &service_account,
            &expire_time,
        )? {
            let mut state_write = state.write().await;

            state_write.expire_time = new_expire;
            state_write.jwt_token = new_token;
        };

        jwt_token.clone()
    };

    let resp = reqwest.post("https://texttospeech.googleapis.com/v1/text:synthesize")
        .header("Authorization", format!("Bearer {jwt_token}"))
        .json(&generate_google_json(text, lang, speaking_rate)?)
        .send().await?.error_for_status()?;

    let resp_raw = resp.bytes().await?;
    let audio_response: AudioResponse = serde_json::from_slice(&resp_raw)?;
    Ok(bytes::Bytes::from(base64::decode(audio_response.audio_content)?))
}


static VOICES: once_cell::sync::Lazy<Vec<String>> = once_cell::sync::Lazy::new(|| {
    let raw_map: Vec<GoogleVoice<'_>> = serde_json::from_str(include_str!("data/voices-premium.json")).unwrap();
    raw_map.into_iter().filter_map(|gvoice|  {
        let mode_variant: String = gvoice.name.split_inclusive('-').skip(2).collect();
        let (mode, variant) = mode_variant.split_once('-').unwrap();

        (mode == "Standard").then(|| {
            let [language] = gvoice.languageCodes;
            format!("{language} {variant}")
        })
    }).collect()
});

pub(crate) fn check_voice(voice: &str) -> bool {
    VOICES.iter().any(|s| s.as_str() == voice)
}

pub(crate) fn get_voices() -> Vec<String> {
    VOICES.iter().cloned().collect()
}
