use browserkit_types::{FrontendMessage, ProtocolError};
use serde::Deserialize;

#[derive(Deserialize)]
struct MessageType {
    #[serde(rename = "type")]
    kind: String,
}

pub(crate) fn parse_message(raw: &str) -> Result<FrontendMessage, ProtocolError> {
    match serde_json::from_str(raw) {
        Ok(message) => Ok(message),
        Err(error) => {
            let code = match serde_json::from_str::<MessageType>(raw) {
                Ok(message) if message.kind != "command" && message.kind != "request" => {
                    "unsupported_message"
                }
                _ => "invalid_message",
            };
            Err(ProtocolError {
                code: code.into(),
                message: error.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_message_is_rejected() {
        assert_eq!(
            parse_message("not json").unwrap_err().code,
            "invalid_message"
        );
    }

    #[test]
    fn unknown_message_type_is_unsupported() {
        assert_eq!(
            parse_message(r#"{"type":"future"}"#).unwrap_err().code,
            "unsupported_message"
        );
    }
}
