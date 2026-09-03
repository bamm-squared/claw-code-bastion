use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{
    detect_provider_kind, AnthropicClient, InputMessage, MessageRequest, OpenAiCompatClient,
    OpenAiCompatConfig, ProviderClient, ProviderKind,
};
use runtime::ConfigLoader;
use serde_json::{json, Value};

use crate::{
    provider_endpoint, provider_privacy_class, redacted_provider_endpoint, ProviderPrivacyClass,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAction {
    Setup { skip_verification: bool },
    Status,
}

pub fn run(action: ProviderAction, output_json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        ProviderAction::Status => print_status(output_json),
        ProviderAction::Setup { skip_verification } => setup(output_json, skip_verification),
    }
}

pub fn is_configured() -> bool {
    let model = effective_model();
    let kind = detect_provider_kind(&model);
    let endpoint = provider_endpoint(&model);
    credential_source(kind, &model) != "none"
        || (kind == ProviderKind::OpenAi
            && ["http://localhost", "http://127.0.0.1", "http://[::1]"]
                .iter()
                .any(|prefix| endpoint.starts_with(prefix)))
}

fn effective_model() -> String {
    let cwd = env::current_dir().unwrap_or_default();
    ConfigLoader::default_for(cwd)
        .load()
        .ok()
        .and_then(|config| {
            config.model().map(ToOwned::to_owned).or_else(|| {
                serde_json::from_str::<Value>(&config.as_json().render())
                    .ok()
                    .and_then(|root| {
                        root.get("modelResources")?
                            .as_array()?
                            .first()?
                            .get("model")?
                            .as_str()
                            .map(str::to_owned)
                    })
            })
        })
        .or_else(|| {
            env::var("ANTHROPIC_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "claude-opus-4-6".to_string())
}

fn credential_source(kind: ProviderKind, model: &str) -> &'static str {
    match kind {
        ProviderKind::Anthropic => {
            if env::var_os("ANTHROPIC_API_KEY").is_some()
                || env::var_os("ANTHROPIC_AUTH_TOKEN").is_some()
            {
                "environment"
            } else if runtime::load_oauth_credentials().ok().flatten().is_some() {
                "trusted store"
            } else {
                "none"
            }
        }
        ProviderKind::Xai => env::var_os("XAI_API_KEY").map_or("none", |_| "environment"),
        ProviderKind::OpenAi => {
            if model.starts_with("qwen") && env::var_os("DASHSCOPE_API_KEY").is_some() {
                "environment"
            } else {
                env::var_os("OPENAI_API_KEY").map_or("none", |_| "environment")
            }
        }
    }
}

fn print_status(output_json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let model = effective_model();
    let kind = detect_provider_kind(&model);
    let endpoint = provider_endpoint(&model);
    let class = provider_privacy_class(&model);
    let credential = credential_source(kind, &model);
    let verification = verification_state(&model, &endpoint);
    let configured = credential != "none"
        || (kind == ProviderKind::OpenAi
            && ["http://localhost", "http://127.0.0.1", "http://[::1]"]
                .iter()
                .any(|prefix| endpoint.starts_with(prefix)));
    if output_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "provider",
                "configured": configured,
                "provider": provider_label(kind),
                "model": model,
                "endpoint": redacted_provider_endpoint(&endpoint),
                "privacy": class.label(),
                "credential_source": credential,
                "verification": verification.state,
                "last_verified": verification.verified_at,
            }))?
        );
    } else {
        println!(
            "Claw Provider Status\n────────────────────\n\nProvider       {provider}\nModel          {model}\nEndpoint       {endpoint}\nPrivacy        {privacy}\nCredentials    {credential}\nConfigured     {configured}\nVerification   {verification}\nLast verified  {last_verified}",
            provider = provider_label(kind),
            endpoint = redacted_provider_endpoint(&endpoint),
            privacy = class.label(),
            configured = if configured { "yes" } else { "no" },
            verification = verification.state.to_ascii_uppercase(),
            last_verified = verification.verified_at.as_deref().unwrap_or("never"),
        );
        if matches!(
            class,
            ProviderPrivacyClass::RemoteStandard | ProviderPrivacyClass::Unknown
        ) {
            println!(
                "\nWarning: prompts, code, and tool results are sent to the selected provider."
            );
        }
    }
    Ok(())
}

