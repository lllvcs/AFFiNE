use llm_adapter::{
  capability::provider_default_capability_upper_bound,
  target::{BackendEndpoint, OpenAiDialect},
};
use sqlx::{FromRow, PgPool, Row};

use super::super::{LocalLeasePayload, RuntimeError, RuntimeResult, token_hash};
use crate::{
  llm::{
    CopilotAccessProjection,
    byok::{ByokEndpoint, ByokPolicy, ByokProfileDefinition, local_aad, server_aad},
    route::{self, AuthorizedProviderProfile, CatalogSlot, CredentialRef, ProfileSource},
  },
  runtime::{BackendRuntimeConfig, CopilotManagedProfileConfig, CopilotRuntimeConfig},
};

#[derive(FromRow)]
struct ServerProfileRow {
  id: String,
  workspace_id: String,
  provider: String,
  encrypted_api_key: String,
  definition: serde_json::Value,
  sort_order: i32,
  enabled: bool,
}

pub(super) struct ProfileLoadInput<'a> {
  pub(super) slot: &'a CatalogSlot,
  pub(super) built_in_route_id: Option<&'a str>,
  pub(super) workspace_id: Option<&'a str>,
  pub(super) user_id: Option<&'a str>,
  pub(super) local_lease_id: Option<&'a str>,
  pub(super) access: &'a CopilotAccessProjection,
  pub(super) managed_target_id: Option<&'a str>,
}

pub(super) async fn load_profiles(
  pool: &PgPool,
  config: &BackendRuntimeConfig,
  input: ProfileLoadInput<'_>,
) -> RuntimeResult<Vec<AuthorizedProviderProfile>> {
  let mut profiles = Vec::new();
  // Every branch below can legitimately end with nothing to route to, and the
  // route diagnostic only reports `profiles=0`. Collect the reason while it is
  // still known, or "wrong workspace", "key disabled", "lease expired" and "not
  // entitled" stay indistinguishable from the outside.
  let mut notes: Vec<String> = Vec::new();
  let policy = config.byok_policy();
  match input.workspace_id {
    None => notes.push("the request carried no workspace id".to_string()),
    Some(workspace_id) => {
      if input.access.server_byok {
        profiles.extend(load_server_profiles(pool, workspace_id, &policy, &mut notes).await?);
      } else {
        notes.push(format!(
          "workspace {workspace_id} is not entitled to server-stored keys (serverByok=false)"
        ));
      }
    }
  }
  match (input.workspace_id, input.user_id, input.local_lease_id) {
    (Some(workspace_id), Some(user_id), Some(lease_id)) => {
      if input.access.local_byok {
        profiles.extend(load_local_profiles(pool, workspace_id, user_id, lease_id, &policy, &mut notes).await?);
      } else {
        notes.push(format!(
          "workspace {workspace_id} is not entitled to locally stored keys (localByok=false)"
        ));
      }
    }
    _ => notes.push(
      "the request carried no locally stored keys: a local lease needs the desktop app, which sends workspace, user and lease id (a browser cannot provide one, so its keys must be stored on the server)".to_string(),
    ),
  }
  profiles.extend(load_managed_profiles(
    &config.copilot,
    input.slot,
    input.built_in_route_id,
    input.access.managed_tier,
    input.managed_target_id,
  )?);
  if profiles.is_empty() && !notes.is_empty() {
    eprintln!(
      "[affine-copilot] no BYOK profile was loaded (workspace={} user={} lease={} serverByok={} localByok={}): {}",
      input.workspace_id.unwrap_or("<none>"),
      input.user_id.unwrap_or("<none>"),
      input.local_lease_id.unwrap_or("<none>"),
      input.access.server_byok,
      input.access.local_byok,
      notes.join(" | ")
    );
  }
  Ok(profiles)
}

