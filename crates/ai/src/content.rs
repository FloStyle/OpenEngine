//! Chat message content: plain text and multimodal parts.
//!
//! [`ChatTurn`] carries ordered [`ContentPart`]s. It serializes to the OpenAI
//! `chat/completions` message shape: a plain `content` string when the turn is
//! text-only (so it stays compatible with every text server / older code), and
//! an OpenAI-vision `content` **array** when an image is present:
//!
//! ```json
//! {"role":"user","content":[
//!   {"type":"text","text":"what is this?"},
//!   {"type":"image_url","image_url":{"url":"data:image/png;base64,...."}}
//! ]}
//! ```

use serde::{Serialize, Serializer};

/// A single piece of message content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentPart {
    /// Plain text.
    Text(String),
    /// A base64-encoded image (data URL on the wire).
    Image { mime: String, base64: String },
}

/// Convenience for a text+image turn (system/user role with a caption + one
/// picture), kept separate from the raw parts vector for readability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextImageContent {
    /// Caption / question accompanying the image.
    pub text: String,
    /// Image MIME, e.g. `image/png`.
    pub mime: String,
    /// Base64-encoded image bytes.
    pub base64: String,
}

/// A single chat message sent to a model.
///
/// Built with [`ChatTurn::text`] (pure text) or [`ChatTurn::with_image`]
/// (multimodal). Text-only turns serialize `content` as a plain string; turns
/// containing an image serialize it as an OpenAI-vision array.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatTurn {
    /// `"system"` | `"user"` | `"assistant"`.
    pub role: String,
    /// Ordered message parts.
    pub parts: Vec<ContentPart>,
}

impl ChatTurn {
    /// A plain-text message.
    pub fn text(role: &str, text: &str) -> Self {
        ChatTurn {
            role: role.to_string(),
            parts: vec![ContentPart::Text(text.to_string())],
        }
    }

    /// A multimodal message: caption text plus one base64 image.
    pub fn with_image(role: &str, img: TextImageContent) -> Self {
        ChatTurn {
            role: role.to_string(),
            parts: vec![
                ContentPart::Text(img.text),
                ContentPart::Image {
                    mime: img.mime,
                    base64: img.base64,
                },
            ],
        }
    }

    /// True when the turn carries at least one image part.
    pub fn has_image(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p, ContentPart::Image { .. }))
    }

    /// The raw text of a single-part text turn, if this turn is pure text.
    pub fn content_text(&self) -> Option<String> {
        match self.parts.as_slice() {
            [ContentPart::Text(t)] => Some(t.clone()),
            _ => None,
        }
    }
}

impl Serialize for ChatTurn {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("ChatTurn", 2)?;
        st.serialize_field("role", &self.role)?;
        st.serialize_field("content", &ContentField(self))?;
        st.end()
    }
}

/// Serializes `content` to a JSON Value (string for text, array for vision).
fn serialize_content_helper(t: &ChatTurn) -> serde_json::Value {
    if !t.has_image() {
        let text = t.parts.iter().fold(String::new(), |mut acc, p| {
            if let ContentPart::Text(x) = p {
                if !acc.is_empty() {
                    acc.push('\n');
                }
                acc.push_str(x);
            }
            acc
        });
        return serde_json::Value::String(text);
    }
    let arr: Vec<serde_json::Value> = t
        .parts
        .iter()
        .map(|p| match p {
            ContentPart::Text(x) => serde_json::json!({"type":"text","text":x}),
            ContentPart::Image { mime, base64 } => {
                let url = format!("data:{mime};base64,{base64}");
                serde_json::json!({"type":"image_url","image_url":{"url":url}})
            }
        })
        .collect();
    serde_json::Value::Array(arr)
}

/// Newtype so a struct-field serializer can hand back the right JSON Value.
struct ContentField<'a>(&'a ChatTurn);

impl Serialize for ContentField<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_content_helper(self.0).serialize(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_only_turn_serializes_as_plain_content_string() {
        let t = ChatTurn::text("user", "hello");
        let v = serialize_content_helper(&t);
        assert_eq!(v, serde_json::json!("hello"));
        // Legacy shape: content is a bare string.
        let full = serde_json::json!({ "role": "user", "content": v });
        assert_eq!(full["content"], "hello");
    }

    #[test]
    fn multimodal_turn_serializes_openai_vision_array() {
        let t = ChatTurn::with_image(
            "user",
            TextImageContent {
                text: "describe".into(),
                mime: "image/png".into(),
                base64: "aGVsbG8=".into(),
            },
        );
        assert!(t.has_image());
        let v = serialize_content_helper(&t);
        let arr = v.as_array().expect("vision content is an array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0], serde_json::json!({"type":"text","text":"describe"}));
        assert_eq!(
            arr[1],
            serde_json::json!({
                "type":"image_url",
                "image_url":{"url":"data:image/png;base64,aGVsbG8="}
            })
        );
        // End-to-end message JSON embeds that array under `content`.
        let msg = serde_json::to_value(&t).expect("serialize ChatTurn");
        let expected = serde_json::json!([
            {"type":"text","text":"describe"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8="}}
        ]);
        assert_eq!(msg["content"], expected);
        assert_eq!(msg["role"], "user");
    }
}
