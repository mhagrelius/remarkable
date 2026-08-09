//! What goes to llama-server and what comes back.
//!
//! The OpenAI-compatible shape, the same one familiar speaks, so a section of a
//! page is a `user` message holding the instruction and one `image_url` part
//! carrying a `data:` URL. llama-server decodes that and hands the pixels to
//! the projector; there is no upload endpoint and a `data:application/pdf` URL
//! is rejected, which is why a PDF is rasterised before it gets here.
//!
//! Not streamed. A transcription is wanted whole or not at all — there is no
//! use for half a section — and the unit of progress the window reports is the
//! section, not the token. That trades a little perceived latency for no SSE
//! parser and no partial-response state to unwind when a section fails.

use serde::{Deserialize, Serialize};

/// One request: transcribe this section.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatRequest {
    /// llama-server serves whatever it was launched with and ignores this. A
    /// gateway in front of it will not, so it is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub messages: Vec<Message>,
    pub stream: bool,
    /// Low, but not zero. Transcription wants the likeliest reading of a word;
    /// zero makes the model repeat itself when the handwriting gives it
    /// nothing to go on, filling a page with the same bullet.
    pub temperature: f32,
    /// A section of handwriting is at most a few hundred words. This is headroom,
    /// not a target — its job is to stop a repetition loop from running to the
    /// end of the context.
    pub max_tokens: u32,
    /// Arguments for the server's Jinja chat template.
    ///
    /// This one field is why the first eval run scored zero. The llama-server
    /// this talks to is launched with `enable_thinking` on, which is right for
    /// a chat client and wrong here: asked to transcribe a page, the model
    /// reasons about every line first, and on a full section it spent all 4,096
    /// tokens in `reasoning_content` and returned empty `content`. There is
    /// nothing to think about — the answer is what the page says — so thinking
    /// is turned off per request rather than by relaunching the server, which
    /// familiar and everything else on this machine still want it for.
    ///
    /// `reasoning_budget: 0` is the other documented lever and does not work
    /// here: the template emits the thinking block regardless, and the budget
    /// only caps it.
    pub chat_template_kwargs: TemplateArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TemplateArgs {
    pub enable_thinking: bool,
}

