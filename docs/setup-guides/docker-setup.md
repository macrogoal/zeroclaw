# Docker setup and agent surface reference

This page gives a **single map** of the CLI, built-in tools, MCP, skills, and permission-related config, plus a short **Docker image orientation** (no build/run steps). Deeper detail lives in the linked reference docs.

Last aligned with repo layout: **April 2026**.

---

## Docker image

The root [`Dockerfile`](../../Dockerfile) builds the `zeroclaw` binary and defines two runtime stages:

| Stage | Base | Typical use |
|-------|------|-------------|
| `dev` | `debian:trixie-slim` | Local development; includes `curl`, `ca-certificates`; config from `dev/config.template.toml`. |
| `release` | `gcr.io/distroless/cc-debian13:nonroot` | Minimal production image; you supply API keys and config at runtime. |

**Defaults (both stages):**

- **Entrypoint:** `zeroclaw` — **default CMD:** `daemon` (supervised runtime).
- **Listen:** gateway **port `42617`** (`EXPOSE 42617`); healthcheck: `zeroclaw status --format=exit-code`.
- **Paths:** `HOME=/zeroclaw-data`, workspace `ZEROCLAW_WORKSPACE=/zeroclaw-data/workspace`, config under `/zeroclaw-data/.zeroclaw/`.
- **User:** non-root UID `65534`.

The builder stage seeds a minimal `config.toml` under `/zeroclaw-data/.zeroclaw/` (overridden in `dev` by the template). It includes an **`[autonomy]`** block with `auto_approve` listing several built-in tool names (read the file for the exact set shipped with the image).

**Container + config:** For provider overrides and custom endpoints, see the operational note in [`config-reference.md`](../reference/api/config-reference.md) (custom `api_url` vs `ZEROCLAW_PROVIDER`).

---

## CLI command map

Full command surface (flags and subcommands): [`commands-reference.md`](../reference/cli/commands-reference.md).

**High-signal commands for tools, MCP, skills, and safety:**

| Area | Commands |
|------|----------|
| Interactive / one-shot agent | `zeroclaw agent`, `zeroclaw agent -m "..."` |
| Supervised runtime (gateway + channels) | `zeroclaw daemon`, `zeroclaw gateway` |
| **MCP server** (expose ZeroClaw tools to editors) | `zeroclaw mcp serve` (stdio or HTTP) |
| **Skills** | `zeroclaw skills list`, `install`, `remove`, `audit` |
| **Emergency stop / tool freeze** | `zeroclaw estop`, `zeroclaw estop --level tool-freeze --tool <name>`, `zeroclaw estop resume` |
| Diagnostics | `zeroclaw status`, `zeroclaw doctor` |

Validate against your binary: `zeroclaw --help` and `zeroclaw <command> --help`.

---

## Built-in tools (registry)

Rust-implemented tools are wired in [`all_tools_with_runtime`](../../src/tools/mod.rs) (and helpers such as peripheral factories under `src/peripherals/`). The table below lists **stable tool names** (`Tool::name()`). Registration is often **config-gated**; if a feature is off or prerequisites are missing, that tool is not added to the runtime registry.

**Drift warning:** New releases may add or gate tools. Confirm in source or by inspecting the live tool list (for example [`zeroclaw mcp serve`](../mcp-serve.md) and JSON-RPC `tools/list` for an allowlisted surface).

**Agent loop:** [`[agent]`](../reference/api/config-reference.md) keys include `max_tool_iterations`, `parallel_tools`, `tool_call_dedup_exempt`, **`tool_filter_groups`** (MCP tool schemas), and optional **`[agent.tool_router]`** (small model narrows visible tools per user message).

Extending the codebase: [`change-playbooks.md`](../contributing/change-playbooks.md) (“Adding a Tool”).

### In-process and peripheral tools (alphabetical)