async fn load_server_profiles(
  pool: &PgPool,
  workspace_id: &str,
  policy: &ByokPolicy,
  notes: &mut Vec<String>,
) -> RuntimeResult<Vec<AuthorizedProviderProfile>> {
  let rows = sqlx::query_as::<_, ServerProfileRow>(
    r#"
    SELECT id, workspace_id, provider, encrypted_api_key, definition, sort_order, enabled
    FROM ai_workspace_byok_configs
    WHERE workspace_id = $1
    ORDER BY sort_order ASC, created_at ASC
    "#,
  )
  .bind(workspace_id)
  .fetch_all(pool)
  .await
  .map_err(|error| RuntimeError::database("load authorized BYOK profiles failed", error))?;
  if rows.is_empty() {
    notes.push(format!(
      "workspace {workspace_id} has no stored BYOK key at all (ai_workspace_byok_configs is empty for it) - add the key in Settings -> Workspace -> AI while THIS workspace is open"
    ));
    return Ok(Vec::new());
  }
  let mut profiles = Vec::new();
  for row in rows {
    if !row.enabled {
      // The query used to filter these out, which made a disabled key look
      // exactly like a missing one.
      notes.push(format!(
        "stored BYOK profile {} (provider {}) is disabled - enable it in Settings -> Workspace -> AI",
        row.id, row.provider
      ));
      continue;
    }
    let definition = serde_json::from_value::<ByokProfileDefinition>(row.definition)
      .map_err(|error| RuntimeError::json("invalid stored BYOK definition", error))?;
    if !policy.allows(&row.provider, &definition.endpoint) {
      let reason = format!(
        "stored BYOK profile {} (workspace {}, provider {}) is not allowed by the copilot.byok runtime policy - check copilot.byok.enabled / allowedProviders / allowCustomEndpoint / allowPrivateEndpoint",
        row.id, row.workspace_id, row.provider
      );
      eprintln!("[affine-copilot] skipping {reason}");
      notes.push(reason);
      continue;
    }
    let aad = server_aad(
      &row.workspace_id,
      &row.id,
      &row.provider,
      definition.endpoint_identity(),
    );
    profiles.push(authorized_byok_profile(
      row.id,
      ProfileSource::Server,
      row.provider,
      definition,
      policy,
      row.sort_order,
      CredentialRef::Envelope {
        encrypted: row.encrypted_api_key,
        aad,
      },
    ));
  }
  Ok(profiles)
}

async fn load_local_profiles(
  pool: &PgPool,
  workspace_id: &str,
  user_id: &str,
  lease_id: &str,
  policy: &ByokPolicy,
  notes: &mut Vec<String>,
) -> RuntimeResult<Vec<AuthorizedProviderProfile>> {
  let payload = sqlx::query(
    r#"
    SELECT payload
    FROM runtime_states
    WHERE purpose = 'copilot_byok_local_lease' AND token_hash = $1
      AND consumed_at IS NULL AND expires_at > clock_timestamp()
    "#,
  )
  .bind(token_hash(lease_id))
  .fetch_optional(pool)
  .await
  .map_err(|error| RuntimeError::database("load BYOK local lease failed", error))?
  .map(|row| row.get::<serde_json::Value, _>("payload"));
  let Some(payload) = payload else {
    notes.push(
      "the local BYOK lease is unknown, already consumed or expired - reopen the AI panel so the client requests a new one"
        .to_string(),
    );
    return Ok(Vec::new());
  };
  let payload: LocalLeasePayload =
    serde_json::from_value(payload).map_err(|error| RuntimeError::json("invalid BYOK local lease", error))?;
  if payload.workspace_id != workspace_id || payload.user_id != user_id {
    notes.push(format!(
      "the local BYOK lease belongs to workspace {} / user {}, not to this request (workspace {} / user {})",
      payload.workspace_id, payload.user_id, workspace_id, user_id
    ));
    return Ok(Vec::new());
  }
  let mut profiles = Vec::new();
  for (index, provider) in payload.providers.into_iter().enumerate() {
    if !provider.enabled {
      notes.push(format!(
        "locally stored key #{} (provider {}) is disabled",
        index, provider.provider
      ));
      continue;
    }
    if !policy.allows(&provider.provider, &provider.definition.endpoint) {
      notes.push(format!(
        "locally stored key #{} (provider {}) is not allowed by the copilot.byok runtime policy",
        index, provider.provider
      ));
      continue;
    }
    let aad = local_aad(
      workspace_id,
      user_id,
      lease_id,
      index,
      &provider.provider,
      provider.definition.endpoint_identity(),
    );
    profiles.push(authorized_byok_profile(
      format!("{lease_id}:{index}"),
      ProfileSource::Local,
      provider.provider,
      provider.definition,
      policy,
      index as i32,
      CredentialRef::Envelope {
        encrypted: provider.encrypted_credential,
        aad,
      },
    ));
  }
  Ok(profiles)
}

