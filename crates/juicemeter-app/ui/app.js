const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let view = null;

function human(ms) {
  const m = Math.max(0, Math.round(ms / 60000));
  const d = Math.floor(m / 1440), h = Math.floor((m % 1440) / 60), mm = m % 60;
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${String(mm).padStart(2, "0")}m`;
  return `${mm}m`;
}

const SYMBOLS = { USD: "$", EUR: "€", GBP: "£", CNY: "¥", JPY: "¥" };
function money(b) {
  const sym = SYMBOLS[b.currency];
  return sym ? `${sym}${b.amount.toFixed(2)}` : `${b.amount.toFixed(2)} ${b.currency}`;
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
}

function windowRow(w, show, now) {
  const row = el("div", "win");
  row.append(el("span", "label", w.label));

  const reset = w.resets_at ? new Date(w.resets_at) : null;
  const passed = reset && reset <= now;
  const shown = show === "used" ? w.used : w.left;
  const track = el("div", "track");
  const fill = el("div", "fill");
  fill.style.width = `${shown}%`;
  const tone = w.state === "running_out" && w.left < 10 ? "out" : w.state === "unused" ? "unused" : "";
  if (tone) fill.classList.add(tone);
  if (passed) fill.classList.add("stale");
  track.append(fill);
  // Mark the matching share of time: fill beyond it means behind pace, short of it ahead.
  if (w.elapsed != null && !passed) {
    const tick = el("div", "tick");
    tick.style.left = `${(show === "used" ? w.elapsed : 1 - w.elapsed) * 100}%`;
    track.append(tick);
  }
  row.append(track, el("span", "pct", `${Math.round(shown)}% ${show === "used" ? "used" : "left"}`));

  let status = "", cls = "status";
  if (passed) status = "window has reset since";
  else if (w.state === "running_out" && w.runs_out_at) {
    status = `squeezed dry in ~${human(new Date(w.runs_out_at) - now)}`;
    if (reset) status += ` · refills in ${human(reset - now)}`;
    cls += " out";
  } else if (w.state === "unused") {
    status = `~${Math.round(w.projected_left ?? w.left)}% goes to waste`;
    if (reset) status += ` · refills in ${human(reset - now)}`;
    cls += " unused";
  } else if (w.state === "running_out" && reset) {
    status = `refills in ${human(reset - now)}`;
    cls += " out";
  } else if (reset) status = `refills in ${human(reset - now)}`;
  if (status) row.append(el("span", cls, status));
  return row;
}

function card(e, show, now) {
  const c = el("section", `card ${e.state}`);
  if (e.status === "not_configured") c.classList.add("muted");

  const head = el("div", "head");
  if (e.state === "running_out") head.append(el("span", "badge running_out", "SQUEEZED DRY"));
  if (e.state === "unused") head.append(el("span", "badge unused", "DRINK UP"));
  head.append(el("span", "name", e.name));
  if (e.plan) head.append(el("span", "plan", e.plan));
  if (e.status === "ok") {
    const pin = el("button", `pin${e.pinned ? " on" : ""}`, "★");
    pin.title = e.pinned ? "In the menu bar — click to remove" : "Show in the menu bar";
    pin.setAttribute("aria-label", pin.title);
    pin.onclick = () => invoke("toggle_pin", { key: e.key });
    head.append(pin);
  }
  c.append(head, el("div", "who", e.who));

  for (const w of e.windows) c.append(windowRow(w, show, now));
  for (const b of e.balances) {
    const row = el("div", "balance");
    row.append(el("span", "", b.label), el("span", "", money(b)));
    c.append(row);
  }
  for (const p of e.perks || []) {
    if (!p.count) continue;
    const row = el("div", "balance perk");
    const exp = p.expires_at ? new Date(p.expires_at) : null;
    const soon = exp && exp - now < 3 * 86400000;
    row.append(el("span", "", p.label), el("span", soon ? "soon" : "", `${p.count}${exp ? ` · next expires in ${human(exp - now)}` : ""}`));
    c.append(row);
  }
  if (e.message) c.append(el("div", `msg${e.status === "error" ? " error" : ""}`, e.message));
  if (e.from_log && e.as_of) c.append(el("div", "msg", `from Codex's log, ${human(now - new Date(e.as_of))} old`));
  if (e.machines.length) c.append(el("div", "machines-line", `on ${e.machines.join(", ")}`));
  return c;
}

