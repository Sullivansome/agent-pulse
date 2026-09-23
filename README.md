# Agent Pulse

Claude Code, Codex (desktop and CLI), and the official xAI Grok CLI in one Island surface. Session status comes from local lifecycle hooks. No API key, server, transcript scanner, or terminal scraping is needed.

## Connect

Build and connect from this standalone repository (stable Rust with edition 2024 support required):

```sh
./scripts/agent-pulse-setup.sh
./scripts/package.sh
island-cli plugin install ./dist/com.sullivansome.agent-pulse
```

Alternatively, select **Connect agents** on first use, or **… → Connection settings** inside Agent Pulse. Setup adds hooks for all three tools, preserves unrelated settings/hooks, backs up changed config files, and copies a small native receiver into the plugin's persistent data directory. The receiver keeps recording while Island is closed; restarting Island reloads recent state.

After setup:

1. Restart Claude Code and Grok sessions so they load the new hooks.
2. In Codex desktop, open **Settings → Hooks**; in the CLI, open `/hooks`. Review the Agent Pulse commands and trust them. Setup does not change hook trust or approval policy. Restart if the source list has not refreshed.
3. Submit a prompt. The session appears with its CLI, project folder name, and latest state.

Existing sessions do not retroactively appear. An installed hook is not proof that a tool has loaded or trusted it. The first lifecycle signal confirms that it is reporting.

**Connection settings** shows setup and observed activity separately for Claude Code, Codex, and Grok. **Hooks installed** means the expected local hooks and executable receiver were found; it does not claim that the agent has loaded or trusted them. **Last signal just now / … ago** comes from that provider's recorded events. **No recorded activity** is neutral: start a task to check it. The view refreshes from actual configuration instead of relying on an old setup-success marker. Reinstallation is optional; **Repair setup** appears only when verified hook entries or the receiver are missing. It never changes hook trust.

Supported integration baseline verified on this machine: Claude Code 2.1.236, Codex CLI 0.153.4, Grok 1.0.34. Older Codex versions with only the legacy `notify` setting do not provide the complete lifecycle and need an upgrade. Grok here means xAI's official native CLI, not third-party packages also named `grok`.

## Use

- **Needs you:** permission request or a supported question tool. Appears before working sessions.
- **Working:** a prompt, tool call, or compaction is in progress. A failed tool call is still working because the agent can recover.
- **Turn ended:** the tool emitted a Stop event. This describes one turn, not completion of a larger goal; another Stop hook may continue the agent. For Codex, the next prompt reopens the turn; delayed tool or approval events from the completed turn cannot restore **Needs you**.
- **Turn failed / Interrupted:** explicit lifecycle reports where the CLI exposes them. Closed sessions are removed from the list.
- **No recent signal:** a working state has received no hook for 10 minutes. Requests awaiting human input remain waiting until a resolving lifecycle event or the 24-hour retention limit. A long silent model/tool call can also cause this; it is never presented as completed. Entries expire after 24 hours, with a maximum of 128 recent sessions.

The island stays compact until hovered. It expands after a short pointer dwell and collapses on leave; clicking never pins it open. Requests for input stay first in the list and show an amber ! beside the notch, without auto-expanding. The panel grows to a compact three-row viewport, with every session accessible by scrolling. The header stays fixed and a draggable scrollbar shows your position. Requests sort first; a new request returns the viewport to the top.

Closed sessions disappear from the list. `SessionEnd` removes a row immediately; an ordinary turn ending keeps the open session available. On macOS and Linux, hooks also record the agent process's PID, birth token and controlling terminal. A closed terminal or exited CLI is removed on the next refresh (normally within one second), even if its final hook never arrived. Another tab in the same terminal application stays listed. Reused PIDs cannot keep an old row alive, and process-inspection errors never imply closure. Existing records acquire this tracking on their next hook. Desktop sessions without a controlling terminal remain governed by their session hooks. Grok's registry also removes missing sessions after its 30-second registration grace period.

Codex permission requests are tracked independently from parallel tool activity. `PermissionRequest` runs before a reviewer decides, including in **Approve for me** mode. It shows a neutral **Checking permissions**, then plain **Working** when activity continues. Unmatched permission records do not add warnings, amber badges, or attention priority. Matching tool completion, interruption, turn end, or the next prompt resolves the internal record. Explicit question tools still show **Needs you**; asynchronous questions remain waiting after their immediate return.

The documented hook payload does not identify the approval reviewer or confirm a human prompt. Therefore manual Codex approvals cannot reliably be distinguished from automatic review by this integration. `permission_mode` and global config are not used to guess a task's reviewer. Codex itself remains the source for those prompts. See [hooks](https://learn.chatgpt.com/docs/hooks) and [automatic review](https://learn.chatgpt.com/docs/sandboxing/auto-review).

