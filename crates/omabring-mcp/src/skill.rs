pub const SKILL: &str = r#"# OmaBring

OmaBring is a multi-user grocery MCP and dashboard for Dubai supermarkets. Currency is AED. Spend, orders, and ranks come from imported invoices.

## Tools

- `omabring_whoami` — signed-in account, linked stores, version.
- `omabring_stores` — retailers, capability flags, saved logins.
- `omabring_search` — `retailer` plus `query`. Searchable stores: carrefour, grandiose, unioncoop, waitrose, spinneys, mmi, africaneastern.
- `omabring_compare` — quotes already stored for a `product_key`, with `vs_paid`.
- `omabring_cart` — `retailer`, `action` list/add/set/remove/clear. `item_id` is the cart line id. Add takes `name` or `product_id` and `qty`.
- `omabring_checkout` — Grandiose and Union Coop. Default `action=prepare`. Grandiose `action=place` with `payment_method=ccod` or `cashondelivery` records a placed order. No card number is accepted.
- Store tools `carrefour_search`, `carrefour_cart`, `grandiose_search`, `grandiose_cart`, `grandiose_checkout`, `unioncoop_search`, `unioncoop_cart`, `unioncoop_checkout`, `waitrose_search`, `spinneys_search`, `mmi_search`, `africaneastern_search` are the same operations with the retailer fixed.
- `omabring_spend` — `range` is `1w`, `1m`, `last_month`, `this_month`, or `all`. `grain` is `daily`, `weekly`, `monthly`, or `yearly`.
- `omabring_orders` — invoice history. `omabring_products` ranks by `spend`, `frequency`, or `unit_price`.
- `omabring_shopping_list` — likely buys. `likely` is 0–100. Thumbs down drops an item.
- `omabring_product` — one product by `query`.
- `omabring_import_invoice` — `retailer`, `invoice_no`, `items` (`name`, `qty`, `unit_price` or `line_total`). Optional `text` parses a receipt. Pass `source_ref` so a repeat import replaces the same invoice.
- `omabring_delete_invoice` — `invoice_id` or `retailer` plus `invoice_no`, never both.
- `omabring_vote` — `product_key` and `vote` `up` or `down`.

## X

`x_me`, `x_user_by_username` (default @ilTrumpista), `x_user_posts`, `x_mentions`, `x_search`, `x_post`. Requests are signed and not sent. Missing keys return `x_credentials_missing`. A read that needs the signed-in X user returns `x_user_context_required`.

## Delivery

When a checkout note is relevant: Leave with security. Do not ring, call, or leave at the door.

Account mode uses the bundled catalog and the saved cart. Live mode returns `live_unavailable` and the request that would have been sent.
"#;
