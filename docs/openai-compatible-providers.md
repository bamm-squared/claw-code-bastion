# OpenAI-compatible provider configuration

Claw supports explicit OpenAI-compatible connections through `modelResources`.
This separates the endpoint connection, wire protocol, and model capabilities.
Adding a compliant endpoint or changing its model identifier does not require a
Rust source change.

## Configuration shape

Each resource has a model identity and an optional `connection` plus explicit
compatible transport profile:

```json
{
  "modelResources": [
    {
      "id": "official-like",
      "provider": "acme-cloud",
      "model": "acme/reasoner-1",
      "privacy": "remote",
      "enabled": true,
      "capability": {
        "coding": 95,
        "reasoning": 95,
        "agent_tool_use": 95,
        "planning": 90,
        "evaluation": 85,
        "context_window": 128000
      },
      "pricing": {
        "actual_cost_known": true,
        "actual_input_micros": 1500,
        "actual_output_micros": 6000
      },
      "connection": {
        "id": "acme-primary",
        "baseUrl": "https://llm.example.test/v1",
        "credentialEnv": "ACME_LLM_TOKEN",
        "auth": "bearer",
        "headerEnv": {
          "x-tenant-id": "ACME_TENANT_ID"
        },
        "timeoutMs": 120000,
        "maxRetries": 2
      },
      "protocol": "responses",
      "protocolCapabilities": {
        "chatCompletions": false,
        "responses": true,
        "functionTools": true,
        "reasoning": true,
        "streaming": true,
        "typedImages": false
      },
      "reasoning": {
        "supported": true,
        "defaultEffort": "medium",
        "allowedEfforts": ["low", "medium", "high"],
        "parameter": "reasoning_effort",
        "supportsWithTools": true
      },
      "parameterCapabilities": {
        "maxOutputTokens": true,
        "maxOutputTokensParameter": "max_output_tokens",
        "temperature": false,
        "topP": false,
        "toolChoice": true,
        "streamUsage": true
      }
    }
  ]
}
```

`provider` and `id` are labels, not protocol selectors. `model` is sent as an
opaque string. `baseUrl` is required for an unrecognized provider label unless
`baseUrlEnv` is used. URL paths are resolved according to the declared
protocol, so a `/v1` base URL is suitable for both `/chat/completions` and
`/responses`.

For a trusted local endpoint, use `"auth": "none"`. For bearer authentication,
set `credentialEnv` to the environment variable containing the token. Header
values should normally be supplied through `headerEnv`; literal `headers` are
intended for non-secret routing metadata. Secrets are never included in
telemetry.

## Protocols and normalized behavior

The supported explicit protocols are:

- `chat_completions`: OpenAI Chat Completions-compatible JSON and SSE.
- `responses`: OpenAI Responses-compatible JSON and SSE.

Both transports normalize assistant text, tool calls, refusals, terminal
status, request/response IDs, usage, provider errors, and optional rate-limit
metadata into the common Bastion model-turn stream. Responses output may be
finalized through text deltas, content-part completion, output-item completion,
or the completed response snapshot. Tool-only turns and multiple tool calls
are preserved without duplicate execution.

An endpoint claiming compatibility must return the declared protocol's normal
message shape, terminal status, and tool shape when `functionTools` is true.
Streaming must be declared when the profile is used by a streaming runtime.
Malformed or contradictory output fails clearly as a provider compatibility
error; it is not treated as a coding failure and is not retried indefinitely.

This is a capability contract, not a hostname check. A Chat Completions URL
does not imply that every model behind it supports tools, reasoning, or every
optional parameter.

## Reasoning and optional parameters

`reasoning` is model-profile data. It is not inferred from `gpt`, `Luna`, a
provider name, or the endpoint hostname. `defaultEffort` controls what
`reasoningProfile: "default"` means for that profile. An explicit
`reasoningProfile` selects a declared value, `none`/`off` omits the control,
and unsupported values are rejected before a provider call.

The profile's `parameter` allows a compatible endpoint to use a different wire
field. If reasoning is unsupported, or unsupported with tools, the field is
omitted. The same omission policy applies to optional parameters declared
unsupported by `parameterCapabilities`. Unavailable provider defaults are not
silently represented as a requested Bastion effort; effective effort is
recorded in provider telemetry.

## Routing, limits, usage, and pricing

The numeric `capability` object supplies routing scores. Explicit compatible
profiles additionally constrain writer eligibility when function tools are not
declared. Routing rejection telemetry identifies the role, profile, and
reason, including capability-threshold failures.

Set `capability.context_window` to the model's usable context limit. Unknown
limits should use a conservative value rather than assuming an OpenAI family
limit. `pricing` is optional: unknown or local models can omit it, while
configured input/output prices are used for accounting. Reference prices remain
comparison data and are not reported as actual spend.

Provider usage is normalized when returned. Missing usage remains unavailable
to the provider accounting path rather than being invented. Rate-limit headers
are optional; any recognized token/request limits and reset intervals are
retained, while endpoints without those headers continue without fabricated
headroom.

## Examples

Local/self-hosted Chat Completions:

```json
{
  "id": "local-model",
  "provider": "self-hosted",
  "model": "my-model-v7",
  "connection": {
    "baseUrl": "http://localhost:8000/v1",
    "auth": "none"
  },
  "protocol": "chat_completions",
  "protocolCapabilities": {"functionTools": true, "reasoning": false}
}
```

Generic third-party Responses endpoint:

```json
{
  "id": "proxy-reasoner",
  "provider": "gateway-b",
  "model": "proxy/model-name",
  "connection": {
    "baseUrlEnv": "GATEWAY_B_BASE_URL",
    "credentialEnv": "GATEWAY_B_TOKEN"
  },
  "protocol": "responses",
  "reasoning": {"supported": true, "defaultEffort": "low"}
}
```

The existing legacy `OPENAI_API_KEY`/`OPENAI_BASE_URL` setup remains supported.
Use an explicit profile when provider, protocol, authentication, model
capabilities, or reasoning behavior must be controlled rather than inferred.