fn load_managed_profiles(
  config: &CopilotRuntimeConfig,
  slot: &CatalogSlot,
  built_in_route_id: Option<&str>,
  managed_tier: route::CopilotManagedTier,
  managed_target_id: Option<&str>,
) -> RuntimeResult<Vec<AuthorizedProviderProfile>> {
  let targets = if let Some(target_id) = managed_target_id {
    vec![
      route::managed_selected_target(built_in_route_id, target_id, managed_tier)
        .ok_or_else(|| RuntimeError::invalid_input("managed_target_unavailable"))?,
    ]
  } else if let Some(targets) = route::managed_targets(slot, built_in_route_id, managed_tier) {
    targets
  } else {
    return Ok(Vec::new());
  };
  targets
    .iter()
    .enumerate()
    .map(|(index, model_id)| {
      let matches = config
        .providers
        .profiles
        .iter()
        .filter(|profile| profile.enabled && profile.models.iter().any(|model| model == model_id))
        .collect::<Vec<_>>();
      let Some(profile) = matches.first() else {
        return Ok(None);
      };
      if matches.len() > 1 {
        return Err(RuntimeError::invalid_state(
          "built-in managed route model matches multiple profiles",
        ));
      }
      let capabilities = provider_default_capability_upper_bound(&profile.provider, model_id)
        .ok_or_else(|| RuntimeError::invalid_state("built-in managed route model is incompatible with its profile"))?;
      let endpoint = managed_endpoint(profile)?;
      Ok(Some(AuthorizedProviderProfile {
        profile_id: profile.id.clone(),
        source: ProfileSource::Managed,
        provider: profile.provider.clone(),
        endpoint,
        openai_dialect: (profile.provider == "openai").then_some(OpenAiDialect::Responses),
        egress_policy: llm_adapter::target::EgressPolicy::PublicOnly,
        models: vec![crate::llm::byok::ByokModelDeclaration {
          model_id: model_id.clone(),
          enabled: true,
          capabilities,
        }],
        sort_order: index as i32,
        credential_ref: CredentialRef::Managed {
          profile_id: profile.id.clone(),
        },
      }))
    })
    .filter_map(|profile| profile.transpose())
    .collect()
}

fn managed_endpoint(profile: &CopilotManagedProfileConfig) -> RuntimeResult<BackendEndpoint> {
  if let Some(base_url) = profile.config.get("baseURL").and_then(serde_json::Value::as_str) {
    return llm_adapter::target::canonicalize_endpoint(base_url)
      .map(BackendEndpoint::Custom)
      .map_err(|error| RuntimeError::invalid_state(error.to_string()));
  }
  let endpoint = match profile.provider.as_str() {
    "geminiVertex" | "anthropicVertex" => {
      let location = required_config_text(profile, "location")?;
      let project = required_config_text(profile, "project")?;
      let publisher = if profile.provider == "geminiVertex" {
        "google"
      } else {
        "anthropic"
      };
      let host = if location == "global" {
        "aiplatform.googleapis.com".to_string()
      } else {
        format!("{location}-aiplatform.googleapis.com")
      };
      format!("https://{host}/v1/projects/{project}/locations/{location}/publishers/{publisher}")
    }
    "cloudflareWorkersAi" => format!(
      "https://api.cloudflare.com/client/v4/accounts/{}/ai",
      required_config_text(profile, "accountId")?
    ),
    _ => return Ok(BackendEndpoint::ProviderDefault),
  };
  Ok(BackendEndpoint::Custom(endpoint))
}