function render() {
  if (!view) return;
  const now = new Date();
  const list = document.getElementById("entries");
  list.replaceChildren(...(view.entries.length
    ? view.entries.map((e) => card(e, view.show, now))
    : [el("p", "empty", "No accounts found yet.")]));

  for (const b of document.querySelectorAll(".seg button[data-show]")) b.classList.toggle("on", b.dataset.show === view.show);
  for (const b of document.querySelectorAll(".seg button[data-key]")) b.classList.toggle("on", view[b.dataset.key] === b.dataset.value);

  document.getElementById("machines").replaceChildren(...view.machines.map((m) => {
    const s = el("span", `machine${m.error ? " off" : ""}`, m.name);
    if (m.error) s.title = m.error;
    return s;
  }));
  const t = new Date(view.updated_at);
  document.getElementById("updated").textContent =
    `updated ${t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

const MODES = [
  ["watch", "Watch", "starred accounts, alerts in front"],
  ["lowest", "Lowest", "whatever has the least left"],
  ["use_it", "Use it", "the biggest allowance about to go to waste"],
  ["minimal", "Minimal", "just the juice box"],
  ["everything", "Everything", "a short number for every account"],
];

function showError(msg) {
  const p = document.getElementById("settings-error");
  p.hidden = !msg;
  p.textContent = msg || "";
}

async function call(cmd, args) {
  try { await invoke(cmd, args); showError(""); } catch (e) { showError(String(e)); }
}

function renderSettings() {
  if (!view || document.body.classList.contains("in-settings") === false) return;
  // Don't rebuild under the user's cursor while they type a name.
  if (document.activeElement && document.activeElement.tagName === "INPUT") return;

  const accounts = view.entries.filter((e) => !e.key.includes("@"));
  document.getElementById("set-accounts").replaceChildren(...accounts.map((e) => {
    const box = el("div", "acct");
    const top = el("div", "acct-top");
    const star = el("button", `pin${e.pinned ? " on" : ""}`, "★");
    star.setAttribute("aria-label", e.pinned ? `Remove ${e.name} from the menu bar` : `Show ${e.name} in the menu bar`);
    star.onclick = () => call("toggle_pin", { key: e.key });
    const name = el("input");
    name.value = e.label || "";
    name.placeholder = e.name;
    name.setAttribute("aria-label", `Name for ${e.name} ${e.who}`);
    const save = () => { if ((e.label || "") !== name.value.trim()) call("set_label", { key: e.key, label: name.value }); };
    name.onchange = save;
    name.onkeydown = (ev) => { if (ev.key === "Enter") name.blur(); };
    top.append(star, name);
    const meta = [e.name, e.plan, e.who, e.machines.join(", ")].filter(Boolean).join(" · ");
    box.append(top, el("div", "meta", meta));
    return box;
  }));

  document.getElementById("set-modes").replaceChildren(...MODES.map(([key, title, desc]) => {
    const label = el("label");
    const radio = el("input");
    radio.type = "radio";
    radio.name = "mode";
    radio.checked = view.mode === key;
    radio.onchange = () => call("set_mode", { mode: key });
    const text = el("span");
    text.append(el("b", "", title), document.createTextNode(` — ${desc}`));
    label.append(radio, text);
    return label;
  }));

  const status = Object.fromEntries(view.machines.map((m) => [m.name, m.error]));
  document.getElementById("set-hosts").replaceChildren(...(view.hosts.length ? view.hosts.map((h) => {
    const row = el("div", "host");
    const name = el("span", `machine${status[h] ? " off" : ""}`, h);
    if (status[h]) name.title = status[h];
    const rm = el("button", "", "remove");
    rm.setAttribute("aria-label", `Remove ${h}`);
    rm.onclick = () => call("set_host", { name: h, present: false });
    row.append(name, rm);
    return row;
  }) : [el("p", "hint", "None yet.")]));
}

function openSettings(open) {
  document.body.classList.toggle("in-settings", open);
  document.getElementById("settings").hidden = !open;
  showError("");
  if (open) {
    renderSettings();
    invoke("get_autostart").then((on) => { document.getElementById("autostart").checked = on; });
  }
}
document.getElementById("autostart").onchange = (ev) => call("set_autostart", { enabled: ev.target.checked });
document.getElementById("open-settings").onclick = () => openSettings(true);
document.getElementById("close-settings").onclick = () => openSettings(false);
document.getElementById("discover").onclick = async (ev) => {
  const btn = ev.currentTarget;
  const box = document.getElementById("found");
  btn.disabled = true;
  btn.textContent = "Looking…";
  showError("");
  try {
    const found = await invoke("discover");
    box.replaceChildren(...found.map((f) => {
      const row = el("div", "host");
      const name = el("span", `machine${f.agent ? "" : " off"}`, f.address);
      name.append(el("span", "os", f.os));
      row.append(name);
      if (view && view.hosts.includes(f.address)) row.append(el("span", "muted", "reading"));
      else if (f.agent) {
        const add = el("button", "add", "add");
        add.setAttribute("aria-label", `Read ${f.address}`);
        add.onclick = () => call("set_host", { name: f.address, present: true }).then(() => row.replaceChildren(name, el("span", "muted", "reading")));
        row.append(add);
      } else row.append(el("span", "muted", f.online ? "no agent" : "offline"));
      return row;
    }));
    if (!found.length) box.replaceChildren(el("p", "hint", "No other devices on your tailnet."));
  } catch (e) {
    showError(String(e));
  } finally {
    btn.disabled = false;
    btn.textContent = "Find machines on my tailnet";
  }
};

document.getElementById("add-host").onsubmit = (ev) => {
  ev.preventDefault();
  const input = document.getElementById("new-host");
  const name = input.value.trim();
  if (!name) return;
  input.value = "";
  input.blur();
  call("set_host", { name, present: true });
};

document.querySelectorAll(".seg button[data-key]").forEach((b) => {
  b.onclick = () => call("set_bar", { key: b.dataset.key, value: b.dataset.value });
});
document.querySelectorAll(".seg button[data-show]").forEach((b) => {
  b.onclick = () => invoke("set_show", { show: b.dataset.show });
});
document.getElementById("refresh").onclick = async (ev) => {
  const btn = ev.currentTarget;
  btn.classList.add("spin");
  try { await invoke("refresh"); } finally { btn.classList.remove("spin"); }
};
document.getElementById("quit").onclick = () => invoke("quit");

listen("view", (e) => { view = e.payload; render(); renderSettings(); });
invoke("get_view").then((v) => { if (v) { view = v; render(); } });
// Keep countdowns current between updates.
setInterval(render, 30000);
