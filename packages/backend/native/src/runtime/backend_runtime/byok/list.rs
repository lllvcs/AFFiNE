use std::collections::HashMap;

use llm_adapter::target::EgressPolicy;

use super::{RuntimeError, RuntimeResult};
use crate::llm::byok::{ByokEndpoint, ByokPolicy};

const MODELS_TIMEOUT_MS: u32 = 15_000;
const MODELS_MAX_BYTES: u32 = 2 * 1024 * 1024;
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// `safefetch` drops every header that is not listed here (or prefixed with
/// `accept`/`sec-`), so credential headers must be named explicitly.
const ALLOWED_REQUEST_HEADERS: [&str; 3] = ["authorization", "x-api-key", "x-goog-api-key"];

struct ModelsRequest {
  url: String,
  headers: HashMap<String, String>,
}

/// Lists the model ids the provider behind `endpoint` exposes to `credential`.
///
/// The URL layout mirrors how chat requests are built for the same provider:
/// OpenAI-compatible bases already carry their version prefix (`/v1`), while
/// Anthropic bases do not and get `/v1/models` appended.
pub(in super::super) async fn list_provider_models(
  provider: &str,
  endpoint: &ByokEndpoint,
  credential: String,
  policy: &ByokPolicy,
) -> RuntimeResult<Vec<crate::llm::ByokProviderModelOutput>> {
  policy.admit(provider, endpoint).await?;
  let request = models_request(provider, endpoint, &credential)?;
  let allow_private = policy.egress_policy(endpoint) == EgressPolicy::AllowPrivate;
  let body = tokio::task::spawn_blocking(move || fetch_models(request, allow_private))
    .await
    .map_err(|error| RuntimeError::invalid_state(format!("BYOK model listing task failed: {error}")))??;
  parse_models(provider, &body)
}

fn models_request(provider: &str, endpoint: &ByokEndpoint, credential: &str) -> RuntimeResult<ModelsRequest> {
  let base = match endpoint {
    ByokEndpoint::ProviderDefault => default_models_endpoint(provider)?,
    ByokEndpoint::OpenAiCompatible { url, .. } | ByokEndpoint::AnthropicCompatible { url } => url.as_str(),
  }
  .trim_end_matches('/');
  match provider {
    "openai" => Ok(ModelsRequest {
      url: format!("{base}/models"),
      headers: HashMap::from([("authorization".to_string(), format!("Bearer {credential}"))]),
    }),
    "anthropic" => Ok(ModelsRequest {
      url: format!("{base}/v1/models"),
      headers: HashMap::from([
        ("x-api-key".to_string(), credential.to_string()),
        ("anthropic-version".to_string(), ANTHROPIC_VERSION.to_string()),
      ]),
    }),
    "gemini" => Ok(ModelsRequest {
      url: format!("{base}/models"),
      headers: HashMap::from([("x-goog-api-key".to_string(), credential.to_string())]),
    }),
    _ => Err(RuntimeError::invalid_input("provider does not expose a model list")),
  }
}

fn default_models_endpoint(provider: &str) -> RuntimeResult<&'static str> {
  match provider {
    "openai" => Ok("https://api.openai.com/v1"),
    "anthropic" => Ok("https://api.anthropic.com"),
    "gemini" => Ok("https://generativelanguage.googleapis.com/v1beta"),
    _ => Err(RuntimeError::invalid_input("provider does not expose a model list")),
  }
}

fn fetch_models(request: ModelsRequest, allow_private: bool) -> RuntimeResult<Vec<u8>> {
  let response = safefetch::safe_fetch(&safefetch::SafeFetchRequest {
    url: request.url,
    method: Some(safefetch::SafeFetchMethod::Get),
    headers: Some(request.headers),
    body: None,
    timeout_ms: Some(MODELS_TIMEOUT_MS),
    max_redirects: Some(3),
    max_bytes: Some(MODELS_MAX_BYTES),
    allowed_headers: Some(
      ALLOWED_REQUEST_HEADERS
        .iter()
        .map(|header| (*header).to_string())
        .collect(),
    ),
    allowed_hosts: None,
    allow_http: Some(allow_private),
    allow_private_target_origin: Some(allow_private),
    ech_config_list: None,
  })
  .map_err(|error| RuntimeError::invalid_input(format!("BYOK model listing request failed: {error}")))?;
  if response.status != 200 {
    return Err(RuntimeError::invalid_input(format!(
      "BYOK model listing was rejected with status {}",
      response.status
    )));
  }
  Ok(response.body)
}

