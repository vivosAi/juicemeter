# Build your own provider

If a service can't be added as a config recipe (see [providers.md](providers.md)), it needs a
little Rust. You don't have to write it yourself: copy the prompt below into a coding agent
(Claude Code, Codex, whatever you use), working in your own fork of juicemeter.

Providers that follow the principles in the README are welcome as pull requests. Providers
that read browser cookies, other apps' private data or another tool's tokens won't be merged,
but nothing stops you running them in your fork.

---

## The prompt

```text
I want to add a usage provider to juicemeter, a Rust workspace that shows AI subscription
usage (rate-limit windows and balances) in a menu bar app and a CLI.

The provider is for: <SERVICE NAME>
Where its usage comes from: <endpoint URL, or command, or file, and where the credential lives>
An example response: <paste one, with any secrets removed>

How juicemeter is built:
- Providers live in crates/juicemeter-core/src/providers/. Read claude.rs, codex.rs and
  deepseek.rs first; they are the reference implementations.
- Each provider implements the `Provider` trait in providers/mod.rs:
  - kind(): a stable lowercase id, e.g. "example"
  - name(): display name
  - source(): where the credentials live on this machine, for display and de-duplication
  - account(): who the credentials belong to, from LOCAL data only (no network). Build it
    with Account::new(kind, native_id) where native_id is the service's own account id or,
    for API keys, the key itself (it is hashed). Fill email or a masked key hint if known.
  - poll_interval(): optional, default 5 minutes
  - fetch(): returns a Snapshot { plan, windows, balances, source, as_of, note }
- A Window has a label, used_percent (0..100), and, when known, window_seconds and
  resets_at. Set both when you can: they power the pace flags (squeezed dry / drink up).
- Missing credentials return Error::NotConfigured(hint) with a one-line hint telling the user
  how to set them up. Other failures return anyhow errors with plain messages.
- Pass HTTP responses through providers::check(), which turns 401/403 into a clear message
  and 429 into RateLimited (honouring Retry-After).
- Register the provider: a Cargo feature in crates/juicemeter-core/Cargo.toml, the module in
  providers/mod.rs, kinds() and defaults() if it should run by default, and build() in
  config.rs so it can be added as a [[source]].
- Add unit tests that parse a saved example response. Never put real keys in tests.
- Run `cargo clippy --all-targets` and `cargo test`, then try it with
  `cargo run -p juicemeter-cli -- --local --fresh`.

juicemeter's rules (upstream only accepts providers that follow them):
1. Read-only: read credentials a tool already stores; never refresh, rotate or rewrite them,
   and never edit another tool's settings.
2. Zero tokens: only call usage, quota or balance endpoints. Never send a prompt.
3. No snooping: no browser cookies, no other apps' databases, no reading other processes.
4. Undocumented endpoints are fine, but say so in a comment, and fail with a clear message
   when they change.

Keep the code in the style of the existing providers: short doc comments, serde structs for
the response, no new dependencies unless really needed.
```
