# OmaBring

OmaBring is a native desktop app for [Omarchy](https://omarchy.org) (Arch Linux and Hyprland). It is a personal UAE grocery shopper on this machine: link store accounts, search and compare prices, keep carts, prepare checkout, import receipts, and see spending plus what to buy again.

The window is GTK 4 and libadwaita. It follows the active Omarchy theme and runs as a Wayland application. Dashboard, Purchases, Stores, and Settings sit under one header: spend bars, a product table, store cards, a cart, and the local service address. A background service on this machine exposes the same features to an approved client over MCP, with OAuth.

## Features

- One grocery record for the person signed in to the machine. OmaBring does not ask for its own email or password.
- Store sign-ins for Carrefour, Union Coop, Grandiose, Spinneys, Waitrose, MMI, and African & Eastern. Usernames and passwords are encrypted with a machine key in the data directory (`0600`).
- Product search and price comparison across linked stores and imported receipts.
- A cart for every store, and checkout that prepares the cart. Payment stays on the store website.
- Receipt import from JSON or CSV, with spending totals for this month, last month, and the last 7, 30, or 365 days.
- A rebuy forecast from repeat purchases: next date, status, and a likelihood score. One-off buys and items quiet for more than 90 days are left off the list.

## Omarchy

The app reads the active theme from `~/.config/omarchy/current/theme`, then from `~/.local/state/omarchy/current/theme`. Colors come from `colors.toml` when it is present, and otherwise from `alacritty.toml`, `walker.css`, or the waybar stylesheet in that directory. The monospace face comes from `~/.config/fontconfig/fonts.conf` (the file Omarchy writes when the font changes), then from the terminal font config. Those colors become the window background, foreground, accent, borders, and selection. The window is flat and compact, with no title-bar chrome, so it sits in a Hyprland tile. `OMABRING_THEME` can point at a theme directory or a color file. `OMABRING_NO_CSS=1` skips that stylesheet and the color-scheme update.

The theme directory is watched. Switching theme with `omarchy-theme-set`, or changing the font config, restyles the open window.

`install.sh` builds the release binaries, installs a launcher entry so OmaBring shows up in the Omarchy app launcher, and installs a systemd user service. OmaBring does not install a bar plugin. Open it from the app launcher.

The window registers a small `image-missing` icon before it is shown. GTK 4.22 otherwise repeats that lookup until the main thread overflows when the active icon theme does not provide it.

## Install

Runtime libraries on Arch: `gtk4` and `libadwaita`. Building also needs those development files, a C compiler, and Rust.

```sh
./install.sh
```

The app binary is `~/.local/bin/omabring`. The service binary is `~/.local/bin/omabring-service`. Data lives in `~/.local/share/omabring` (override with `OMABRING_DATA_DIR`). The service listens on `127.0.0.1:8787` (override with `OMABRING_BIND`).

The window asks before an MCP client may use this machine's data. Allow or deny that request in the OmaBring window. The service does not open a browser page.

## Local service and OAuth

The service speaks MCP at `POST /mcp` and OAuth on the same address.

- `GET /.well-known/oauth-authorization-server`
- `GET /.well-known/oauth-protected-resource`
- `POST /oauth/register` — dynamic client registration
- `GET /oauth/authorize` — authorization code with PKCE (`S256` only). The request waits until OmaBring allows or denies it.
- `POST /oauth/token` — `authorization_code` and `refresh_token`. Refresh tokens rotate.
- `POST /oauth/revoke`

Redirect addresses must be `https`, `omabring://`, or loopback `http`. Public clients use `token_endpoint_auth_method=none` together with PKCE. Confidential clients can use `client_secret_post` or `client_secret_basic`.

MCP tools: `whoami`, `list_stores`, `link_store`, `unlink_store`, `search`, `compare`, `cart_list`, `cart_add`, `cart_update`, `cart_remove`, `cart_clear`, `checkout`, `import_invoice`, `list_invoices`, `spend`, `forecast`.

## Store contract

Each store is an adapter behind one interface: search, and checkout when that store can take a remote cart.

Live calls run only when the saved account has a contract address (`base_url`). That address must be `https`, or loopback `http`. The adapter then calls:

| Call | Request | Success body |
| --- | --- | --- |
| Search | `GET {base}/omabring/search?q=` | `{"products":[{"id","name","price","currency","unit"}]}` |
| Checkout | `POST {base}/omabring/checkout` with `{"store","lines"}` | `{"status":"ready","url","reference"}` |

`status` may also be `prepared` or `accepted`.

Carrefour, Union Coop, and Grandiose can push that checkout. Spinneys, Waitrose, MMI, and African & Eastern keep the cart locally and do not submit a remote cart. OmaBring does not open a browser or any address. MMI and African & Eastern are search-and-list stores in OmaBring.

If a contract address is missing, or the endpoint does not answer in that shape, search still uses imported receipts and the local cart still works. The public storefronts are not assumed to implement this contract. Point `base_url` at an endpoint that does when you have one.

Default websites, used when checkout stays on the store:

- Carrefour — https://www.carrefouruae.com/mafuae/en
- Union Coop — https://www.unioncoop.ae/
- Grandiose — https://www.grandiose.ae/
- Spinneys — https://www.spinneys.com/en-ae/
- Waitrose — https://www.waitrose.ae/en/
- MMI — https://www.mmihomedelivery.ae/
- African & Eastern — https://www.africanandeastern.com/

## Receipt formats

JSON:

```json
{
  "store": "carrefour",
  "invoice_no": "1001",
  "date": "2026-03-01",
  "currency": "AED",
  "items": [
    {"name": "Milk", "qty": 1, "unit": "piece", "unit_price": 5.25}
  ]
}
```

A document may also be `{"invoices":[ ... ]}` or a JSON array. Importing the same store and `invoice_no` again replaces that receipt.

CSV header: `store,invoice_no,date,currency,name,qty,unit,unit_price`.

Store names accept the labels above (`union coop`, `african & eastern`, and the snake-case ids).

## Develop

```sh
cargo test --workspace
cargo build --release
```

`omabring --version` prints the version. `omabring --print-theme` prints the CSS taken from the active Omarchy theme. `omabring --smoke` opens the window and closes it, for a display check. `OMABRING_PAGE` opens `dashboard`, `purchases`, `stores`, `settings`, `store`, or `product`. `OMABRING_DEMO=1` loads sample receipts only when the receipt list is empty.

## Layout

| Path | Role |
| --- | --- |
| `crates/omabring` | GTK 4 window |
| `crates/omabring-service` | Background MCP and OAuth service |
| `crates/omabring-core` | Encryption, carts, receipts, spending, forecasts, theme colors |
| `crates/omabring-stores` | Store adapter interface and the seven stores |
| `crates/omabring-mcp` | HTTP MCP and OAuth |
| `packaging/` | Launcher entry and systemd user unit |
| `install.sh` | Build and install onto the machine |
