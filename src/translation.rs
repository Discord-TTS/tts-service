use std::marker::PhantomData;

use anyhow::Result;
use small_fixed_array::FixedString;

fn deserialize_single_seq<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    struct SingleVisitor<T>(PhantomData<T>);

    impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for SingleVisitor<T> {
        type Value = Option<T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a sequence")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            seq.next_element()
        }
    }

    deserializer.deserialize_seq(SingleVisitor(PhantomData))
}

#[derive(serde::Serialize)]
struct TranslateRequest<'a> {
    text: &'a str,
    target_lang: &'a str,
    preserve_formatting: u8,
}

#[derive(serde::Deserialize)]
struct Translation {
    pub text: FixedString,
    pub detected_source_language: FixedString<u8>,
}

#[derive(serde::Deserialize)]
struct TranslateResponse {
    #[serde(deserialize_with = "deserialize_single_seq")]
    pub translations: Option<Translation>,
}

pub async fn run(
    reqwest: &reqwest::Client,
    translation_token: &str,
    content: &str,
    target_lang: &str,
) -> Result<Option<FixedString>> {
    let request = TranslateRequest {
        target_lang,
        text: content,
        preserve_formatting: 1,
    };

    let auth_header = format!("DeepL-Auth-Key {translation_token}");
    let response: TranslateResponse = reqwest
        .get("https://api.deepl.com/v2/translate")
        .query(&request)
        .header("Authorization", auth_header)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if let Some(translation) = response.translations {
        if translation.detected_source_language != target_lang {
            return Ok(Some(translation.text));
        }
    }

    Ok(None)
}
