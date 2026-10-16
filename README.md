# OmaBring

OmaBring is a native desktop app for [Omarchy](https://omarchy.org) (Arch Linux and Hyprland). It is a personal UAE grocery shopper on this machine: link store accounts, search and compare prices, keep carts, prepare checkout, import receipts, and see spending plus what to buy again.

The window is GTK 4 and libadwaita. It follows the active Omarchy theme and runs as a Wayland application. It opens on Dashboard. The header order is Dashboard, Purchases, Stores, Search, Forecast, Settings. Alt+1 through Alt+6, or Up and Down, move between those views. Enter activates the focused header tab. The range, store, search, and sort bar stays pinned above the scrolling content. The top-right button opens the selected Omarchy agent in a terminal with the local MCP address. A background service on this machine exposes the same features to an approved client over MCP, with OAuth.

## Features

- One grocery record for the person signed in to the machine. OmaBring does not ask for its own email or password.
- Store sign-ins for Carrefour, Union Coop, Grandiose, Spinneys, Waitrose, MMI, and African & Eastern. Usernames and passwords are encrypted with a machine key in the data directory (`0600`).
- Product search and price comparison across the seven store catalogs and imported receipts. Search does not need a store sign-in.
- A cart for every store, and checkout that prepares the cart. Payment stays on the store website.
- Receipt import from JSON or CSV, with spending totals for this month, last month, and the last 7, 30, or 365 days.
- A rebuy forecast from repeat purchases: next date, status, and a likelihood score. One-off buys and items quiet for more than 90 days are left off the list.

## Omarchy

The active theme is `~/.local/state/omarchy/current/theme`. `omarchy-theme-set` rebuilds that directory. `install.sh` installs `~/.config/omarchy/themed/omabring.css.tpl` and runs `omarchy-theme-set-templates`, which writes `omabring.css` into the theme directory. That file defines accent, accent foreground, selection, muted, the background variants, foreground, borders, and red, green, and yellow. OmaBring reads those roles. When `omabring.css` is missing or still contains `{{` placeholders, colors come from `colors.toml`, then `alacritty.toml`, `walker.css`, or the waybar stylesheet. `~/.config/omarchy/current/theme` is used only when the state theme directory is absent.

Selected tabs, active range buttons, primary buttons, focus rings, chart bars, and links use the theme accent, with text chosen so it stays readable on that accent. Row highlights use the selection color. Borders and rules use `lighter_background` when it differs from the background, and otherwise the muted color.

The window watches `~/.local/state/omarchy/current`. That directory stays in place when the theme directory is replaced, so a theme swap restyles the open window. The monospace face comes from `~/.config/fontconfig/fonts.conf` (the file Omarchy writes when the font changes), then from the terminal font config. The window is flat and compact, with no title-bar chrome, so it sits in a Hyprland tile. `OMABRING_THEME` can point at a theme directory or a color file. `OMABRING_NO_CSS=1` skips that stylesheet and the color-scheme update.

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

## Stores

Search calls each shop's own catalog. No sign-in is required to search.

| Store | Catalog |
| --- | --- |
| Carrefour | Constructor.io product search |
| Grandiose | Magento GraphQL `products` |
| Union Coop | Algolia config published by the storefront |
| Waitrose | Public search page |
| Spinneys | Public search page |
| MMI | Retter `searchProductsV2` |
| African & Eastern | Magento GraphQL `products` |

Sign-in uses the store's own login and keeps the session token encrypted with the other credentials. Grandiose and Union Coop can then receive the cart on checkout. Carrefour sign-in posts to the store login. Waitrose and Spinneys keep the cart on their websites. OmaBring does not open a browser or any address. Receipt history is still included in search results.

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

`omabring --version` prints the version. `omabring --print-theme` prints the CSS taken from the active Omarchy theme. `omabring --search milk` prints matching products as tab-separated lines (`store`, name, price, source) and does not open a window, so it can run over SSH. With no matches it prints `no matching products`. `omabring --gui-search milk` (or `OMABRING_QUERY=milk`) opens the window on Search with that query already run. `omabring --view search` prints `view search`, the store count, and the receipt count. View names are `search`, `dashboard`, `purchases` (`receipts`), `stores` (`carts`, `store`), `settings` (`service`), `forecast`, and `product`. `omabring --smoke` opens the window and closes it, for a display check. The window opens on Dashboard. `OMABRING_PAGE` or `omabring --page` selects a tab without clicking: `dashboard`, `purchases`, `stores`, `search`, `forecast`, or `settings` (also `receipts`, `carts`, `service`, `store`, `product`, `receipt`). `omabring --product "Full Fat Milk"` (or `OMABRING_PRODUCT`) opens that product page. `OMABRING_CHART` or `omabring --chart` opens it on `price`, `price-by-store`, `qty-month`, `spend-month`, `spend-year`, `store-compare`, or `cumulative`. A chart with no product opens the latest receipt line. `OMABRING_DEMO=1` loads sample receipts only when the receipt list is empty. The receipt import box stays empty until a receipt is pasted.

`omabring --import-bringfast path` copies a Bring Fast sqlite file or a plain SQL dump into this machine: receipts with their dates and lines, products, product images, aliases, carts, and the shopping list. Careem, McDonald's, Amazon.it, and Amazon.ae are receipt stores, so their invoices are kept. `--receipts-dir DIR` reads that folder the way Bring Fast stores `~/.bring-fast/receipts`: each `store/invoice.pdf` or `.html` is parsed, and a file whose receipt number is already stored is skipped. Image files in the folder are still copied, and image URLs in the database are downloaded. `--dry-run` prints the same counts and writes nothing. A second run updates dates and images on receipts that are already stored and does not duplicate them. Store sign-ins are imported only with `--with-store-accounts`, which reads `master.key` beside that database (or `BRINGFAST_MASTER_KEY`) and seals the passwords again with OmaBring's key. Products that still have no picture are filled from Bring Fast's `product-images` cache and stored image URLs, then from a catalog search of the selling store, Carrefour, and the other searchable stores. The image is saved on this machine. `--no-fetch-images` or `OMABRING_FETCH_IMAGES=0` skips that search.

## Layout

| Path | Role |
| --- | --- |
| `crates/omabring` | GTK 4 window |
| `crates/omabring-service` | Background MCP and OAuth service |
| `crates/omabring-core` | Encryption, carts, receipts, spending, forecasts, theme colors |
| `crates/omabring-stores` | Store adapter interface and the Bring Fast stores |
| `crates/omabring-mcp` | HTTP MCP and OAuth |
| `packaging/` | Launcher entry and systemd user unit |
| `install.sh` | Build and install onto the machine |
