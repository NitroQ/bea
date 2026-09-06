//! OpenAI / ChatGPT (Codex) OAuth 2.0 + PKCE login and Responses API transport.
//!
//! Constants describe the public Codex-CLI OAuth client. They are isolated
//! here on purpose: if OpenAI changes any of them, this is the only file to
//! update (verify against a current `@openai/codex` release).
use crate::LlmRequest;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const AUTH_ISSUER: &str = "https://auth.openai.com";
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// Same URI, percent-encoded for the authorize query string.
pub const REDIRECT_URI_ENCODED: &str =
    "http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback";
pub const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";

/// OAuth tokens for the ChatGPT (Codex) subscription flow. Persisted as JSON
/// in the OS credential store — never in SQLite.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix epoch seconds; compare with a 60s refresh skew.
    pub expires_at: i64,
    pub account_id: String,
}

/// RFC 7636 code verifier: 64 chars from the unreserved set. UUIDs filtered
/// to alphanumeric + hyphen give enough entropy and stay URL-safe.
pub fn pkce_verifier() -> String {
    let raw = format!(
        "{}{}{}{}",
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4()
    );
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(64)
        .collect()
}

/// BASE64URL(SHA256(verifier)) with no padding, per RFC 7636 §4.2.
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub fn authorize_url(verifier: &str) -> String {
    format!(
        "{AUTH_ISSUER}/oauth/authorize?response_type=code&client_id={CLIENT_ID}&redirect_uri={REDIRECT_URI_ENCODED}&scope=openid%20profile%20email&code_challenge={}&code_challenge_method=S256",
        pkce_challenge(verifier)
    )
}

pub fn token_exchange_body(code: &str, verifier: &str) -> String {
    format!("grant_type=authorization_code&code={code}&redirect_uri={REDIRECT_URI}&client_id={CLIENT_ID}&code_verifier={verifier}")
}

pub fn token_refresh_body(refresh_token: &str) -> String {
    format!("grant_type=refresh_token&refresh_token={refresh_token}&client_id={CLIENT_ID}")
}

/// The Codex backend speaks the Responses API, not chat/completions.
/// `instructions` carries the system prompt; input is a plain user turn.
/// When `json_schema` is set (the minutes path) it rides in
/// `text.format.json_schema` so structured output still works.
pub fn responses_payload(request: &LlmRequest) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": request.model,
        "instructions": request.system,
        "input": [
            {"role": "user", "content": [{"type": "input_text", "text": request.user}]}
        ],
        "max_output_tokens": request.max_output_tokens,
    });
    if !request.json_schema.trim().is_empty() {
        if let Ok(schema) = serde_json::from_str::<serde_json::Value>(&request.json_schema) {
            body["text"] = serde_json::json!({
                "format": {
                    "type": "json_schema",
                    "name": "bea_minutes",
                    "schema": schema
                }
            });
        }
    }
    body
}

/// Extracts the assistant message text from a Responses API payload.
/// Returns None when no message output exists (e.g. empty output array).
pub fn responses_output_text(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("output")?
        .as_array()?
        .iter()
        .filter(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("message"))
        .filter_map(|item| item.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
        .next()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_url_safe_and_challenge_matches_rfc7636() {
        let verifier = pkce_verifier();
        assert_eq!(verifier.len(), 64);
        assert!(verifier.chars().all(|c| c.is_ascii_alphanumeric()
            || c == '-'
            || c == '_'
            || c == '.'
            || c == '~'));
        // RFC 7636 Appendix B worked example.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_carries_client_redirect_scope_and_challenge() {
        let url = authorize_url("my-verifier");
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=openid"));
    }

    #[test]
    fn token_exchange_and_refresh_bodies_are_form_encoded_pkce() {
        let body = token_exchange_body("auth-code-1", "my-verifier");
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code=auth-code-1"));
        assert!(body.contains("code_verifier=my-verifier"));
        assert!(body.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        let refresh = token_refresh_body("refresh-token-1");
        assert!(refresh.contains("grant_type=refresh_token"));
        assert!(refresh.contains("refresh_token=refresh-token-1"));
    }

    #[test]
    fn codex_payload_translates_llm_request_to_responses_api() {
        let body = responses_payload(&LlmRequest {
            model: "gpt-5.1-codex".into(),
            system: "sys".into(),
            user: "usr".into(),
            json_schema: String::new(),
            max_output_tokens: 500,
        });
        assert_eq!(body["model"], "gpt-5.1-codex");
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["max_output_tokens"], 500);
        assert!(body["input"].as_array().unwrap().iter().any(|item| {
            item["role"] == "user"
                && item["content"][0]["type"] == "input_text"
                && item["content"][0]["text"] == "usr"
        }));
        assert!(
            body.get("text").is_none(),
            "no schema means no text.format block"
        );
    }

    #[test]
    fn codex_payload_rides_the_minutes_json_schema_in_text_format() {
        let body = responses_payload(&LlmRequest {
            model: "gpt-5.1-codex".into(),
            system: "sys".into(),
            user: "usr".into(),
            json_schema: r#"{"type":"object","properties":{"title":{"type":"string"}}}"#.into(),
            max_output_tokens: 500,
        });
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["name"], "bea_minutes");
        assert_eq!(body["text"]["format"]["schema"]["type"], "object");
    }

    #[test]
    fn responses_output_text_is_extracted_from_structured_items() {
        let payload = serde_json::json!({
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "{\"title\":\"T\"}"}]}
            ]
        });
        assert_eq!(
            responses_output_text(&payload).unwrap(),
            "{\"title\":\"T\"}"
        );
        assert!(responses_output_text(&serde_json::json!({"output": []})).is_none());
    }
}
