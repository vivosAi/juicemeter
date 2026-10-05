# juicemeter

How much juice is left in your AI subscriptions. Plan limits, reset times and API
balances for Claude, Codex and DeepSeek, from every machine you use, in one place.

I built it after one too many Fridays of realising I'd barely touched my main subscription
that week, while a second account on another machine had run dry on Tuesday.

```
$ juicemeter
Claude (max) · you@example.com  live · 21:25
  Session          ████████████████████  99% left  refills in 3h43m
  Weekly           ████████████████░░░░  80% left  refills in 4d8h
  on laptop, mini

Codex (plus) · Work ChatGPT  live · 21:25
  5-hour           ████████████████████ 100% left  refills in 4h59m
  Weekly           ██████████████░░░░░░  69% left  refills in 5d2h
  on mini

DeepSeek · sk-…a1b2  live · 21:25
  Balance          $4.97
  on laptop
```

It reads the logins your tools already have (Claude Code, the Codex CLI) and whatever API
keys you point it at. Checking usage never sends a prompt, so it costs nothing.

## How it works

There are three pieces. `juicemeter-agent` runs on each machine in the background, checks
that machine's logins every few minutes and serves the numbers over your
[Tailscale](https://tailscale.com) network. `juicemeter` is the command line tool, and
`juicemeter-app` is the menu bar app (macOS for now). Both read every agent and show one
entry per account.

Agents share usage numbers, never credentials, so a key stays on the machine that has it.
An account logged in on two machines shows up once. A server can run just the agent; a
laptop can run just the app.

## Principles

These decide what juicemeter does, and what it won't do even when it would be convenient.

1. **Read-only.** It reads logins your tools already store. It never refreshes, rotates or
   rewrites them, and never edits another tool's settings. It can't log you out of anything.
2. **Zero tokens.** It only asks for usage, quota and balance numbers. It never sends a prompt.
3. **Credentials stay home.** Agents share numbers over Tailscale, never keys or tokens.
4. **No snooping.** No browser cookies, no other apps' databases, no reading other processes,
   no Full Disk Access.
5. **Opt-in beyond the basics.** Extra providers are off until you add them, and anything that
   relies on an undocumented endpoint says so.

Want something these rules rule out? Build it in your fork: [docs/build-your-own-provider.md](docs/build-your-own-provider.md)
has a ready-made prompt for your coding agent.

## Install

**macOS and Linux**, on every machine you use:

```sh
curl -fsSL https://raw.githubusercontent.com/vivosAi/juicemeter/main/install.sh | sh
```

That puts `juicemeter` and `juicemeter-agent` in `~/.local/bin` and runs `juicemeter setup`,
which checks Tailscale, starts the agent, deals with the macOS firewall (asking before anything
that needs your password) and finds your other machines. Run `juicemeter setup` again any time;
`juicemeter setup --check` only reports.

**The menu bar app** (macOS `.dmg`, Linux `.deb`/`.AppImage`, Windows installer) is on the
[releases page](https://github.com/vivosAi/juicemeter/releases). Install it on the machines you
look from.

The macOS app isn't signed with an Apple Developer certificate, so the first time you open it
macOS says it can't check it for malware. Open it once, then go to System Settings → Privacy &
Security and click **Open Anyway**. After that it opens normally. Or clear the download flag
yourself: `xattr -dr com.apple.quarantine /Applications/juicemeter.app`.

**Windows**: download the zip from the releases page and run `juicemeter-agent run`.
Starting it at login isn't automated on Windows yet.

### From source

Needs a Rust toolchain ([rustup](https://rustup.rs)).

```sh
git clone https://github.com/vivosAi/juicemeter.git && cd juicemeter
cargo install --path crates/juicemeter-cli
cargo install --path crates/juicemeter-agent
juicemeter setup
```

To update: `git pull`, the two `cargo install` lines, then `juicemeter-agent install` to restart
the agent.

## Usage

```sh
juicemeter                    # all machines, one entry per account
juicemeter --local            # this machine only
juicemeter --by-host          # one section per machine
juicemeter --used             # show used instead of left (or set `show` in the config)
juicemeter --only claude,codex
juicemeter --json             # for scripts, status bars, widgets

juicemeter accounts           # every account, its id, and which machines have it
juicemeter label codex:3f9a1c2b4d5e "Work ChatGPT"
juicemeter paths              # where the config and env file live
```

Entries are sorted by urgency and flagged when they need attention:

- **▼ SQUEEZED DRY** means under 20% left, or more than half used and burning fast enough to
  hit the limit before it refills.
- **▲ DRINK UP** means a weekly allowance is on pace to leave 40% or more unused, or a perk
  like a free Codex reset expires within three days. Five-hour sessions never get this flag.
  Leaving those idle is normal.

The `│` on each bar is the clock: how much of the window's time is left (or has passed,
with `--used`). Juice past the mark means you're behind pace. Short of it, you're ahead.

When an agent runs on the same machine, `juicemeter` reads its cached report, so you can
run it as often as you like. `--fresh` asks the providers directly instead.

## Menu bar app

Build `juicemeter.app` and put it in Applications (needs the Tauri CLI once:
`cargo install tauri-cli --version '^2' --locked`):

```sh
cd crates/juicemeter-app && cargo tauri build --bundles app
cp -R ../../target/release/bundle/macos/juicemeter.app /Applications/
open /Applications/juicemeter.app
```

It lives only in the menu bar (no Dock icon). Turn on **Start at login** in its settings.
For a quick try without bundling: `cargo run --release -p juicemeter-app`.

The menu bar shows the accounts you star in the panel. Any number works, including none,
and until you pick, it shows your Claude account. Anything squeezed dry or about to go to
waste gets added in front, or marked in place if it's already starred. Nothing starred and
nothing flagged leaves just the juice box, filled to your lowest limit:

```
Claude 79% ↻4d · Codex 92% ↻3d               starred, nothing urgent
Codex ▼8% ↻40m · Claude 79% ↻4d               Codex squeezed dry soon, in front of the stars
Claude ▲70% ↻1d                               starred Claude about to go to waste
Codex 89% ↻5d/5h 4% ↻53m                      the 5-hour session is nearly used up
Codex 89% ↻5d, reset exp3h                    a free reset expires in 3 hours
```

Each item is the account's general weekly window (per-model limits like Claude's
"Weekly · Fable" stay in the panel): what's left, then `↻` and when it refills. The five-hour session joins after a `/`
only once it's down to 10%, and leaves again above 20%, so it doesn't flicker. Settings can
show only the percentage or only the time, and short names (`Cl`, `Co-A`).

The ⚙ in the panel opens settings: name your accounts, pick what the menu bar shows, and
add or remove other machines. The same choices are in the right-click menu and the config
file (`mode`):

| Mode         | Menu bar                                              |
|--------------|-------------------------------------------------------|
| `watch`      | Pinned accounts, plus any alert in front (default)    |
| `lowest`     | Whatever has the least left                           |
| `use_it`     | The biggest allowance about to go unused              |
| `minimal`    | Just the juice box                                    |
| `everything` | A short number for every account                      |

The app reads the agents once a minute. With an agent running, it never calls the
providers itself.

## Providers

| Provider | What it shows                             | Where it reads credentials                         |
|----------|-------------------------------------------|----------------------------------------------------|
| Claude   | Session and weekly limits (Pro/Max)       | Claude Code's login (macOS Keychain or `~/.claude`) |
| Codex    | 5-hour and weekly limits, credits, free resets | Codex CLI's login (`~/.codex/auth.json`), or Codex itself via `codex app-server` |
| DeepSeek | API balance                               | An API key (see below)                              |
| Antigravity | Weekly quota for Gemini and Claude models | Antigravity's login in the macOS Keychain       |

Antigravity keeps its login fresh only while it's running, so open it now and then; when the
stored login has expired, juicemeter keeps the last numbers it saw and says so.

Claude, Codex and Antigravity usage comes from the same endpoints their own apps use. Those aren't
documented, so expect them to break now and then. juicemeter only reads the logins and
never refreshes them, which means it can't log you out. If a login expires, open the tool
once and it renews itself.

Anything else that answers with JSON, an HTTP endpoint or a command, can be added in the
config without code. [docs/providers.md](docs/providers.md) explains how and has recipes for
Vercel AI Gateway, Ollama, GitHub Copilot and MiniMax (untested so far).

### API keys

Keys are looked up in this order:

1. An environment variable, e.g. `DEEPSEEK_API_KEY`
2. The env file at `~/.config/juicemeter/juicemeter.env` (`DEEPSEEK_API_KEY=sk-…`)
3. The macOS Keychain: `security add-generic-password -s juicemeter -a deepseek -w`

## Configuration

`~/.config/juicemeter/config.toml`. Every setting is optional.

```toml
hosts = ["my-mac-mini"]     # other machines running juicemeter-agent
show = "remaining"          # or "used"
mode = "watch"              # what the menu bar shows (see above)
bar_value = "both"          # each menu bar item: "both" (79% (4d)), "percent" or "time"
bar_names = "full"          # or "short": "Cl 79%" (labels of 4 characters or fewer stay)
pins = ["claude:1a2b3c4d5e6f"]  # menu bar accounts when nothing needs attention ([] = none)
share_email = true          # false: agents don't include account emails
auto_detect = true          # false: only use the sources listed below

[intervals]                 # how often agents check (defaults: claude 10m, codex 5m, deepseek 30m)
claude = "15m"

[[source]]                  # a second Codex login on this machine
provider = "codex"
home = "~/.codex-work"      # its CODEX_HOME
label = "Work ChatGPT"

[[source]]                  # a key another tool keeps
provider = "deepseek"
key = { env_file = "~/.someagent/.env", var = "DEEPSEEK_API_KEY" }

[labels]                    # names for accounts, by id (see `juicemeter accounts`)
"claude:1a2b3c4d5e6f" = "Personal Claude"
```

A key can also live in a JSON file (`{ json_file = "…", pointer = "/path/to/key" }`), a
Keychain item (`{ keychain = "service", account = "name" }`) or come from a command such as
a password manager (`{ command = "op read op://Private/DeepSeek/credential" }`).

### Keys held by other tools

Other agent tools keep their own keys. Instead of finding them by hand:

1. Run `juicemeter setup-prompt` and paste the prompt into that tool's agent.
2. Save its answer and run `juicemeter import answer.txt`.

The answer only says *where* each key lives, never the key itself. `import` checks every
source before adding it and asks before writing the config.

## The agent

`juicemeter-agent` listens on port 47878, on `127.0.0.1` and the machine's Tailscale
address. Nothing on your LAN or the internet can reach it. Who can is up to your Tailscale
ACLs.

| Endpoint            | |
|---------------------|-|
| `GET /v1/report`    | Latest report (JSON) |
| `POST /v1/refresh`  | Check everything now (at most every 30 seconds) |
| `GET /healthz`      | `ok` |

When a check fails, the agent keeps the last good numbers, with their age, and waits
longer before trying again. It saves the last report to disk, so a restart doesn't come
back empty.
Logs go to `~/Library/Logs/juicemeter-agent.log` on macOS and `journalctl --user -u
juicemeter-agent` on Linux. `juicemeter-agent uninstall` removes the service.

## Common issues

### Serving and reading machines

Think of each machine as serving, reading, or both:

- **Serving** machines run `juicemeter-agent`. Do this on every machine that has subscriptions
  you want counted. A server or an always-on box can run only the agent.
- **Reading** machines are the ones you look from: the menu bar app or the `juicemeter` CLI.
  A reading machine only sees the machines listed in its own `hosts` (settings, Other
  machines). Nothing is shared automatically, so "I only see this machine" usually means that
  list is empty.

If you want every machine to read every other one, each needs the others in its `hosts`, and
each serving machine must accept connections (next section). That's the setup where people
most often get stuck.

### "Connection reset by peer" or "Empty reply from server"

The other machine is reachable but its macOS firewall is refusing the agent. The agent runs in
the background, so macOS never gets to ask you. On the **serving** machine:

```sh
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add ~/.cargo/bin/juicemeter-agent
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp ~/.cargo/bin/juicemeter-agent
```

The rule allows that one program, on any network. What keeps it private is where the agent
listens: `127.0.0.1` and the machine's Tailscale address, nothing else, so there's no way in
from your Wi-Fi or the internet. Check with `lsof -nP -iTCP:47878 -sTCP:LISTEN`. Updating the
agent replaces the file, so you may need to run the two commands again afterwards.

### Who on my tailnet can read it

Every device on your tailnet can reach a serving machine's agent. It hands out usage numbers
and account emails, never credentials (`share_email = false` drops the emails). To limit it to
the machines you read from, add a [Tailscale ACL](https://tailscale.com/kb/1018/acls) that only
allows port 47878 from those devices.

### Codex shows old or missing data

If several copies of Codex are installed (Homebrew, npm, nvm), the agent uses the newest one it
finds, since it doesn't run with your shell's PATH. Point it at a specific one with
`JUICEMETER_CODEX_BIN`. Old copies are harmless but can be confusing in a terminal:
`which -a codex` lists them.

### Claude says rate limited (HTTP 429)

Claude's usage endpoint is strict. The agent waits as long as Claude asks before trying again,
and keeps showing the last numbers meanwhile. Running `juicemeter --fresh` a lot, or polling
the same account from many machines, makes it more likely.

### Antigravity says the login expired

Antigravity only renews its login while it's open. Open it for a moment and the next check
picks it up.

## Adding a provider

Providers live in `crates/juicemeter-core/src/providers/`. Implement the `Provider` trait:
where the credentials live (`source`), whose they are (`account`, from local data only),
and a `fetch` that returns a `Snapshot` of windows and balances. Two house rules: read
credentials but never refresh them, and never spend tokens to check usage.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
