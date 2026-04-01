//! MCP **server** (stdio): expose a curated allowlist of ZeroClaw [`Tool`]s via JSON-RPC.
//!
//! Protocol alignment: `2024-11-05` (same as [`mcp_protocol`] client). Transport: one JSON-RPC
//! object per line on stdin; responses on stdout — matching [`mcp_transport::StdioTransport`].
//!
//! Policy: see [`McpServeConfig`](crate::config::schema::McpServeConfig). By default only a small
//! read-oriented tool set may be listed without `relax_tool_policy`.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{timeout, Duration};

use crate::config::Config;
use crate::memory;
use crate::runtime;
use crate::security::SecurityPolicy;
use crate::skills;
use crate::tools::mcp_protocol::{
    JsonRpcResponse, INVALID_PARAMS, JSONRPC_VERSION, MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND,
    PARSE_ERROR,
};
use crate::tools::traits::Tool;
use crate::tools::{all_tools_with_runtime, register_skill_tools};

/// Tools allowed in `allowed_tools` / `--allow-tool` when `relax_tool_policy` is `false`.
const SAFE_WITHOUT_RELAX: &[&str] = &[
    "memory_recall",
    "file_read",
    "calculator",
    "weather",
    "project_intel",
    "image_info",
    "glob_search",
    "content_search",
    "pdf_read",
];

/// When `[mcp_serve].allowed_tools` and CLI `--allow-tool` are both empty, expose these names.
fn default_mcp_serve_tool_names() -> Vec<String> {
    vec!["memory_recall".into(), "file_read".into()]
}

#[derive(Debug, Deserialize)]
struct InboundRpc {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    params: Option<serde_json::Value>,
}

