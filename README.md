# OmaBring

OmaBring is a Rust plugin for SuperOmi on Omarchy. It is a multi-user grocery dashboard and MCP server for Dubai supermarkets: Carrefour, Union Coop, Grandiose, Spinneys, Waitrose, MMI, and African + Eastern. Receipts from Careem and McDonald's can be imported too. Amazon.it and Amazon.ae are mapped for invoice domains and have no catalog.

## What it does

- Accounts with a session cookie and an MCP bearer token. The first sign-in creates the account.
- Encrypted supermarket logins (AES-256-GCM) under `~/.omabring`.
- Product search and cross-store price comparison, in AED.
- Carts for Carrefour, Grandiose, and Union Coop. Grandiose checkout can prepare a basket or place cash or card on delivery. Union Coop prepares only. Carrefour checkout stays on the store site.
- Invoice import from receipt text, purchase history, spend windows, and a buy-again shopping list.
- OAuth with dynamic client registration, PKCE, and refresh tokens for MCP connectors.
- An offline-capable dashboard (network-first, cached shell).
- An Omarchy bar widget that opens the dashboard.

## Layout

| Path | Role |
| --- | --- |
| `crates/omabring-core` | Accounts, invoices, spend, forecasts |
| `crates/omabring-stores` | Catalog, carts, checkout request shapes |
| `crates/omabring-oauth` | Metadata, redirects, PKCE, token responses |
| `crates/omabring-mcp` | JSON-RPC tools and the agent skill |
| `crates/omabring-web` | Dashboard and HTTP routes |
| `crates/omabring` | `omabring` binary |
| `manifest.json` | Omarchy plugin `emrgim.omabring` |
| `deploy/omabring.service` | systemd user unit |
| `deploy/install.sh` | Release build, install, enable |

## Setup

Rust 1.83 or newer.

```sh
cargo test
cargo run -p omabring
```

The server listens on `127.0.0.1:8877`. Open that URL, sign in, and copy the MCP token from Settings.

| Variable | Default | Meaning |
| --- | --- | --- |
| `OMABRING_DATA` | `~/.omabring` | Database, `master.key`, receipts |
| `OMABRING_HOST` | `127.0.0.1` | Bind address |
| `OMABRING_PORT` | `8877` | Bind port |
| `OMABRING_PUBLIC_URL` | request host | OAuth issuer |
| `OMABRING_STORE_MODE` | `account` | `account` uses the bundled catalog. `live` returns the request and does not send it |

Install on the laptop, without sudo:

```sh
sh deploy/install.sh
```

Omarchy:

```sh
omarchy plugin add https://github.com/emrgim/OmaBring.git --enable
```

Left click on the bar opens the dashboard. Right click opens the panel.

Commands: `omabring` (serve), `omabring doctor`, `omabring backup`.

## MCP

`POST /mcp` with `Authorization: Bearer <token>`. Unauthenticated calls return 401 and a protected-resource metadata URL. `GET /mcp` is 405. `DELETE /mcp` is 204.

Connectors can register at `POST /oauth/register` and use the authorization-code grant with PKCE. The built-in client id is `omabring`. Access and refresh tokens are the account MCP token and last 30 days.

Tool names and the delivery note are in the `omabring://skill` resource.

## Store mode

Account mode is the running product: a bundled UAE catalog, a saved cart, and local checkout records. A cart pill turns on after a successful cart read.

Live mode builds the Grandiose, Union Coop, Carrefour, MMI, Waitrose, Spinneys, and African + Eastern requests and returns `live_unavailable`. Nothing is sent. X tools (`x_me`, `x_user_by_username`, `x_user_posts`, `x_mentions`, `x_search`, `x_post`) are signed with OAuth 1.0a and are not sent (`x_outbound_disabled`, or `x_credentials_missing` / `x_user_context_required` when the keys are absent).

PDF receipt bytes are not extracted. Paste the receipt text, or pass structured `items` to `omabring_import_invoice`.

## License

MIT. Copyright (c) 2026 Emiliano Morgia.
