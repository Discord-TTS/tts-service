use anyhow::Result;
use small_fixed_array::FixedString;

pub async fn run(
    reqwest: &reqwest::Client,
    translation_url: &str,
    translation_token: &str,
    content: &str,
    target_lang: &str,
) -> Result<Option<FixedString>> {
    #[derive(serde::Deserialize)]
    pub struct DeeplTranslateResponse {
        pub translations: Vec<DeeplTranslation>,
    }

    #[derive(serde::Deserialize)]
    pub struct DeeplTranslation {
        pub text: FixedString,
        pub detected_source_language: FixedString<u8>,
    }

    #[derive(serde::Serialize)]
    struct DeeplTranslateRequest<'a> {
        text: &'a str,
        target_lang: &'a str,
        preserve_formatting: u8,
    }

    let request = DeeplTranslateRequest {
        target_lang,
        text: content,
        preserve_formatting: 1,
    };

    let response: DeeplTranslateResponse = reqwest
        .get(format!("{translation_url}/translate"))
        .query(&request)
        .header(
            "Authorization",
            format!("DeepL-Auth-Key {translation_token}"),
        )
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(translation) = response.translations.into_iter().next() {
        if translation.detected_source_language != target_lang {
            return Ok(Some(translation.text));
        }
    }

    Ok(None)
}
