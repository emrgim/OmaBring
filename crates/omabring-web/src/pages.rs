use omabring_core::db::StoreAccount;
use omabring_core::purchases::{ProductRow, SpendReport};

pub fn shell(title: &str, body: &str, signed_in: bool) -> String {
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="theme-color" content="#1f3d32">
<title>{title} · OmaBring</title>
<link rel="manifest" href="/manifest.webmanifest">
<link rel="icon" href="/static/icon.svg">
<style>
:root {{
  --ink: #1c1915;
  --paper: #f4efe6;
  --card: #fffaf3;
  --line: #e2d8c8;
  --green: #1f3d32;
  --leaf: #3e6b4f;
  --gold: #b5812b;
  --warn: #8c3a2f;
}}
* {{ box-sizing: border-box; }}
body {{
  margin: 0;
  background: var(--paper);
  color: var(--ink);
  font: 16px/1.45 "Iowan Old Style", Palatino, "Palatino Linotype", Georgia, serif;
}}
a {{ color: var(--green); }}
header {{
  display: flex; align-items: center; justify-content: space-between;
  padding: 16px 22px; background: var(--green); color: #f6f1e7;
}}
header a {{ color: inherit; text-decoration: none; }}
header nav a {{ margin-left: 16px; font-family: ui-sans-serif, system-ui, sans-serif; font-size: 14px; }}
main {{ max-width: 980px; margin: 0 auto; padding: 22px; }}
h1 {{ font-weight: 500; letter-spacing: -0.02em; margin: 0 0 8px; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 14px; }}
.card {{ background: var(--card); border: 1px solid var(--line); border-radius: 14px; padding: 16px; }}
.stat {{ font-size: 32px; font-variant-numeric: tabular-nums; }}
.muted {{ color: #6d6458; font-family: ui-sans-serif, system-ui, sans-serif; font-size: 13px; }}
label {{ display: block; margin: 10px 0 4px; font-family: ui-sans-serif, system-ui, sans-serif; font-size: 13px; }}
input, select, button {{
  font: 15px/1.2 ui-sans-serif, system-ui, sans-serif;
  padding: 10px 12px; border-radius: 10px; border: 1px solid var(--line); background: white; color: var(--ink);
}}
button, .btn {{ background: var(--green); color: #f6f1e7; border: 0; cursor: pointer; text-decoration: none; display: inline-block; }}
.row {{ display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }}
table {{ width: 100%; border-collapse: collapse; }}
td, th {{ text-align: left; padding: 8px 6px; border-bottom: 1px solid var(--line); vertical-align: top; }}
.pill {{ display: inline-block; border-radius: 999px; padding: 2px 8px; background: #efe6d6; font-family: ui-sans-serif, system-ui, sans-serif; font-size: 12px; margin-right: 4px; }}
.pill.on {{ background: #d7eadc; color: var(--green); }}
.mark {{
  width: 36px; height: 36px; border-radius: 10px; display: inline-grid; place-items: center;
  color: white; font-family: ui-sans-serif, system-ui, sans-serif; font-weight: 700;
}}
form.inline {{ display: flex; gap: 8px; flex-wrap: wrap; align-items: end; }}
#offline {{ display: none; background: var(--gold); color: white; padding: 8px 22px; font-family: ui-sans-serif, system-ui, sans-serif; }}
</style>
</head>
<body>
<div id="offline">Offline. Showing the last cached page.</div>
<header>
  <a href="/"><strong>OmaBring</strong></a>
  <nav>
    <a href="/">Home</a>
    <a href="/purchases">Purchases</a>
    <a href="/stores">Stores</a>
    <a href="/settings">Settings</a>
    {auth}
  </nav>
</header>
<main>
{body}
</main>
<script>
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js');
function ping() {{
  var bar = document.getElementById('offline');
  if (!navigator.onLine) {{ bar.style.display = 'block'; return; }}
  fetch('/health').then(function() {{ bar.style.display = 'none'; }}).catch(function() {{ bar.style.display = 'block'; }});
}}
ping();
window.addEventListener('online', ping);
window.addEventListener('offline', ping);
</script>
</body>
</html>"##,
        title = esc(title),
        body = body,
        auth = if signed_in {
            r#"<a href="/logout">Sign out</a>"#
        } else {
            ""
        }
    )
}

pub fn login_form(error: &str, next: &str) -> String {
    format!(
        r#"<h1>Sign in</h1>
<p class="muted">One form. A new email creates the account.</p>
{error}
<form method="post" action="/login">
  <input type="hidden" name="next" value="{next}">
  <label>Email</label><input name="email" type="email" required autofocus>
  <label>Password</label><input name="password" type="password" minlength="6" required>
  <p><button type="submit">Continue</button></p>
</form>"#,
        error = if error.is_empty() { String::new() } else { format!(r#"<p style="color:#8c3a2f">{error}</p>"#, error = esc(error)) },
        next = esc(next),
    )
}

pub fn dashboard(email: &str, spend: &SpendReport, list: &[ProductRow], stores: &[StoreAccount]) -> String {
    let items = if list.is_empty() {
        "<p class=\"muted\">No staples are due. Import a few receipts and the list fills in.</p>".into()
    } else {
        let rows = list
            .iter()
            .map(|p| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&p.name),
                    esc(&p.status),
                    p.likely,
                    esc(&p.likely_vote)
                )
            })
            .collect::<String>();
        format!("<table><tr><th>Item</th><th>Status</th><th>Likely</th><th>Vote</th></tr>{rows}</table>")
    };
    let pills = stores
        .iter()
        .filter(|s| s.linked || s.enabled)
        .map(|s| {
            let cart_on = s.capabilities.iter().any(|c| c.key == "cart" && c.on);
            format!(
                r#"<span class="pill {}">{}</span>"#,
                if cart_on { "on" } else { "" },
                esc(&s.retailer.name)
            )
        })
        .collect::<String>();
    format!(
        r#"<h1>Good day</h1>
<p class="muted">{email}</p>
<div class="grid">
  <section class="card"><div class="muted">This month</div><div class="stat">{total:.2} <span class="muted">AED</span></div><div class="muted">average {avg:.2} ÷ {n}</div></section>
  <section class="card"><div class="muted">Stores</div><p>{pills}</p><a class="btn" href="/stores">Manage logins</a></section>
</div>
<section class="card" style="margin-top:14px">
  <h2>To buy</h2>
  {items}
</section>"#,
        email = esc(email),
        total = spend.total,
        avg = spend.average,
        n = spend.period_count,
        pills = pills,
        items = items,
    )
}

pub fn purchases(rows: &[serde_json::Value], offset: i64) -> String {
    let body = if rows.is_empty() {
        "<p class=\"muted\">No invoices yet.</p>".into()
    } else {
        rows.iter()
            .map(|r| {
                let raw = r["store_name"].as_str().unwrap_or("");
                let name = omabring_core::retailers::retailer(raw).map(|s| s.name).unwrap_or(raw);
                let letter = name.chars().next().unwrap_or('?').to_string();
                format!(
                    "<tr><td><span class=\"mark\" style=\"background:#1f3d32\">{}</span> {}</td><td>{}</td><td>{}</td><td>{:.2}</td></tr>",
                    esc(&letter),
                    esc(name),
                    esc(r["invoice_date"].as_str().unwrap_or("")),
                    esc(r["invoice_no"].as_str().unwrap_or("")),
                    r["total"].as_f64().unwrap_or(0.0),
                )
            })
            .collect::<String>()
    };
    let next = if rows.len() == 24 {
        format!(r#"<p><a class="btn" href="/purchases?offset={}">Next shelf</a></p>"#, offset + 24)
    } else {
        String::new()
    };
    format!(
        r#"<h1>Purchases</h1>
<p class="muted">Shelf of 24. Photos are letter tiles.</p>
<table><tr><th>Store</th><th>Date</th><th>Invoice</th><th>AED</th></tr>{body}</table>
{next}"#
    )
}

pub fn stores_page(stores: &[StoreAccount], csrf: &str, notice: &str) -> String {
    let cards = stores
        .iter()
        .map(|s| {
            let caps = s
                .capabilities
                .iter()
                .map(|c| format!(r#"<span class="pill {}">{}</span>"#, if c.on { "on" } else { "" }, esc(&c.label)))
                .collect::<String>();
            format!(
                r#"<section class="card">
  <h2><span class="mark" style="background:{color}">{letter}</span> {name}</h2>
  <p>{caps}</p>
  <p class="muted">{email}</p>
  <form method="post" action="/stores/{id}">
    <input type="hidden" name="csrf" value="{csrf}">
    <label>Email</label><input name="email" value="{login}">
    <label>Password</label><input name="password" type="password" placeholder="{pw}">
    <label>Delivery address</label><input name="address" value="{addr}">
    <p class="row"><button type="submit">Save</button></p>
  </form>
  <form method="post" action="/stores/{id}/clear"><input type="hidden" name="csrf" value="{csrf}"><button type="submit">Unlink</button></form>
</section>"#,
                color = esc(&s.retailer.color),
                letter = esc(&s.retailer.name.chars().next().unwrap_or('?').to_string()),
                name = esc(&s.retailer.name),
                caps = caps,
                email = if s.linked { "Login saved" } else { "Not linked" },
                id = esc(&s.retailer.id),
                csrf = esc(csrf),
                login = esc(s.login_email.as_deref().unwrap_or("")),
                pw = if s.has_password { "saved" } else { "" },
                addr = esc(&s.delivery_address),
            )
        })
        .collect::<String>();
    format!(
        r#"<h1>Stores</h1>
{notice}
<p class="muted">Passwords stay encrypted on this machine. Cart pills turn on after a successful cart read.</p>
<div class="grid">{cards}</div>"#,
        notice = if notice.is_empty() { String::new() } else { format!("<p>{}</p>", esc(notice)) }
    )
}

pub fn settings(email: &str, token: &str, backup_path: &str, backup_status: &str, csrf: &str, public_url: &str) -> String {
    format!(
        r#"<h1>Settings</h1>
<section class="card">
  <p>{email}</p>
  <p class="muted">MCP token</p>
  <code>{token}</code>
  <p class="muted">Connectors use {public_url}/mcp</p>
</section>
<section class="card" style="margin-top:14px">
  <h2>Backup</h2>
  <p class="muted">Status {status}</p>
  <form method="post" action="/backup">
    <input type="hidden" name="csrf" value="{csrf}">
    <label>Folder</label><input name="path" value="{path}" placeholder="/home/me/Backups">
    <p class="row"><button type="submit" name="op" value="save">Save path</button><button type="submit" name="op" value="run">Copy database</button></p>
  </form>
</section>
<section class="card" style="margin-top:14px">
  <h2>Password</h2>
  <form method="post" action="/settings/password">
    <input type="hidden" name="csrf" value="{csrf}">
    <label>New password</label><input name="password" type="password" minlength="6" required>
    <p><button type="submit">Update</button></p>
  </form>
  <p><a href="/forgot">Forgot password</a></p>
</section>"#,
        email = esc(email),
        token = esc(token),
        public_url = esc(public_url),
        status = esc(backup_status),
        csrf = esc(csrf),
        path = esc(backup_path),
    )
}

pub fn forgot(notice: &str) -> String {
    format!(
        r#"<h1>Reset password</h1>
{notice}
<form method="post" action="/forgot">
  <label>Email</label><input name="email" type="email" required>
  <p><button type="submit">Create reset link</button></p>
</form>"#,
        notice = if notice.is_empty() { String::new() } else { format!("<p>{}</p>", notice) }
    )
}

pub fn reset(token: &str, error: &str) -> String {
    format!(
        r#"<h1>Choose a new password</h1>
{error}
<form method="post" action="/reset">
  <input type="hidden" name="token" value="{token}">
  <label>New password</label><input name="password" type="password" minlength="6" required>
  <p><button type="submit">Save</button></p>
</form>"#,
        token = esc(token),
        error = if error.is_empty() { String::new() } else { format!("<p>{}</p>", esc(error)) }
    )
}

pub fn esc(raw: &str) -> String {
    raw.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub const MANIFEST: &str = r##"{
  "name": "OmaBring",
  "short_name": "OmaBring",
  "start_url": "/",
  "display": "standalone",
  "background_color": "#f4efe6",
  "theme_color": "#1f3d32",
  "icons": [{ "src": "/static/icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }]
}"##;

pub const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
<rect width="64" height="64" rx="14" fill="#1f3d32"/>
<text x="32" y="42" text-anchor="middle" font-family="Georgia, serif" font-size="32" fill="#f4efe6">O</text>
</svg>"##;

pub const OFFLINE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>OmaBring offline</title></head>
<body style="font-family:Georgia,serif;background:#f4efe6;padding:32px"><h1>OmaBring is offline</h1>
<p>The last cached page is unavailable. Reconnect and open the dashboard again.</p></body></html>"#;

pub const SW: &str = r#"const CACHE = "omabring-shell-v1";
const SHELL = ["/", "/offline.html", "/manifest.webmanifest", "/static/icon.svg"];
self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)));
  self.skipWaiting();
});
self.addEventListener("activate", (event) => {
  event.waitUntil(caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k)))));
  self.clients.claim();
});
self.addEventListener("fetch", (event) => {
  if (event.request.method !== "GET") return;
  event.respondWith(
    fetch(event.request).then((response) => {
      const copy = response.clone();
      caches.open(CACHE).then((cache) => cache.put(event.request, copy));
      return response;
    }).catch(() => caches.match(event.request).then((hit) => hit || caches.match("/offline.html")))
  );
});
self.addEventListener("message", (event) => {
  if (event.data === "clear") caches.delete(CACHE);
});
"#;