fn authorized_byok_profile(
  profile_id: String,
  source: ProfileSource,
  provider: String,
  definition: ByokProfileDefinition,
  policy: &ByokPolicy,
  sort_order: i32,
  credential_ref: CredentialRef,
) -> AuthorizedProviderProfile {
  let egress_policy = policy.egress_policy(&definition.endpoint);
  let (endpoint, openai_dialect) = match definition.endpoint {
    ByokEndpoint::ProviderDefault => (BackendEndpoint::ProviderDefault, None),
    ByokEndpoint::OpenAiCompatible { url, dialect } => (BackendEndpoint::Custom(url), Some(dialect)),
    ByokEndpoint::AnthropicCompatible { url } => (BackendEndpoint::Custom(url), None),
  };
  AuthorizedProviderProfile {
    profile_id,
    source,
    provider,
    endpoint,
    openai_dialect,
    egress_policy,
    models: definition.models,
    sort_order,
    credential_ref,
  }
}

pub(super) fn managed_profile<'a>(
  config: &'a CopilotRuntimeConfig,
  profile_id: &str,
) -> RuntimeResult<&'a CopilotManagedProfileConfig> {
  config
    .providers
    .profiles
    .iter()
    .find(|profile| profile.id == profile_id && profile.enabled)
    .ok_or_else(|| RuntimeError::invalid_state("managed copilot credential unavailable"))
}

pub(super) fn required_config_text<'a>(
  profile: &'a CopilotManagedProfileConfig,
  field: &'static str,
) -> RuntimeResult<&'a str> {
  profile
    .config
    .get(field)
    .and_then(serde_json::Value::as_str)
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| RuntimeError::invalid_state(format!("managed copilot profile requires {field}")))
}

#[cfg(test)]
mod tests {
  use serde_json::json;
  use sqlx::PgPool;

  use super::{
    BackendEndpoint, BackendRuntimeConfig, CopilotAccessProjection, CopilotManagedProfileConfig, CopilotRuntimeConfig,
    ProfileLoadInput, load_local_profiles, load_profiles, load_server_profiles, managed_endpoint,
  };
  use crate::{
    llm::{
      ByokCapabilityInput, ByokEndpointInput, ByokModelDeclarationInput, ByokProfileDefinitionInput,
      CreateByokLocalLeaseInput, CreateByokLocalLeaseProviderInput, CreateByokProfileInput, byok::ByokPolicy,
    },
    runtime::{Deployment, config::CopilotByokRuntimeConfig},
  };

  fn vertex_profile(location: &str) -> CopilotManagedProfileConfig {
    CopilotManagedProfileConfig {
      id: "vertex".to_string(),
      provider: "geminiVertex".to_string(),
      enabled: true,
      models: vec!["gemini-3.7-flash".to_string()],
      config: json!({ "project": "affine-us", "location": location }),
    }
  }

  #[test]
  fn managed_vertex_endpoint_uses_global_host() {
    assert_eq!(
      managed_endpoint(&vertex_profile("global")).unwrap(),
      BackendEndpoint::Custom(
        "https://aiplatform.googleapis.com/v1/projects/affine-us/locations/global/publishers/google".to_string()
      )
    );
  }

  #[test]
  fn managed_vertex_endpoint_uses_regional_host() {
    assert_eq!(
      managed_endpoint(&vertex_profile("us-central1")).unwrap(),
      BackendEndpoint::Custom(
        "https://us-central1-aiplatform.googleapis.com/v1/projects/affine-us/locations/us-central1/publishers/google"
          .to_string()
      )
    );
  }

  // The tests below need the Prisma-managed `ai_workspace_byok_configs` table, so
  // they skip unless a migrated database is reachable - same convention as
  // `tests.rs`. They exist because `profiles=0` used to be unexplainable: a
  // disabled key, a key stored in another workspace and a policy rejection all
  // produced the identical route diagnostic.

