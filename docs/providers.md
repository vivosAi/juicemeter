# More providers

juicemeter ships with Claude, Codex and DeepSeek. Anything else that answers an HTTP request
with JSON, or has a command that prints JSON, can be added in
`~/.config/juicemeter/config.toml` without writing code.

## How a recipe works

```toml
[[source]]
provider = "http"                      # or "command"
name = "Some Service"                  # what the panel calls it
url = "https://api.example.com/usage"
key = { env = "EXAMPLE_API_KEY" }      # where the key lives; any key location works
auth = { header = "X-API-Key" }        # optional; the default is "Authorization: Bearer"
post = '{"scope": "all"}'              # optional; sends a POST with this body
plan = "/plan/name"                    # optional pointer to the plan name

window = [
  { label = "Weekly", used = "/limits/weekly/percent", resets_at = "/limits/weekly/reset", length = "7d" },
]
balance = [
  { label = "Credits", amount = "/credits/remaining", currency = "USD" },
]
```

The paths are [JSON pointers](https://datatracker.ietf.org/doc/html/rfc6901): `/a/b/0/c` means
"field `a`, then `b`, then the first item, then `c`".

For each window, say what the number means:

| Field | |
|---|---|
| `used` or `left` | Pointer to the share used, or left |
| `fraction = true` | The value is 0 to 1, not a percentage |
| `of` | Pointer to a total: `used`/`left` are counts out of it |
| `resets_at` | Pointer to the reset time: RFC 3339, a `YYYY-MM-DD` date, or Unix seconds or milliseconds |
| `resets_in` | Pointer to the seconds until reset, instead |
| `length` | How long the window is (`5h`, `7d`). Without it there's no pace, so no squeezed dry or drink up from pace |

For balances, `currency` is a code like `"USD"`, or a pointer if it starts with `/`.

A `command` source runs its command and reads stdout the same way. With no `window` or
`balance` lines, the command must print juicemeter's own format:

```json
{"plan": "pro",
 "windows": [{"label": "Weekly", "used_percent": 31, "resets_at": "2026-10-06T04:00:00Z", "window_seconds": 604800}],
 "balances": [{"label": "Credits", "amount": 12.5, "currency": "USD"}]}
```

Command sources and POST sources can only be written by hand. `juicemeter import` refuses
them, so a pasted answer from an agent can never make juicemeter run something.

Check a new source with `juicemeter --local --fresh` before relying on it.

## Recipes

**Tested** means someone ran it against a real account. **Untested** recipes are written from
the service's responses as other open source tools document them. They may need adjusting.
If you get one working, a pull request marking it tested is very welcome.

### DeepSeek, as a generic source (tested)

Built in already. Shown here because it's the simplest working example.

```toml
[[source]]
provider = "http"
name = "DeepSeek"
url = "https://api.deepseek.com/user/balance"
key = { env = "DEEPSEEK_API_KEY" }
balance = [{ label = "Balance", amount = "/balance_infos/0/total_balance", currency = "/balance_infos/0/currency" }]
```

### Vercel AI Gateway (untested)

```toml
[[source]]
provider = "http"
name = "Vercel AI Gateway"
url = "https://ai-gateway.vercel.sh/v1/credits"
key = { env = "AI_GATEWAY_API_KEY" }
balance = [{ label = "Credits", amount = "/balance", currency = "USD" }]
```

### Ollama Cloud (untested)

Usage comes as a 0 to 1 fraction, with no reset times, so there's no pace.

```toml
[[source]]
provider = "http"
name = "Ollama"
url = "https://ollama.com/api/usage"
key = { env = "OLLAMA_API_KEY" }
window = [
  { label = "Session", used = "/limits/session/usage", fraction = true },
  { label = "Weekly", used = "/limits/weekly/usage", fraction = true },
  { label = "Monthly", used = "/limits/monthly/usage", fraction = true },
]
```

### GitHub Copilot (untested)

Uses an undocumented GitHub endpoint and a classic personal access token with the
`copilot` scope.

```toml
[[source]]
provider = "http"
name = "Copilot"
url = "https://api.github.com/copilot_internal/user"
key = { env = "GITHUB_COPILOT_TOKEN" }
plan = "/copilot_plan"
window = [
  { label = "Premium requests", left = "/quota_snapshots/premium_interactions/percent_remaining", resets_at = "/quota_reset_date" },
]
```

### MiniMax coding plan (untested)

The field called `current_interval_usage_count` is what's left, not what's used.
Use `api.minimaxi.com` for accounts in mainland China.

```toml
[[source]]
provider = "http"
name = "MiniMax"
url = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains"
key = { env = "MINIMAX_API_KEY" }
window = [
  { label = "Plan", left = "/model_remains/0/current_interval_usage_count", of = "/model_remains/0/current_interval_total_count", resets_at = "/model_remains/0/end_time" },
]
```

## Needs real code

Some services can't be described with pointers alone. Z.ai, for example, returns its windows
as a list tagged with numeric codes. These need a small provider in
`crates/juicemeter-core/src/providers/`, and [build-your-own-provider.md](build-your-own-provider.md)
walks through writing one.

## What juicemeter won't ship

Some usage data is only reachable by reading browser cookies, another app's private
database, or a running process's memory or arguments, or by refreshing another tool's login.
juicemeter doesn't do any of that (see the principles in the README). Those integrations
aren't accepted upstream. You're free to build them in your own fork, and the prompt in
[build-your-own-provider.md](build-your-own-provider.md) is a good starting point.