fn parse_models(provider: &str, body: &[u8]) -> RuntimeResult<Vec<crate::llm::ByokProviderModelOutput>> {
  let value: serde_json::Value = serde_json::from_slice(body)
    .map_err(|_| RuntimeError::invalid_input("BYOK model listing returned invalid JSON"))?;
  let (list_key, id_prefix) = match provider {
    "gemini" => ("models", Some("models/")),
    _ => ("data", None),
  };
  let entries = value
    .get(list_key)
    .and_then(serde_json::Value::as_array)
    .ok_or_else(|| RuntimeError::invalid_input("BYOK model listing response has no model list"))?;

  let mut seen = std::collections::HashSet::new();
  let mut models = Vec::new();
  for entry in entries {
    let Some(raw_id) = entry
      .get("id")
      .and_then(serde_json::Value::as_str)
      .or_else(|| entry.get("name").and_then(serde_json::Value::as_str))
    else {
      continue;
    };
    let model_id = id_prefix
      .and_then(|prefix| raw_id.strip_prefix(prefix))
      .unwrap_or(raw_id)
      .trim();
    if model_id.is_empty() || model_id.len() > 512 || !seen.insert(model_id.to_string()) {
      continue;
    }
    let display_name = entry
      .get("display_name")
      .or_else(|| entry.get("displayName"))
      .and_then(serde_json::Value::as_str)
      .map(str::to_string);
    models.push(crate::llm::ByokProviderModelOutput {
      model_id: model_id.to_string(),
      display_name,
    });
  }
  if models.is_empty() {
    return Err(RuntimeError::invalid_input(
      "BYOK model listing returned no usable models",
    ));
  }
  Ok(models)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn builds_provider_specific_model_endpoints() {
    let openai = models_request(
      "openai",
      &ByokEndpoint::OpenAiCompatible {
        url: "https://gateway.example.com/v1/".to_string(),
        dialect: llm_adapter::target::OpenAiDialect::ChatCompletions,
      },
      "sk-test",
    )
    .unwrap();
    assert_eq!(openai.url, "https://gateway.example.com/v1/models");
    assert_eq!(
      openai.headers.get("authorization").map(String::as_str),
      Some("Bearer sk-test")
    );

    let anthropic = models_request(
      "anthropic",
      &ByokEndpoint::AnthropicCompatible {
        url: "https://gateway.example.com".to_string(),
      },
      "sk-ant",
    )
    .unwrap();
    assert_eq!(anthropic.url, "https://gateway.example.com/v1/models");
    assert_eq!(anthropic.headers.get("x-api-key").map(String::as_str), Some("sk-ant"));
    assert_eq!(
      anthropic.headers.get("anthropic-version").map(String::as_str),
      Some(ANTHROPIC_VERSION)
    );

    assert!(models_request("fal", &ByokEndpoint::ProviderDefault, "key").is_err());
  }

  #[test]
  fn parses_each_provider_envelope() {
    let openai = parse_models("openai", br#"{"data":[{"id":"gpt-x"},{"id":"gpt-x"},{"id":"gpt-y"}]}"#).unwrap();
    assert_eq!(
      openai.iter().map(|model| model.model_id.as_str()).collect::<Vec<_>>(),
      ["gpt-x", "gpt-y"]
    );

    let anthropic = parse_models(
      "anthropic",
      br#"{"data":[{"id":"claude-x","display_name":"Claude X"}]}"#,
    )
    .unwrap();
    assert_eq!(anthropic[0].model_id, "claude-x");
    assert_eq!(anthropic[0].display_name.as_deref(), Some("Claude X"));

    let gemini = parse_models(
      "gemini",
      br#"{"models":[{"name":"models/gemini-x","displayName":"Gemini X"}]}"#,
    )
    .unwrap();
    assert_eq!(gemini[0].model_id, "gemini-x");
    assert_eq!(gemini[0].display_name.as_deref(), Some("Gemini X"));

    assert!(parse_models("openai", br#"{"data":[]}"#).is_err());
    assert!(parse_models("openai", b"not json").is_err());
  }
}
