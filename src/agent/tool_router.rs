//! Optional small-model router that selects a subset of tools per user message.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::config::schema::ToolRouterConfig;
use crate::tools::Tool;

const ROUTER_PROMPT: &str = r#"You are an ultra-fast tool router.
Given the current user message and the full list of available tools, return ONLY a JSON array of the MOST relevant tool names.

Rules:
- Return valid JSON only: ["tool_name1", "tool_name2", ...]
- Choose tools that can actually help solve the query
- If unsure, pick fewer rather than more
- Never add explanations or markdown fences

User query:

{{USER_QUERY}}

Available tools (JSON):

{{TOOL_LIST_JSON}}"#;

/// Merge base exclusions (e.g. `tool_filter_groups` + autonomy) with router-driven exclusions.
pub async fn merge_exclusions_for_turn(
    base_excluded: Vec<String>,
    cfg: &ToolRouterConfig,
    registry: &[Box<dyn Tool>],
    user_message: &str,
    deferred_mcp: bool,
) -> Vec<String> {
    if !cfg.enabled {
        return dedupe_excluded(base_excluded);
    }

    let base_set: HashSet<String> = base_excluded.iter().cloned().collect();
    let candidates: HashSet<String> = registry
        .iter()
        .map(|t| t.name().to_string())
        .filter(|n| !base_set.contains(n))
        .collect();

    if candidates.is_empty() {
        return dedupe_excluded(base_excluded);
    }

    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        tracing::warn!(
            "agent.tool_router.enabled is true but base_url or model is empty; skipping router"
        );
        return dedupe_excluded(base_excluded);
    }

    let url = chat_completions_url(&cfg.base_url);
    if url.is_empty() {
        return dedupe_excluded(base_excluded);
    }

    let tool_json = build_tool_catalog_json(registry, &candidates);
    let prompt = ROUTER_PROMPT
        .replace("{{USER_QUERY}}", user_message)
        .replace("{{TOOL_LIST_JSON}}", &tool_json);

    let body = serde_json::json!({
        "model": cfg.model,
        "temperature": cfg.temperature,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 512u32,
    });

    let builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(cfg.timeout_ms.max(1)))
        .connect_timeout(Duration::from_secs(10));
    let builder = crate::config::apply_runtime_proxy_to_builder(builder, "agent.tool_router");
    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "tool_router: failed to build HTTP client");
            return dedupe_excluded(base_excluded);
        }
    };

    let mut req = client.post(&url).json(&body);
    if let Some(ref key) = cfg.api_key {
        if !key.is_empty() {
            req = req.bearer_auth(key);
        }
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "tool_router: request failed");
            return dedupe_excluded(base_excluded);
        }
    };

    let status = response.status();
    let text = match response.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "tool_router: failed to read body");
            return dedupe_excluded(base_excluded);
        }
    };

    if !status.is_success() {
        tracing::warn!(status = %status, body = %truncate_for_log(&text), "tool_router: non-success response");
        return dedupe_excluded(base_excluded);
    }

    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "tool_router: invalid JSON response");
            return dedupe_excluded(base_excluded);
        }
    };

    let content = parsed
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    let router_names = match extract_json_array_from_text(content) {
        Some(names) => names,
        None => {
            tracing::warn!(content = %truncate_for_log(content), "tool_router: could not parse tool array");
            return dedupe_excluded(base_excluded);
        }
    };

    if router_names.is_empty() {
        if cfg.fallback_to_all_tools {
            tracing::debug!("tool_router: empty JSON array; fallback keeps all candidate tools");
            return dedupe_excluded(base_excluded);
        }
        let mut out = base_excluded;
        out.extend(candidates.iter().cloned());
        return dedupe_excluded(out);
    }

    let mut router_names: Vec<String> = router_names
        .into_iter()
        .filter(|n| candidates.contains(n))
        .collect();
    router_names.dedup();

    let allowed =
        select_allowed_tools(cfg, &candidates, &router_names, deferred_mcp, cfg.max_tools);

    if allowed.is_empty() {
        if cfg.fallback_to_all_tools {
            return dedupe_excluded(base_excluded);
        }
        let mut out = base_excluded;
        out.extend(candidates.iter().cloned());
        return dedupe_excluded(out);
    }

    let allowed_set: HashSet<String> = allowed.into_iter().collect();
    let mut out = base_excluded;
    for name in candidates {
        if !allowed_set.contains(&name) {
            out.push(name);
        }
    }
    dedupe_excluded(out)
}