  const BYOK_TEST_SECRET: &[u8] = b"byok-profile-load-test-secret";

  async fn byok_test_context() -> Option<PgPool> {
    let database_url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&database_url).await.ok()?;
    // The native runtime tables (`runtime_states`, ...) come from the runtime
    // migrations, the BYOK table from the server's Prisma migrations.
    crate::runtime::migrations::migrate_runtime_tables(&pool)
      .await
      .ok()?;
    let ready: bool = sqlx::query_scalar(
      "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
       WHERE table_schema = 'public' AND table_name = 'ai_workspace_byok_configs')",
    )
    .fetch_one(&pool)
    .await
    .ok()?;
    ready.then_some(pool)
  }

  async fn insert_workspace(pool: &PgPool, suffix: &str) -> (String, String) {
    let user_id = format!("byok-load-user-{suffix}");
    let workspace_id = format!("byok-load-workspace-{suffix}");
    sqlx::query("INSERT INTO users(id,name,email) VALUES($1,'BYOK Load Test',$2)")
      .bind(&user_id)
      .bind(format!("byok-load-{suffix}@example.com"))
      .execute(pool)
      .await
      .expect("insert byok load test user");
    sqlx::query("INSERT INTO workspaces(id) VALUES($1)")
      .bind(&workspace_id)
      .execute(pool)
      .await
      .expect("insert byok load test workspace");
    (user_id, workspace_id)
  }

  async fn insert_profile(pool: &PgPool, workspace_id: &str, user_id: &str, provider: &str, enabled: bool) -> String {
    let policy = ByokPolicy::from(Deployment::SelfHosted, &CopilotByokRuntimeConfig::default());
    let profile = crate::runtime::backend_runtime::byok::create(
      pool,
      BYOK_TEST_SECRET,
      &policy,
      CreateByokProfileInput {
        workspace_id: workspace_id.to_string(),
        provider: provider.to_string(),
        name: format!("{provider} key"),
        description: None,
        credential: "sk-not-a-real-key".to_string(),
        definition: ByokProfileDefinitionInput {
          endpoint: ByokEndpointInput {
            kind: "provider_default".to_string(),
            url: None,
            dialect: None,
          },
          models: vec![ByokModelDeclarationInput {
            model_id: "gpt-4o-mini".to_string(),
            enabled: true,
            capabilities: vec![ByokCapabilityInput {
              input: vec!["text".to_string()],
              output: vec!["text".to_string()],
              features: Vec::new(),
              attachment_kinds: Vec::new(),
              attachment_sources: Vec::new(),
            }],
          }],
        },
        enabled,
        actor_user_id: user_id.to_string(),
      },
    )
    .await
    .expect("create byok profile through the real path");
    profile.profile_id
  }

  /// Every reason the loader used to swallow, in one run.
  #[tokio::test]
  async fn profile_load_names_every_reason_it_loaded_nothing() {
    let _guard = crate::runtime::migrations::DATABASE_TEST_LOCK.lock().await;
    let Some(pool) = byok_test_context().await else {
      return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let policy = ByokPolicy::from(Deployment::SelfHosted, &CopilotByokRuntimeConfig::default());

    // 1. The key was added in a different workspace: nothing at all is stored here.
    let (_, empty_workspace) = insert_workspace(&pool, &format!("empty-{suffix}")).await;
    let mut notes = Vec::new();
    let profiles = load_server_profiles(&pool, &empty_workspace, &policy, &mut notes)
      .await
      .unwrap();
    assert!(profiles.is_empty());
    assert_eq!(notes.len(), 1);
    assert!(
      notes[0].contains("has no stored BYOK key at all"),
      "unexpected note: {}",
      notes[0]
    );

    // 2. The key is there but switched off. The old query filtered
    //    `enabled = TRUE` in SQL, so this was indistinguishable from (1).
    let (user_id, disabled_workspace) = insert_workspace(&pool, &format!("disabled-{suffix}")).await;
    let disabled_profile = insert_profile(&pool, &disabled_workspace, &user_id, "openai", false).await;
    let mut notes = Vec::new();
    let profiles = load_server_profiles(&pool, &disabled_workspace, &policy, &mut notes)
      .await
      .unwrap();
    assert!(profiles.is_empty());
    assert_eq!(notes.len(), 1, "notes: {notes:?}");
    assert!(notes[0].contains(&disabled_profile), "unexpected note: {}", notes[0]);
    assert!(notes[0].contains("is disabled"), "unexpected note: {}", notes[0]);

    // 3. The deployment's policy does not allow that provider.
    let (user_id, restricted_workspace) = insert_workspace(&pool, &format!("restricted-{suffix}")).await;
    insert_profile(&pool, &restricted_workspace, &user_id, "openai", true).await;
    let restricted_policy = ByokPolicy::from(
      Deployment::SelfHosted,
      &CopilotByokRuntimeConfig {
        allowed_providers: vec!["anthropic".to_string()],
        ..Default::default()
      },
    );
    let mut notes = Vec::new();
    let profiles = load_server_profiles(&pool, &restricted_workspace, &restricted_policy, &mut notes)
      .await
      .unwrap();
    assert!(profiles.is_empty());
    assert_eq!(notes.len(), 1, "notes: {notes:?}");
    assert!(
      notes[0].contains("not allowed by the copilot.byok runtime policy"),
      "unexpected note: {}",
      notes[0]
    );

    // 4. The healthy case still loads, and reports nothing.
    let (user_id, healthy_workspace) = insert_workspace(&pool, &format!("healthy-{suffix}")).await;
    insert_profile(&pool, &healthy_workspace, &user_id, "openai", true).await;
    let mut notes = Vec::new();
    let profiles = load_server_profiles(&pool, &healthy_workspace, &policy, &mut notes)
      .await
      .unwrap();
    assert_eq!(profiles.len(), 1);
    assert!(notes.is_empty(), "notes: {notes:?}");

    for workspace in [
      empty_workspace,
      disabled_workspace,
      restricted_workspace,
      healthy_workspace,
    ] {
      sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(workspace)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
      .bind(format!("byok-load-%{suffix}@example.com"))
      .execute(&pool)
      .await
      .unwrap();
  }

  /// The whole entry point the server calls, so the aggregate
  /// `[affine-copilot] no BYOK profile was loaded (…)` line is produced here too:
  /// run with `--nocapture` to read it.
  #[tokio::test]
  async fn load_profiles_reports_why_it_loaded_nothing() {
    let _guard = crate::runtime::migrations::DATABASE_TEST_LOCK.lock().await;
    let Some(pool) = byok_test_context().await else {
      return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let (user_id, workspace_id) = insert_workspace(&pool, &format!("report-{suffix}")).await;
    insert_profile(&pool, &workspace_id, &user_id, "openai", false).await;

    let slot = crate::llm::route::slot("chat.default").expect("chat.default slot");
    let access = CopilotAccessProjection {
      route_allowed: true,
      managed_tier: crate::llm::route::CopilotManagedTier::Standard,
      server_byok: true,
      local_byok: true,
    };
    let config = BackendRuntimeConfig {
      database_url: std::env::var("DATABASE_URL").unwrap(),
      auth: Default::default(),
      invite_quota: Default::default(),
      private_key: std::sync::Arc::new(zeroize::Zeroizing::new("test-private-key".to_string())),
      deployment: Deployment::SelfHosted,
      copilot: CopilotRuntimeConfig {
        enabled: true,
        byok: CopilotByokRuntimeConfig::default(),
        providers: Default::default(),
      },
      search: Default::default(),
      redis: Default::default(),
      payment: Default::default(),
    };

    let profiles = load_profiles(
      &pool,
      &config,
      ProfileLoadInput {
        slot: &slot,
        built_in_route_id: None,
        workspace_id: Some(&workspace_id),
        user_id: Some(&user_id),
        local_lease_id: None,
        access: &access,
        managed_target_id: None,
      },
    )
    .await
    .unwrap();

    // Self-hosted has no managed provider, and the only stored key is disabled.
    assert!(profiles.is_empty());

    sqlx::query("DELETE FROM workspaces WHERE id = $1")
      .bind(&workspace_id)
      .execute(&pool)
      .await
      .unwrap();
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
      .bind(format!("byok-load-%{suffix}@example.com"))
      .execute(&pool)
      .await
      .unwrap();
  }

  #[tokio::test]
  async fn local_lease_rejections_are_named() {    let _guard = crate::runtime::migrations::DATABASE_TEST_LOCK.lock().await;
    let Some(pool) = byok_test_context().await else {
      return;
    };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let policy = ByokPolicy::from(Deployment::SelfHosted, &CopilotByokRuntimeConfig::default());
    let (user_id, workspace_id) = insert_workspace(&pool, &format!("lease-{suffix}")).await;

    // An unknown, expired or already consumed lease.
    let mut notes = Vec::new();
    let profiles = load_local_profiles(
      &pool,
      &workspace_id,
      &user_id,
      &uuid::Uuid::new_v4().to_string(),
      &policy,
      &mut notes,
    )
    .await
    .unwrap();
    assert!(profiles.is_empty());
    assert_eq!(notes.len(), 1, "notes: {notes:?}");
    assert!(
      notes[0].contains("unknown, already consumed or expired"),
      "unexpected note: {}",
      notes[0]
    );

    // A lease that belongs to a different user.
    let lease = crate::runtime::backend_runtime::byok::create_local_lease(
      &pool,
      BYOK_TEST_SECRET,
      &policy,
      CreateByokLocalLeaseInput {
        workspace_id: workspace_id.clone(),
        user_id: user_id.clone(),
        providers: vec![CreateByokLocalLeaseProviderInput {
          provider: "openai".to_string(),
          name: "local key".to_string(),
          description: None,
          credential: "sk-not-a-real-key".to_string(),
          definition: ByokProfileDefinitionInput {
            endpoint: ByokEndpointInput {
              kind: "provider_default".to_string(),
              url: None,
              dialect: None,
            },
            models: vec![ByokModelDeclarationInput {
              model_id: "gpt-4o-mini".to_string(),
              enabled: true,
              capabilities: vec![ByokCapabilityInput {
                input: vec!["text".to_string()],
                output: vec!["text".to_string()],
                features: Vec::new(),
                attachment_kinds: Vec::new(),
                attachment_sources: Vec::new(),
              }],
            }],
          },
          enabled: true,
        }],
      },
    )
    .await
    .expect("create local lease through the real path");

    // The lease itself is fine when presented by its own workspace and user...
    let mut notes = Vec::new();
    let profiles = load_local_profiles(
      &pool,
      &workspace_id,
      &user_id,
      &lease.lease_id,
      &policy,
      &mut notes,
    )
    .await
    .unwrap();
    assert_eq!(profiles.len(), 1);
    assert!(notes.is_empty(), "notes: {notes:?}");

    // ...and is reported when presented from somewhere else.
    let mut notes = Vec::new();
    let profiles = load_local_profiles(
      &pool,
      &workspace_id,
      "byok-load-user-somebody-else",
      &lease.lease_id,
      &policy,
      &mut notes,
    )
    .await
    .unwrap();
    assert!(profiles.is_empty());
    assert_eq!(notes.len(), 1, "notes: {notes:?}");
    assert!(
      notes[0].contains("belongs to workspace"),
      "unexpected note: {}",
      notes[0]
    );

    sqlx::query("DELETE FROM runtime_states WHERE lookup_key LIKE $1")
      .bind(format!("%{suffix}%"))
      .execute(&pool)
      .await
      .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
      .bind(&workspace_id)
      .execute(&pool)
      .await
      .unwrap();
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
      .bind(format!("byok-load-%{suffix}@example.com"))
      .execute(&pool)
      .await
      .unwrap();
  }
}
