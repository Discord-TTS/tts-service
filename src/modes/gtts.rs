use std::{
    fmt::Display,
    sync::{Arc, OnceLock, atomic::AtomicBool},
    time::Duration,
};

use aformat::ToArrayString;
use async_trait::async_trait;
use axum::{
    http::HeaderValue,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use ipgen::IpNetwork;
use itertools::Itertools;
use rand::RngExt as _;
use serde_json::to_value;
use tokio::sync::RwLock;

use crate::{DeadlineMonitor, Result, TTSEngine, TTSParams, TTSVoices};

#[derive(Clone)]
pub struct State {
    ip_client: Arc<RwLock<IpClient>>,
    ip_block: Option<IpNetwork>,
    hit_any_deadline: Arc<AtomicBool>,
}

struct IpClient {
    client: reqwest::Client,
    ip: std::net::IpAddr,
}
impl IpClient {
    fn new_ip(&mut self, ip: std::net::IpAddr) -> Result<()> {
        self.client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .local_address(Some(ip))
            .build()?;
        self.ip = ip;
        Ok(())
    }
    async fn get(&self, text: &str, lang: &str) -> Result<CheckResult> {
        let mut url = get_base_url();
        url.query_pairs_mut()
            .append_pair("tl", lang)
            .append_pair("q", text)
            .append_pair("textlen", &text.len().to_arraystring())
            .finish();
        is_block(self.client.get(url).send().await).await
    }

    pub async fn get_random_ipv6(&mut self, ip_block: Option<IpNetwork>) -> Result<()> {
        let Some(ip_block) = ip_block else {
            self.new_ip("0.0.0.0".parse()?)?;
            return Ok(());
        };

        let mut attempts = 1;
        loop {
            let name: String = rand::rng()
                .sample_iter::<char, _>(rand::distr::StandardUniform)
                .take(16)
                .collect();

            tracing::debug!("Generated random name: {:?}", name.as_bytes());
            let ip = ipgen::ip(&name, ip_block).unwrap();

            self.new_ip(ip);

            let check_result = self.get("Hello", "en").await?;
            if let CheckResult::Ok(..) = check_result {
                tracing::warn!("Generated random IP: {ip}");
                break;
            }
            tracing::warn!(
                "Failed to generate a new IP on attempt {attempts} with a {check_result}"
            );

            attempts += 1;
        }
        Ok(())
    }
}

#[async_trait]
impl TTSEngine for State {
    async fn check_params(&self, params: TTSParams<'_>) -> Option<Response> {
        if params.speaking_rate < 0. {
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
        use bytes::Buf;
        mp3_duration::from_read(&mut audio.reader()).map_or(true, |d| d.as_secs() < max_length)
    }
    async fn speak(
        &self,
        text: &str,
        params: TTSParams<'_>,
        preferred_format: Option<&str>,
    ) -> Result<(Bytes, Option<HeaderValue>)> {
        let _guard = DeadlineMonitor::new(
            Duration::from_secs(3),
            self.hit_any_deadline.clone(),
            |took| {
                tracing::warn!("Fetching gTTS audio took {} millis!", took.as_millis());
            },
        );

        let mut content_type = None;
        let mut audio = Vec::new();

        let mut client = self.ip_client.read().await;
        let chunks: Vec<String> = text
            .chars()
            .chunks(200)
            .into_iter()
            .map(Iterator::collect)
            .collect();
        for chunk in chunks {
            loop {
                let result = client.get(&chunk, params.voice).await?;

                if let CheckResult::Ok(content_type_, audio_chunk) = result {
                    if let Some(content_type_) = content_type_ {
                        content_type = Some(content_type_);
                    }

                    break audio.extend(audio_chunk);
                }
                {
                    drop(client);
                    let mut rw_client = self.ip_client.write().await;
                    tracing::warn!("IP {} has been blocked!", rw_client.ip);
                    rw_client.get_random_ipv6(self.ip_block).await?;
                    client = self.ip_client.read().await;
                }
            }
        }

        Ok((bytes::Bytes::from(audio), content_type))
    }
    async fn get_voices(&self) -> Result<TTSVoices> {
        let raw: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("data/voices-gtts.json"))?;
        Ok(TTSVoices {
            raw: to_value(raw.clone())?,
            nice: raw.into_keys().collect(),
        })
    }
    fn get_defaults(&self) -> TTSParams<'static> {
        todo!()
    }
}

fn get_base_url() -> reqwest::Url {
    static BASE_URL: OnceLock<reqwest::Url> = OnceLock::new();
    BASE_URL
        .get_or_init(|| {
            reqwest::Url::parse(
                "https://translate.google.com/translate_tts?ie=UTF-8&total=1&idx=0&client=tw-ob",
            )
            .unwrap()
        })
        .clone()
}

enum CheckResult {
    Ok(Option<reqwest::header::HeaderValue>, bytes::Bytes),
    NormalBlock,
    TimeoutBlock,
    HostUnreachable,
}
impl Display for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckResult::NormalBlock => write!(f, "429 block"),
            CheckResult::TimeoutBlock => write!(f, "timeout block"),
            CheckResult::HostUnreachable => write!(f, "unreachable error"),
            CheckResult::Ok(Some(header_value), bytes) => {
                write!(
                    f,
                    "{} bytes of {}",
                    bytes.len(),
                    header_value.to_str().unwrap_or("unknown")
                )
            }
            CheckResult::Ok(None, bytes) => {
                write!(f, "{} bytes of unknown", bytes.len())
            }
        }
    }
}

fn is_host_unreachable(err: &reqwest::Error) -> bool {
    let debug_message = format!("{err:?}");
    ["No route to host", "HostUnreachable"]
        .into_iter()
        .all(|s| debug_message.contains(s))
}

async fn is_block(resp: reqwest::Result<reqwest::Response>) -> Result<CheckResult> {
    match resp {
        Ok(mut resp) => {
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                Ok(CheckResult::NormalBlock)
            } else {
                let content_type = resp.headers_mut().remove(reqwest::header::CONTENT_TYPE);
                let audio = resp.error_for_status()?.bytes().await?;

                Ok(CheckResult::Ok(content_type, audio))
            }
        }
        Err(err) => {
            if err.is_timeout() {
                Ok(CheckResult::TimeoutBlock)
            } else if is_host_unreachable(&err) {
                Ok(CheckResult::HostUnreachable)
            } else {
                Err(err.into())
            }
        }
    }
}