| Tool name | Availability / notes |
|-----------|-------------------------|
| `arduino_upload` | Peripherals: `arduino-uno` board with a serial `path` configured. |
| `ask_user` | Core registry (interactive approval; channel map wired at runtime). |
| `backup` | `[backup].enabled` (on by default). |
| `browser` | `[browser].enabled`. |
| `browser_delegate` | `[browser_delegate].enabled` and runtime allows shell access. |
| `browser_open` | `[browser].enabled`. |
| `calculator` | Core registry. |
| `canvas` | Core registry. |
| `claude_code` | `[claude_code].enabled`. |
| `claude_code_runner` | `[claude_code_runner].enabled`. |
| `cloud_ops` | `[cloud_ops].enabled`. |
| `cloud_patterns` | `[cloud_ops].enabled`. |
| `codex_cli` | `[codex_cli].enabled`. |
| `composio` | Composio API key configured for the runtime. |
| `content_search` | Core registry. |
| `cron_add` | Core registry. |
| `cron_list` | Core registry. |
| `cron_remove` | Core registry. |
| `cron_run` | Core registry. |
| `cron_runs` | Core registry. |
| `cron_update` | Core registry. |
| `data_management` | `[data_retention].enabled`. |
| `delegate` | Non-empty `[agents.<name>]` delegate profiles in `config.toml`. |
| `discord_search` | `channels_config.discord_history` is set and sqlite memory opens successfully. |
| `file_edit` | Core registry. |
| `file_read` | Core registry. |
| `file_write` | Core registry. |
| `gemini_cli` | `[gemini_cli].enabled`. |
| `git_operations` | Core registry. |
| `glob_search` | Core registry. |
| `google_workspace` | `[google_workspace].enabled` and runtime allows shell access. |
| `gpio_read` | Peripherals: serial or RPi GPIO boards connected. |
| `gpio_write` | Peripherals: serial or RPi GPIO boards connected. |
| `hardware_board_info` | Peripherals enabled with `[peripherals.boards]` (see `create_board_info_tools` / connected boards). |
| `hardware_capabilities` | Peripherals: at least one serial board connected. |
| `hardware_memory_map` | Peripherals enabled with configured boards. |
| `hardware_memory_read` | Peripherals enabled with configured boards. |
| `http_request` | `[http_request].enabled`. |
| `image_gen` | `[image_gen].enabled`. |
| `image_info` | Core registry. |
| `jira` | `[jira].enabled` with base URL, email, and token. |
| `knowledge` | `[knowledge].enabled` and knowledge DB initializes. |
| `linkedin` | `[linkedin].enabled`. |
| `llm_task` | Core registry (uses default provider/model from config). |
| `memory_forget` | Core registry. |
| `memory_purge` | Core registry. |
| `memory_recall` | Core registry. |
| `memory_store` | Core registry. |
| `microsoft365` | `[microsoft365].enabled` with tenant/client (and secret when required). |
| `model_routing_config` | Core registry. |
| `model_switch` | Core registry. |
| `notion` | `[notion].enabled` and API key present (`notion.api_key` or `NOTION_API_KEY`). |
| `opencode_cli` | `[opencode_cli].enabled`. |
| `pdf_read` | Core registry (PDF feature enabled at build when applicable). |
| `poll` | Core registry. |
| `project_intel` | `[project_intel].enabled`. |
| `proxy_config` | Core registry. |
| `pushover` | Core registry. |
| `reaction` | Core registry. |
| `read_skill` | `[skills].prompt_injection_mode = "compact"`. |
| `schedule` | Core registry. |
| `screenshot` | Core registry. |
| `security_ops` | `[security_ops].enabled`. |
| `sessions_history` | Session store under the workspace initializes successfully. |
| `sessions_list` | Session store under the workspace initializes successfully. |
| `sessions_send` | Session store under the workspace initializes successfully. |
| `shell` | Core registry. |
| `sop_advance` | `[sop].sops_dir` configured. |
| `sop_approve` | `[sop].sops_dir` configured. |
| `sop_execute` | `[sop].sops_dir` configured. |
| `sop_list` | `[sop].sops_dir` configured. |
| `sop_status` | `[sop].sops_dir` configured. |
| `swarm` | Non-empty `[swarms]` table in `config.toml`. |
| `text_browser` | `[text_browser].enabled`. |
| `tool_search` | `[mcp]` enabled with servers and **`[mcp].deferred_loading = true`** (stubs + on-demand activation). |
| `vi_verify` | `[verifiable_intent].enabled`. |
| `weather` | Core registry. |
| `web_fetch` | `[web_fetch].enabled`. |
| `web_search_tool` | `[web_search].enabled`. |
| `workspace` | `[workspace].enabled` (workspace isolation / multi-workspace management). |

