use itertools::Itertools;
use once_cell::sync::Lazy;
use reqwest::header::HeaderValue;

use crate::Result;

pub struct State {
    reqwest: reqwest::Client,
}

impl State {
    pub fn new(reqwest: reqwest::Client) -> Self {
        Self {reqwest}
    }
}


#[derive(serde::Deserialize)]
struct TTResponse<'a> {
    #[serde(borrow)]
    data: Option<TTResponseData<'a>>,

    status_msg: &'a str,
    status_code: u16,
}

#[derive(serde::Deserialize)]
struct TTResponseData<'a> {
    v_str: &'a str
}

static BASE_URL: Lazy<reqwest::Url> = Lazy::new(|| {
    reqwest::Url::parse("https://api16-normal-useast5.us.tiktokv.com/media/api/text/speech/invoke/?speaker_map_type=0").unwrap()
});

fn parse_url(text: &str, voice: &str) -> reqwest::Url {
    let mut url = BASE_URL.clone();
    url.query_pairs_mut()
        .append_pair("text_speaker", voice)
        .append_pair("req_text", text)
        .finish();
    url
}

pub async fn get_tts(state: &State, text: &str, voice: &str) -> Result<(bytes::Bytes, Option<HeaderValue>)> {
    let mut audio = Vec::new();
    let chunks: Vec<String> = text.chars().chunks(200).into_iter().map(String::from_iter).collect();
    
    for chunk in chunks {
        let url = parse_url(&chunk, voice);
        let resp_raw = state.reqwest.post(url)
            .send().await?.error_for_status()?
            .bytes().await?;

        let resp_json: TTResponse<'_> = serde_json::from_slice(&resp_raw)?;
        let status_code = resp_json.status_code;

        if let Some(resp_data) = resp_json.data {
            if status_code == 0 {
                let resp_audio = base64::decode(resp_data.v_str)?;
                audio.extend(&resp_audio);
                continue;
            }
        }

        anyhow::bail!("TikTok Status Code {status_code}: {}", resp_json.status_msg)
    }

    Ok((
        audio.into(),
        Some(HeaderValue::from_static("audio/mpeg"))
    ))
}

pub fn check_voice(voice: &str) -> bool {
    get_voices().iter().any(|s| s.as_str() == voice)
}


pub fn get_voices() -> Vec<String> {
    get_raw_voices().into_iter().map(|(k, _)| k).collect()
}

pub fn get_raw_voices() -> std::collections::BTreeMap<String, String> {
    serde_json::from_str(include_str!("data/voices-tiktok.json")).unwrap()
}