fn setup(output_json: bool, skip_verification: bool) -> Result<(), Box<dyn std::error::Error>> {
    if output_json || !io::stdin().is_terminal() {
        return Err("provider setup requires an interactive terminal; configure trusted environment variables for headless use and run `claw provider status`".into());
    }
    println!("Claw Provider Setup\n\nChoose a provider:\n  1. Local OpenAI-compatible\n  2. Confidential remote\n  3. Standard remote\n  4. Custom OpenAI-compatible");
    let choice = prompt("Selection [1-4]: ")?;
    let (model, endpoint, privacy, credential) = match choice.trim() {
        "1" => (
            prompt_default("Model", "qwen2.5-coder")?,
            prompt_default("Endpoint", "http://127.0.0.1:11434/v1")?,
            "LOCAL",
            "OPENAI_API_KEY",
        ),
        "2" => (
            prompt("Model: ")?,
            prompt("Endpoint: ")?,
            "CONFIDENTIAL",
            "provider-specific trusted credential",
        ),
        "3" => (
            prompt("Model: ")?,
            String::new(),
            "REMOTE STANDARD",
            "provider-specific environment variable or trusted OAuth store",
        ),
        "4" => (
            prompt("Model: ")?,
            prompt("Endpoint: ")?,
            "UNKNOWN",
            "OPENAI_API_KEY",
        ),
        _ => return Err("select 1, 2, 3, or 4".into()),
    };
    let config_path = ConfigLoader::default_for(env::current_dir()?)
        .config_home()
        .join("settings.json");
    let resolved_endpoint = if endpoint.is_empty() {
        provider_endpoint(&model)
    } else {
        endpoint.clone()
    };
    if skip_verification {
        persist_verification(&model, &resolved_endpoint, "unverified")?;
        println!("\nVerification skipped explicitly; this provider is UNVERIFIED.");
    } else {
        println!("\nVerifying the provider with a minimal harmless request...");
        verify_provider(&model, &resolved_endpoint)?;
        persist_verification(&model, &resolved_endpoint, "verified")?;
    }
    persist_model(&config_path, &model)?;
    println!(
        "\nProvider selection saved to trusted user settings: {}",
        config_path.display()
    );
    println!("Privacy class: {privacy}\nCredential source: {credential}");
    if !endpoint.is_empty() {
        println!("Set the trusted process environment before starting Claw:\n  OPENAI_BASE_URL={endpoint} claw");
    }
    if privacy == "REMOTE STANDARD" {
        println!("Warning: prompts, code, and tool results are sent to this provider.");
    }
    println!("Run `claw provider status` to inspect the resolved configuration.");
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct VerificationRecord {
    state: String,
    verified_at: Option<String>,
}

fn verification_path() -> std::path::PathBuf {
    ConfigLoader::default_for(env::current_dir().unwrap_or_default())
        .config_home()
        .join("provider-verification.json")
}

fn verification_state(model: &str, endpoint: &str) -> VerificationRecord {
    let Ok(contents) = fs::read_to_string(verification_path()) else {
        return VerificationRecord {
            state: "unverified".to_string(),
            ..Default::default()
        };
    };
    let Ok(record) = serde_json::from_str::<Value>(&contents) else {
        return VerificationRecord {
            state: "unverified".to_string(),
            ..Default::default()
        };
    };
    let endpoint_identity = redacted_provider_endpoint(endpoint);
    if record.get("model").and_then(Value::as_str) != Some(model)
        || record.get("endpoint").and_then(Value::as_str) != Some(endpoint_identity.as_str())
    {
        return VerificationRecord {
            state: "unverified".to_string(),
            ..Default::default()
        };
    }
    VerificationRecord {
        state: record
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unverified")
            .to_string(),
        verified_at: record
            .get("verified_at")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    }
}

fn persist_verification(
    model: &str,
    endpoint: &str,
    state: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = verification_path();
    fs::create_dir_all(path.parent().ok_or("verification path has no parent")?)?;
    let verified_at = (state == "verified").then(|| {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
            |_| "unknown".to_string(),
            |duration| duration.as_secs().to_string(),
        )
    });
    fs::write(
        path,
        serde_json::to_string_pretty(&json!({
            "model": model,
            "endpoint": redacted_provider_endpoint(endpoint),
            "state": state,
            "verified_at": verified_at,
        }))? + "\n",
    )?;
    Ok(())
}

fn verify_provider(model: &str, endpoint: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = match detect_provider_kind(model) {
        ProviderKind::Anthropic => {
            ProviderClient::Anthropic(AnthropicClient::from_env()?.with_base_url(endpoint))
        }
        ProviderKind::Xai => ProviderClient::Xai(
            OpenAiCompatClient::from_env(OpenAiCompatConfig::xai())?.with_base_url(endpoint),
        ),
        ProviderKind::OpenAi => ProviderClient::OpenAi(
            OpenAiCompatClient::new("", OpenAiCompatConfig::openai()).with_base_url(endpoint),
        ),
    };
    let request = MessageRequest {
        model: model.to_string(),
        max_tokens: 8,
        messages: vec![InputMessage::user_text("Reply with OK.")],
        ..MessageRequest::default()
    };
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(15), client.send_message(&request))
            .await
            .map_err(|_| "provider verification timed out".to_string())?
            .map(|_| ())
            .map_err(|error| format!("provider verification failed: {error}"))
    })?;
    Ok(())
}

fn prompt(label: &str) -> io::Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_string())
}

fn prompt_default(label: &str, default: &str) -> io::Result<String> {
    let value = prompt(&format!("{label} [{default}]: "))?;
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value
    })
}

fn persist_model(path: &Path, model: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(path)?)?
    } else {
        json!({})
    };
    let object = root
        .as_object_mut()
        .ok_or("trusted settings file must contain a JSON object")?;
    object.insert("model".to_string(), Value::String(model.to_string()));
    fs::create_dir_all(path.parent().ok_or("settings path has no parent")?)?;
    fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(())
}

fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "Anthropic",
        ProviderKind::Xai => "xAI",
        ProviderKind::OpenAi => "OpenAI-compatible",
    }
}
