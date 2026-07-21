//! CCCX-only secure configuration handoff.
//!
//! The slash-command parser never receives a secret value. The value entered
//! in the masked `request_user_input` overlay is consumed locally by the TUI
//! before an app-server request, session log, or transcript persistence path.

use codex_app_server_protocol::ToolRequestUserInputResponse;

pub(crate) const CCCX_API_KEY_QUESTION_ID: &str = "cccx_api_key";
pub(crate) const CCCX_API_KEY_TURN_ID: &str = "cccx-secure-config:CCCX_API_KEY";

pub(crate) fn is_secure_config_turn(turn_id: &str) -> bool {
    turn_id == CCCX_API_KEY_TURN_ID
}

pub(crate) fn api_key_from_secure_response(
    response: &ToolRequestUserInputResponse,
) -> Result<String, &'static str> {
    let answer = response
        .answers
        .get(CCCX_API_KEY_QUESTION_ID)
        .ok_or("No API key was entered.")?;
    let [entry] = answer.answers.as_slice() else {
        return Err("Enter exactly one non-empty API key.");
    };
    let value = entry
        .strip_prefix("user_note: ")
        .ok_or("The secure input response was invalid.")?
        .trim();
    if value.is_empty() {
        return Err("The API key cannot be empty.");
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use codex_app_server_protocol::ToolRequestUserInputAnswer;

    use super::*;

    #[test]
    fn secure_input_extracts_only_one_nonempty_masked_answer() {
        let response = ToolRequestUserInputResponse {
            answers: HashMap::from([(
                CCCX_API_KEY_QUESTION_ID.to_string(),
                ToolRequestUserInputAnswer {
                    answers: vec!["user_note: router-secret".to_string()],
                },
            )]),
        };

        assert_eq!(
            api_key_from_secure_response(&response),
            Ok("router-secret".to_string())
        );
    }

    #[test]
    fn secure_input_rejects_missing_or_malformed_answers() {
        assert!(api_key_from_secure_response(&ToolRequestUserInputResponse {
            answers: HashMap::new(),
        })
        .is_err());
        let response = ToolRequestUserInputResponse {
            answers: HashMap::from([(
                CCCX_API_KEY_QUESTION_ID.to_string(),
                ToolRequestUserInputAnswer {
                    answers: vec!["not-a-secret-note".to_string()],
                },
            )]),
        };
        assert!(api_key_from_secure_response(&response).is_err());
    }
}