/// Merge config + CLI allowlist; if nothing specified, use [`default_mcp_serve_tool_names`].
#[must_use]
pub fn merge_mcp_serve_allowlist(
    cfg: &crate::config::schema::McpServeConfig,
    cli: &[String],
) -> Vec<String> {
    let mut names: Vec<String> = cfg.allowed_tools.clone();
    for t in cli {
        if !t.trim().is_empty() {
            names.push(t.trim().to_string());
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() {
        return default_mcp_serve_tool_names();
    }
    names
}

pub fn validate_mcp_serve_allowlist(names: &[String], relax: bool) -> Result<()> {
    if relax {
        return Ok(());
    }
    for n in names {
        if !SAFE_WITHOUT_RELAX.contains(&n.as_str()) {
            bail!(
                "mcp_serve: tool `{n}` is not in the safe preset. \
Add `[mcp_serve].relax_tool_policy = true` after reviewing risk, \
or use only: {}",
                SAFE_WITHOUT_RELAX.join(", ")
            );
        }
    }
    Ok(())
}

fn tool_to_mcp_def(tool: &dyn Tool) -> crate::tools::mcp_protocol::McpToolDef {
    let spec = tool.spec();
    crate::tools::mcp_protocol::McpToolDef {
        name: spec.name,
        description: Some(spec.description),
        input_schema: spec.parameters,
    }
}

fn call_tool_result_json(text: &str, is_error: bool) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn response_ok(id: Option<serde_json::Value>, result: serde_json::Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: Some(result),
        error: None,
    }
}

fn response_err(
    id: Option<serde_json::Value>,
    code: i32,
    message: impl Into<String>,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: None,
        error: Some(crate::tools::mcp_protocol::JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    }
}

async fn write_response(out: &mut tokio::io::Stdout, resp: &JsonRpcResponse) -> Result<()> {
    let line = serde_json::to_string(resp).context("serialize JSON-RPC response")?;
    out.write_all(line.as_bytes()).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

/// Build the filtered registry and run the MCP stdio loop until EOF.
pub async fn run_mcp_stdio_server(config: Config, cli_allow_tools: Vec<String>) -> Result<()> {
    let names = merge_mcp_serve_allowlist(&config.mcp_serve, &cli_allow_tools);
    validate_mcp_serve_allowlist(&names, config.mcp_serve.relax_tool_policy)?;

    let allowed: HashSet<String> = names.iter().cloned().collect();

    let _observer: Arc<dyn crate::observability::Observer> =
        Arc::from(crate::observability::create_observer(&config.observability));
    let runtime: Arc<dyn runtime::RuntimeAdapter> =
        Arc::from(runtime::create_runtime(&config.runtime)?);
    let security = Arc::new(SecurityPolicy::from_config(
        &config.autonomy,
        &config.workspace_dir,
    ));

    let mem: Arc<dyn memory::Memory> = Arc::from(memory::create_memory_with_storage_and_routes(
        &config.memory,
        &config.embedding_routes,
        Some(&config.storage.provider.config),
        &config.workspace_dir,
        config.api_key.as_deref(),
    )?);

    let (composio_key, composio_entity_id) = if config.composio.enabled {
        (
            config.composio.api_key.as_deref(),
            Some(config.composio.entity_id.as_str()),
        )
    } else {
        (None, None)
    };

    let (mut tools_registry, _delegate, _r1, _r2, _r3) = all_tools_with_runtime(
        Arc::new(config.clone()),
        &security,
        runtime,
        mem,
        composio_key,
        composio_entity_id,
        &config.browser,
        &config.http_request,
        &config.web_fetch,
        &config.workspace_dir,
        &config.agents,
        config.api_key.as_deref(),
        &config,
        None,
    );

    let loaded_skills = skills::load_skills_with_config(&config.workspace_dir, &config);
    register_skill_tools(&mut tools_registry, &loaded_skills, security.clone());

    let peripheral_tools: Vec<Box<dyn Tool>> =
        crate::peripherals::create_peripheral_tools(&config.peripherals).await?;
    tools_registry.extend(peripheral_tools);

    // Do not proxy external MCP client tools in this server (separate policy story).
    let mut exposed: Vec<Box<dyn Tool>> = Vec::new();
    for want in &names {
        if let Some(pos) = tools_registry.iter().position(|t| t.name() == want) {
            exposed.push(tools_registry.swap_remove(pos));
        } else {
            tracing::warn!(tool = %want, "mcp serve: tool not in registry (skipping)");
        }
    }

    if exposed.is_empty() {
        bail!(
            "mcp serve: no tools matched the allowlist (check tool names against `zeroclaw` registry)"
        );
    }

    tracing::info!(
        count = exposed.len(),
        tools = ?exposed.iter().map(|t| t.name().to_string()).collect::<Vec<_>>(),
        "MCP server (stdio) ready"
    );

    let timeout_secs = config.mcp_serve.tool_timeout_secs.max(1);

    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = stdin.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: InboundRpc = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = response_err(None, PARSE_ERROR, format!("Parse error: {e}"));
                write_response(&mut stdout, &resp).await?;
                continue;
            }
        };

        let id = req.id.clone();

        match req.method.as_str() {
            "initialize" => {
                let result = serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "zeroclaw",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                });
                write_response(&mut stdout, &response_ok(id, result)).await?;
            }
            "notifications/initialized" => {
                // Notification — no JSON-RPC reply.
            }
            "tools/list" => {
                let tools: Vec<_> = exposed
                    .iter()
                    .map(|t| tool_to_mcp_def(t.as_ref()))
                    .collect();
                write_response(
                    &mut stdout,
                    &response_ok(id, serde_json::json!({ "tools": tools })),
                )
                .await?;
            }
            "tools/call" => {
                let params = match req.params {
                    Some(p) => p,
                    None => {
                        write_response(
                            &mut stdout,
                            &response_err(id, INVALID_PARAMS, "missing params"),
                        )
                        .await?;
                        continue;
                    }
                };
                let name = params
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                if name.is_empty() {
                    write_response(
                        &mut stdout,
                        &response_err(id, INVALID_PARAMS, "tools/call requires name"),
                    )
                    .await?;
                    continue;
                }

                if !allowed.contains(name) {
                    write_response(
                        &mut stdout,
                        &response_err(
                            id,
                            INVALID_PARAMS,
                            format!("tool `{name}` is not on the mcp_serve allowlist"),
                        ),
                    )
                    .await?;
                    continue;
                }

                let Some(tool) = exposed.iter().find(|t| t.name() == name) else {
                    write_response(
                        &mut stdout,
                        &response_err(id, INVALID_PARAMS, format!("tool `{name}` not available")),
                    )
                    .await?;
                    continue;
                };

                let exec_fut = tool.execute(arguments);
                let outcome = timeout(Duration::from_secs(timeout_secs), exec_fut).await;

                let result_payload = match outcome {
                    Ok(Ok(r)) => {
                        if r.success {
                            call_tool_result_json(&r.output, false)
                        } else {
                            call_tool_result_json(
                                &r.error.unwrap_or_else(|| r.output.clone()),
                                true,
                            )
                        }
                    }
                    Ok(Err(e)) => call_tool_result_json(&format!("execution error: {e:#}"), true),
                    Err(_) => call_tool_result_json(
                        &format!("tool `{name}` timed out after {timeout_secs}s"),
                        true,
                    ),
                };

                write_response(&mut stdout, &response_ok(id, result_payload)).await?;
            }
            "ping" => {
                write_response(&mut stdout, &response_ok(id, serde_json::json!({}))).await?;
            }
            _ => {
                write_response(
                    &mut stdout,
                    &response_err(
                        id,
                        METHOD_NOT_FOUND,
                        format!("method not found: {}", req.method),
                    ),
                )
                .await?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::McpServeConfig;

    #[test]
    fn merge_defaults_when_empty() {
        let cfg = McpServeConfig::default();
        let m = merge_mcp_serve_allowlist(&cfg, &[]);
        assert_eq!(m, vec!["memory_recall", "file_read"]);
    }

    #[test]
    fn merge_dedupes() {
        let cfg = McpServeConfig {
            allowed_tools: vec!["calculator".into(), "file_read".into()],
            ..Default::default()
        };
        let m = merge_mcp_serve_allowlist(&cfg, &["file_read".into()]);
        assert_eq!(m, vec!["calculator", "file_read"]);
    }

    #[test]
    fn validate_rejects_unsafe_without_relax() {
        let err = validate_mcp_serve_allowlist(&["shell".into()], false).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("relax_tool_policy"));
    }

    #[test]
    fn validate_allows_shell_with_relax() {
        validate_mcp_serve_allowlist(&["shell".into()], true).unwrap();
    }
}