Codex has no separate approval-resolved hook. In the observed desktop unified-exec path, a command can finish without a `PostToolUse` signal when its running session is not polled. Agent Pulse therefore distinguishes observed activity from unresolved approval signals rather than claiming the agent is blocked. See the [Codex hook coverage](https://learn.chatgpt.com/docs/hooks#tool-coverage). Requests without matching metadata remain unconfirmed until a turn boundary; existing hooks cannot reconstruct an earlier missed request.

Codex turn-end signals clear pending requests even when concurrent hook processes reach the store out of order. After a known turn settles, later tool or approval signals for that same turn are ignored until a new prompt starts work.

Click a row (or Enter on the selected row) to open the session. Codex desktop opens the exact task through `codex://threads/<UUID>`. On macOS, new CLI hooks record their owning application and process-start identity; clicking returns to that application. For terminals without a session deep link, this activates the application, not a guaranteed tab. Older records without an origin offer **Copy resume command**. Wheel/trackpad scrolling moves the list without changing selection. Up/Down, Home/End and Page Up/Down select and reveal rows without opening; Tab cycles controls. **…** also offers session actions and connection settings. Escape/right-click collapses until you leave and hover again. Launching requires `shell:open-url`; PID reuse cannot activate an unrelated application.

Opening a session keeps the list in place. Repeated clicks share one pending launch; failures appear in its header and permit retry. The host's five-second launch limit reports an error without unloading Agent Pulse. A launch result confirms only the OS handoff, not that the destination window appeared. Moving the pointer away still collapses the island normally.

On macOS, terminal discovery follows parent IDs through system-owned login helpers (including Ghostty's). It requires the full process identity only for the application being activated, so a restricted intermediate helper does not make an open terminal appear unavailable.

Provider icons are bundled assets; see [asset provenance](assets/README.md). Expanded and collapsed surfaces use the same artwork.

Main sessions are monitored. Child hooks cannot finish their parent. Grok's Claude-compatible discovery is deduplicated. PermissionDenied resolves the tool request while the turn remains Working; only StopCancelled marks the turn Interrupted. Stop ends the foreground turn even when background tasks or future wakeups remain. Grok's small `active_sessions.json` registry supplies liveness only: after a 30-second registration grace period, a missing entry removes the closed session from the list. An unreadable registry does not change activity. No transcripts are scanned. Restarted/new sessions and the next lifecycle event refresh their status; historical state alone is not proof of a running process.

## Commands and removal

```sh
target/debug/island-plugin-agent-pulse status
target/debug/island-plugin-agent-pulse status --json
./scripts/agent-pulse-setup.sh --provider claude
./scripts/agent-pulse-setup.sh --remove
```

`setup --home PATH` supports isolated configuration tests. `--data-dir PATH` selects an isolated store for setup, hook receipt, status, and the plugin. Normal setup honors `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, and `GROK_HOME` when set. Default configuration paths:

| Tool | File |
| --- | --- |
| Claude Code | `~/.claude/settings.json` |
| Codex | `~/.codex/hooks.json` |
| Grok | `~/.grok/hooks/island-agent-pulse.json` |

Remove hooks **before** deleting the plugin data directory. Disabling the plugin stops the visual surface; use `setup --remove` to stop collecting new signals. Removal retains other hooks, backups, and recorded status. Local data lives in `Island/plugins/com.sullivansome.agent-pulse/data` under the platform application-data directory, alongside the host-provided plugin data. `sessions.json` stores only provider, session/turn identifiers, project basename, status, tool name, timestamps, pending request kinds and tool IDs/input SHA-256 digests (at most 16 per session), terminal agent PID/birth token/terminal identifier, and (on macOS) the owning application PID, process-start identity, bundle identifier, and name. Digests correlate Codex approvals that omit the tool-call ID without retaining the input itself. Hook bodies, prompts, tool arguments/results, complete workspace paths, terminal contents, and transcripts are not retained or sent anywhere. Hook responses are always `{}` and never grant permission, deny a tool, or continue a turn.

Native surface and installer support: macOS and Linux. Linux needs DejaVu Sans or Liberation Sans; the optional copy action uses `wl-copy` or `xclip`. Windows builds are compile-compatible but automatic hook setup and packaging are not enabled yet.

## Integration references

- [Claude Code hooks](https://code.claude.com/docs/en/hooks)
- [Codex hooks and trust](https://learn.chatgpt.com/docs/hooks)
- [Grok hooks and event envelopes](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md)

## Checks and previews

```sh
cargo test -p island-plugin-agent-pulse
cargo run -p island-plugin-agent-pulse -- export-previews market
```

Market images use labeled example sessions from the actual renderer. They are not captured private sessions. Native surfaces have the host's existing framebuffer accessibility limitation; `status` provides a text alternative.

## Repository and license

This independently buildable MIT repository was extracted from Island on 2026-09-23. Its API, SDK and design dependencies are pinned to the public [Island Plugin SDK](https://github.com/Sullivansome/island-plugin-sdk); no private app source or checkout is required. The package script only builds files. Hook setup is a separate explicit command that modifies your local agent configurations.

The overlay requires an installed compatible Island host. The Island application and management CLI are proprietary and are not bundled here. Hook receipt, setup, status commands, and automated tests build independently of the app. See [LICENSE](LICENSE) and [third-party asset notices](assets/README.md).