### MCP client tools (dynamic names)

When ZeroClaw is configured as an **MCP client**, each remote tool is wrapped as [`McpToolWrapper`](../../src/tools/mcp_tool.rs) with a **prefixed name** `<mcp_server_name>__<remote_tool_name>` (double underscore). There is no fixed list in-repo; it depends on configured MCP servers and their `tools/list` responses. Use **`tool_search`** when deferred registration is enabled, or inspect the agent tool list at runtime.

### Skill-defined tools (dynamic names)

Skills can declare `[[tools]]` in `SKILL.toml`. Callable names are typically **`{skill_name}.{tool_name}`** (see [`SkillShellTool` / `SkillHttpTool`](../../src/tools/skill_tool.rs)). Conflicts with built-in names are skipped with a warning.

### Optional WASM plugin tools (`plugins-wasm` build)

With the **`plugins-wasm`** feature and `[plugins].enabled`, tools loaded from the plugin manifest use **names from each plugin manifest** (not enumerated here).

---

## MCP

### ZeroClaw as an MCP server (editors, IDEs)

Use **`zeroclaw mcp serve`** to expose a **curated allowlist** of ZeroClaw tools over MCP (`2024-11-05`). Full detail: [`mcp-serve.md`](../mcp-serve.md).

Summary:

- **Allowlist only:** `[mcp_serve].allowed_tools` and `--allow-tool`; if both empty, defaults are safe read-oriented tools (`memory_recall`, `file_read`).
- **Stricter preset:** unless `relax_tool_policy = true`, only read-oriented tools from the built-in safe list may be allowlisted.
- **HTTP:** non-loopback binds require `[mcp_serve].auth_token` and `Authorization: Bearer`.

### MCP tools consumed by the agent (client side)

External MCP servers attach tools with names you typically match via patterns (e.g. `mcp_vikunja_*`). Control which schemas reach the model per turn with **`[[agent.tool_filter_groups]]`** in [`config-reference.md`](../reference/api/config-reference.md).

---

## Skills

| Mechanism | Where |
|-----------|--------|
| CLI | `zeroclaw skills list`, `install`, `remove`, `audit` — see [`commands-reference.md`](../reference/cli/commands-reference.md) (`skills` section). |
| Config | `[skills]` and optional `skills_directory` for scoped loading — [`config-reference.md`](../reference/api/config-reference.md). |
| Manifests | `SKILL.toml`: `prompts` and `[[tools]]` are injected into the agent system prompt at runtime. |

`skills install` runs a **static security audit** (symlinks, script-like files, risky snippets, unsafe markdown links). Use `skills audit` to validate before install.

---

## Permissions, policy, and approval

| Mechanism | Role |
|-----------|------|
| **`[autonomy]`** | `level` (`read_only` / `supervised` / `full`), workspace and path rules, `allowed_commands`, `require_approval_for_medium_risk`, `block_high_risk_commands`, **`auto_approve`**, **`always_ask`**, budgets. See [`config-reference.md`](../reference/api/config-reference.md) (section `[autonomy]`). |
| **`zeroclaw estop`** | Emergency stop levels including **tool freeze** for named tools — [`commands-reference.md`](../reference/cli/commands-reference.md). Requires `[security.estop].enabled` where applicable. |
| **`[mcp_serve]`** | Which tools exist over MCP; separate from in-process agent autonomy. |
| **Cron / shell** | Schedule payloads validated against security command policy — noted under `cron` in [`commands-reference.md`](../reference/cli/commands-reference.md). |
| **Google Workspace** | Fine-grained operation allowlists when using that integration — e.g. [`superpowers/specs/2026-03-19-google-workspace-operation-allowlist.md`](../superpowers/specs/2026-03-19-google-workspace-operation-allowlist.md). |

Broader security context: [`security/README.md`](../security/README.md).

---

## Related reference index

- Config keys (full schema: `zeroclaw config schema`): [`config-reference.md`](../reference/api/config-reference.md)
- CLI: [`commands-reference.md`](../reference/cli/commands-reference.md)
- MCP server: [`mcp-serve.md`](../mcp-serve.md)
- Reference catalogs hub: [`reference/README.md`](../reference/README.md)