impl ChatRequest {
    pub fn transcribe(model: Option<String>, prompt: &str, image: &DataUrl) -> Self {
        Self {
            model,
            messages: vec![Message {
                role: Role::User,
                content: vec![
                    // Text first: the model should know what is being asked
                    // before it looks at the picture.
                    Part::Text {
                        text: prompt.to_string(),
                    },
                    Part::ImageUrl {
                        image_url: ImageUrl {
                            url: image.0.clone(),
                        },
                    },
                ],
            }],
            stream: false,
            temperature: 0.2,
            max_tokens: 4096,
            chat_template_kwargs: TemplateArgs {
                enable_thinking: false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageUrl {
    pub url: String,
}

/// A `data:` URL, built once and moved rather than copied — a section of a page is
/// about a megabyte of base64 and the request holds the only copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataUrl(pub String);

impl DataUrl {
    /// Wrap PNG bytes as the URL llama-server decodes.
    pub fn png(bytes: &[u8]) -> Self {
        Self(format!("data:image/png;base64,{}", base64(bytes)))
    }
}

/// What llama-server says about itself, from `/props`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerInfo {
    /// The alias it was launched with, for the window to name.
    pub model: Option<String>,
    /// Whether the loaded model has a projector. Without one it cannot see the
    /// page, and that is worth saying before a transcription rather than after
    /// it comes back as an apology.
    pub vision: bool,
}

/// Read `/props`. Everything is optional: a gateway may answer a shape of its
/// own, and a missing field is a thing not shown rather than a failure.
pub fn parse_props(body: &str) -> ServerInfo {
    let Ok(props) = serde_json::from_str::<serde_json::Value>(body) else {
        return ServerInfo::default();
    };

    let model = props
        .get("model_alias")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            props
                .get("model_path")
                .and_then(serde_json::Value::as_str)
                .and_then(|path| path.rsplit(['/', '\\']).next())
                .map(|file| file.trim_end_matches(".gguf").to_string())
        });

    let vision = props
        .get("modalities")
        .and_then(|modalities| modalities.get("vision"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    ServerInfo { model, vision }
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<ResponseMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

/// A transcription, and whether the model was cut off producing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    /// `true` when generation stopped at `max_tokens` rather than because the
    /// model was finished. The section's tail is missing, and the merge is told so
    /// it does not treat a truncated line as a seam.
    pub truncated: bool,
}

/// Read a completion out of a response body.
///
/// `Err` carries what the body actually was, because the useful diagnostic for
/// a server answering the wrong shape is the shape it answered.
pub fn parse_completion(body: &str) -> Result<Completion, String> {
    let response: ChatResponse = serde_json::from_str(body)
        .map_err(|error| format!("the server's answer was not a completion: {error}"))?;

    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| "the server returned no completion".to_string())?;

    let text = choice
        .message
        .and_then(|message| message.content)
        .ok_or_else(|| "the completion had no content".to_string())?;

    Ok(Completion {
        truncated: choice.finish_reason.as_deref() == Some("length"),
        text,
    })
}

/// Base64, written out rather than taken as a dependency, as in familiar's
/// attachments. Forty lines of table lookup is not a crate.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let block = match chunk.len() {
            3 => u32::from(chunk[0]) << 16 | u32::from(chunk[1]) << 8 | u32::from(chunk[2]),
            2 => u32::from(chunk[0]) << 16 | u32::from(chunk[1]) << 8,
            _ => u32::from(chunk[0]) << 16,
        };
        for shift in 0..4 {
            if shift <= chunk.len() {
                out.push(ALPHABET[((block >> (18 - shift * 6)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_definition() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_data_url_says_what_the_bytes_are() {
        let url = DataUrl::png(b"\x89PNG\r\n\x1a\n");
        assert!(url.0.starts_with("data:image/png;base64,"), "{}", url.0);
    }

    #[test]
    fn a_request_puts_the_question_before_the_picture() {
        let request = ChatRequest::transcribe(
            Some("qwen3.6-27b".into()),
            "transcribe this",
            &DataUrl::png(b"bytes"),
        );
        let json = serde_json::to_value(&request).expect("serialises");
        let parts = &json["messages"][0]["content"];
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "transcribe this");
        assert_eq!(parts[1]["type"], "image_url");
        assert!(parts[1]["image_url"]["url"]
            .as_str()
            .expect("a url")
            .starts_with("data:image/png;base64,"));
        assert_eq!(json["stream"], false);
    }

    #[test]
    fn a_request_turns_the_servers_thinking_off() {
        // The server is launched with thinking on for its chat clients. Left
        // on, a full section comes back with an empty `content` and 4,096 tokens
        // of reasoning — which is what the first eval run scored zero on.
        let request = ChatRequest::transcribe(None, "x", &DataUrl::png(b"y"));
        let json = serde_json::to_value(&request).expect("serialises");
        assert_eq!(json["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn a_request_without_a_model_omits_the_field_rather_than_sending_null() {
        let request = ChatRequest::transcribe(None, "x", &DataUrl::png(b"y"));
        let json = serde_json::to_value(&request).expect("serialises");
        assert!(json.get("model").is_none());
    }

    #[test]
    fn a_completion_is_read_out_of_the_answer() {
        let body = r##"{"choices":[{"message":{"role":"assistant","content":"# Page\n\ntext"},
                       "finish_reason":"stop"}]}"##;
        let completion = parse_completion(body).expect("a completion");
        assert_eq!(completion.text, "# Page\n\ntext");
        assert!(!completion.truncated);
    }

    #[test]
    fn hitting_the_token_ceiling_is_reported_not_hidden() {
        let body = r#"{"choices":[{"message":{"content":"half a "},"finish_reason":"length"}]}"#;
        assert!(parse_completion(body).expect("a completion").truncated);
    }

    #[test]
    fn a_server_answering_the_wrong_shape_says_so() {
        assert!(parse_completion("not json").is_err());
        assert!(parse_completion(r#"{"choices":[]}"#).is_err());
        assert!(parse_completion(r#"{"choices":[{"finish_reason":"stop"}]}"#).is_err());
    }

    #[test]
    fn props_report_the_model_and_whether_it_can_see() {
        let info = parse_props(
            r#"{"model_alias":"qwen3.6-27b","model_path":"/m/Qwen3.6-27B.gguf",
                "modalities":{"vision":true,"audio":false}}"#,
        );
        assert_eq!(info.model.as_deref(), Some("qwen3.6-27b"));
        assert!(info.vision);
    }

    #[test]
    fn a_text_only_model_is_not_reported_as_able_to_see() {
        let info = parse_props(r#"{"model_alias":"qwen3.6-27b","modalities":{"vision":false}}"#);
        assert!(!info.vision);
        // A server that does not mention modalities at all is assumed blind,
        // rather than sending it a page it will refuse.
        assert!(!parse_props(r#"{"model_alias":"x"}"#).vision);
    }

    #[test]
    fn a_filename_stands_in_for_a_missing_alias() {
        let info = parse_props(r#"{"model_path":"/srv/models/Qwen3.6-27B-UD-Q5_K_XL.gguf"}"#);
        assert_eq!(info.model.as_deref(), Some("Qwen3.6-27B-UD-Q5_K_XL"));
    }

    #[test]
    fn a_server_that_answers_something_else_is_not_a_failure() {
        assert_eq!(parse_props("not json"), ServerInfo::default());
        assert_eq!(parse_props("{}"), ServerInfo::default());
    }
}