fn select_allowed_tools(
    cfg: &ToolRouterConfig,
    candidates: &HashSet<String>,
    router_names: &[String],
    deferred_mcp: bool,
    max_tools: usize,
) -> Vec<String> {
    let cap = max_tools.max(1);
    let mut out: Vec<String> = Vec::new();

    for n in &cfg.always_include {
        if out.len() >= cap {
            break;
        }
        if candidates.contains(n) && !out.contains(n) {
            out.push(n.clone());
        }
    }

    if deferred_mcp
        && out.len() < cap
        && candidates.contains("tool_search")
        && !out.contains(&"tool_search".to_string())
    {
        out.push("tool_search".to_string());
    }

    for n in router_names {
        if out.len() >= cap {
            break;
        }
        if candidates.contains(n) && !out.contains(n) {
            out.push(n.clone());
        }
    }

    out
}

fn dedupe_excluded(mut v: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    v.retain(|n| seen.insert(n.clone()));
    v
}

fn truncate_for_log(s: &str) -> String {
    const MAX: usize = 400;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

fn chat_completions_url(base_url: &str) -> String {
    let t = base_url.trim();
    if t.is_empty() {
        return String::new();
    }
    let trimmed = t.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn build_tool_catalog_json(registry: &[Box<dyn Tool>], candidates: &HashSet<String>) -> String {
    let mut rows: Vec<HashMap<String, String>> = Vec::new();
    for t in registry {
        let name = t.name().to_string();
        if !candidates.contains(&name) {
            continue;
        }
        let spec = t.spec();
        let desc = spec.description.chars().take(200).collect::<String>();
        let mut m = HashMap::new();
        m.insert("name".to_string(), name);
        m.insert("description".to_string(), desc);
        rows.push(m);
    }
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string())
}

fn extract_json_array_from_text(content: &str) -> Option<Vec<String>> {
    let stripped = strip_markdown_fence(content);
    let slice = if let Some(start) = stripped.find('[') {
        let tail = &stripped[start..];
        let end = find_matching_bracket_end(tail)?;
        &tail[..=end]
    } else {
        return None;
    };

    let parsed: Vec<String> = serde_json::from_str(slice).ok()?;
    Some(parsed)
}

fn strip_markdown_fence(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let rest = rest.trim_start_matches("json").trim_start_matches("JSON");
        let rest = rest.trim();
        if let Some(idx) = rest.rfind("```") {
            return rest[..idx].trim().to_string();
        }
        return rest.to_string();
    }
    t.to_string()
}

fn find_matching_bracket_end(tail: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in tail.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matching_bracket_nested() {
        let s = r#"Some text ["a", "b"] tail"#;
        let tail = &s[s.find('[').unwrap()..];
        // Closing `]` of the JSON array (not the last char of `tail`).
        assert_eq!(find_matching_bracket_end(tail), Some(9));
    }

    #[test]
    fn extract_json_array_from_text_fenced() {
        let s = "```json\n[\"a\", \"b\"]\n```";
        let v = extract_json_array_from_text(s).unwrap();
        assert_eq!(v, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn chat_completions_url_appends_path() {
        assert_eq!(
            chat_completions_url("http://127.0.0.1:11437/v1"),
            "http://127.0.0.1:11437/v1/chat/completions"
        );
    }
}
