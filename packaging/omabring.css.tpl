/* OmaBring follows the active Omarchy theme.
   Omarchy renders this file into the theme directory as omabring.css.
   Placeholders are palette keys. accent_fg and font are filled by the app. */
window.omabring {
  --omabring-bg: {{ background }};
  --omabring-surface: {{ lighter_background }};
  --omabring-fg: {{ foreground }};
  --omabring-accent: {{ accent }};
  --omabring-accent-fg: {{ accent_fg }};
  --omabring-muted: {{ muted }};
  --omabring-sidebar-bg: {{ darker_background }};
  --omabring-card-bg: {{ lighter_background }};
  --omabring-selection: {{ selection }};
  --omabring-border: {{ border }};
  --omabring-line: {{ border }};
  --omabring-fill: {{ accent }};
  --omabring-on-fill: {{ accent_fg }};
  --omabring-danger: {{ red }};
  --omabring-ok: {{ green }};
  --omabring-warn: {{ yellow }};
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
  font-family: {{ font }};
  font-size: 14px;
  border-radius: 0;
}

window.omabring,
window.omabring * {
  font-family: {{ font }};
  border-radius: 0;
  box-shadow: none;
  outline: none;
}

window.omabring label,
window.omabring entry,
window.omabring text,
window.omabring textview {
  color: var(--omabring-fg);
}

window.omabring .bf-header {
  background-color: var(--omabring-bg);
  border-bottom: 1px dashed var(--omabring-line);
}

window.omabring .bf-mark {
  background-color: var(--omabring-fill);
  color: var(--omabring-on-fill);
  font-weight: 700;
  font-size: 13px;
  min-width: 34px;
  min-height: 34px;
}

window.omabring .bf-brand {
  font-size: 16px;
  font-weight: 700;
  letter-spacing: 0.3px;
}

window.omabring .bf-kicker {
  color: var(--omabring-muted);
  font-size: 10px;
  letter-spacing: 1.6px;
}

window.omabring .bf-field {
  color: var(--omabring-muted);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.8px;
}

window.omabring .bf-muted {
  color: var(--omabring-muted);
  font-size: 13px;
}

window.omabring .bf-h1 {
  font-size: 32px;
  font-weight: 700;
  letter-spacing: 0.4px;
}

window.omabring .bf-h1-sm {
  font-size: 20px;
  font-weight: 700;
  letter-spacing: 0.3px;
}

window.omabring .bf-go {
  color: var(--omabring-accent);
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.8px;
}

window.omabring .bf-link,
window.omabring .bf-link label {
  color: var(--omabring-accent);
}

window.omabring .bf-delta {
  font-size: 24px;
  font-weight: 700;
}

window.omabring .bf-ok {
  color: var(--omabring-fg);
  font-size: 12px;
  font-weight: 700;
}

window.omabring .bf-danger {
  color: var(--omabring-danger);
}

window.omabring button {
  background-color: var(--omabring-fill);
  color: var(--omabring-on-fill);
  border: 1px solid var(--omabring-fill);
  border-radius: 0;
  padding: 12px 16px;
  min-height: 0;
  font-weight: 700;
  box-shadow: none;
}

window.omabring button label {
  color: var(--omabring-on-fill);
  font-weight: 700;
}

window.omabring button.bf-ghost,
window.omabring button.bf-ghost label {
  background-color: transparent;
  color: var(--omabring-fg);
  border-color: var(--omabring-line);
}

window.omabring button.bf-chip,
window.omabring button.bf-chip label {
  background-color: transparent;
  color: var(--omabring-muted);
  border: 1px solid var(--omabring-line);
  padding: 4px 8px;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.4px;
  min-height: 0;
}

window.omabring button.bf-tab,
window.omabring button.bf-tab label {
  background-color: transparent;
  color: var(--omabring-muted);
  border: 1px solid transparent;
  padding: 7px 10px;
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0.8px;
  min-height: 0;
}

window.omabring button.bf-chip.on,
window.omabring button.bf-tab.on,
window.omabring button.bf-chip.on label,
window.omabring button.bf-tab.on label {
  background-color: var(--omabring-fill);
  color: var(--omabring-on-fill);
  border-color: var(--omabring-fill);
}

window.omabring button:focus-visible,
window.omabring entry:focus-visible,
window.omabring textview:focus-visible {
  outline: 1px solid var(--omabring-accent);
  outline-offset: 1px;
}

window.omabring button.bf-tab:focus,
window.omabring entry:focus {
  border-color: var(--omabring-accent);
}

window.omabring .bf-card {
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-line);
  padding: 18px 16px;
}

window.omabring .bf-dash {
  padding: 10px 12px 8px;
}

window.omabring .bf-filters {
  border-bottom: 1px solid var(--omabring-line);
  padding: 0 0 8px;
}

window.omabring .bf-theme {
  background-color: transparent;
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-line);
  font-size: 12px;
  font-weight: 700;
  padding: 7px 10px;
}

window.omabring .bf-dashed {
  border-style: dashed;
}

window.omabring .bf-pill {
  color: var(--omabring-muted);
  border: 1px solid var(--omabring-border);
  padding: 2px 6px;
  font-size: 11px;
  font-weight: 700;
}

window.omabring .bf-pill.on {
  background-color: var(--omabring-fill);
  color: var(--omabring-on-fill);
  border-color: var(--omabring-fill);
}

window.omabring .bf-cap {
  background-color: var(--omabring-fill);
  color: var(--omabring-on-fill);
  padding: 3px 8px;
  font-size: 11px;
  font-weight: 700;
}

window.omabring .bf-cap.off {
  background-color: transparent;
  color: var(--omabring-muted);
  border: 1px dashed var(--omabring-border);
}

window.omabring .bf-thumb,
window.omabring .bf-logo {
  font-size: 18px;
  font-weight: 700;
  background-color: var(--omabring-selection);
  color: var(--omabring-fg);
  min-width: 48px;
  min-height: 48px;
}

window.omabring .bf-logo {
  min-width: 64px;
  min-height: 64px;
  font-size: 22px;
}

window.omabring picture.bf-logo {
  background-color: transparent;
}

window.omabring .logo-carrefour { background-color: #004e9f; color: #ffffff; }
window.omabring .logo-union { background-color: #0b7a3e; color: #ffffff; }
window.omabring .logo-grandiose { background-color: #1a1a1a; color: #d4af37; }
window.omabring .logo-waitrose { background-color: #007a33; color: #ffffff; }
window.omabring .logo-spinneys { background-color: #8b1e3f; color: #ffffff; }
window.omabring .logo-mmi { background-color: #1a1a1a; color: #ffffff; }
window.omabring .logo-african { background-color: #b11226; color: #ffffff; }

window.omabring .bf-bar {
  background-color: transparent;
  min-height: 110px;
}

window.omabring .bf-bar-fill,
window.omabring .bf-donut {
  background-color: var(--omabring-accent);
  min-width: 8px;
}

window.omabring .bf-bar.on {
  border: 1px solid var(--omabring-accent);
  color: var(--omabring-accent);
}

window.omabring .bf-row {
  background-color: transparent;
  border-bottom: 1px dashed var(--omabring-line);
  padding: 8px 4px;
}

window.omabring .bf-row:hover,
window.omabring .bf-row:hover label {
  background-color: var(--omabring-selection);
  color: var(--omabring-fg);
}

window.omabring .bf-thead {
  border-bottom: 1px solid var(--omabring-line);
  padding: 10px 8px;
}

window.omabring .bf-thead label {
  color: var(--omabring-muted);
  font-size: 11px;
  font-weight: 700;
}

window.omabring button.bf-th,
window.omabring button.bf-th label {
  background-color: transparent;
  color: var(--omabring-muted);
  border: none;
  padding: 0;
  font-size: 11px;
  font-weight: 700;
  min-height: 0;
}

window.omabring button.bf-th.on,
window.omabring button.bf-th.on label {
  background-color: transparent;
  color: var(--omabring-fg);
  border: none;
}

window.omabring entry,
window.omabring passwordentry,
window.omabring searchentry,
window.omabring spinbutton {
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-line);
  border-radius: 0;
  box-shadow: none;
  min-height: 0;
  padding: 12px 13px;
}

window.omabring textview,
window.omabring textview text {
  background-color: var(--omabring-bg);
  color: var(--omabring-fg);
}

window.omabring scrolledwindow,
window.omabring viewport {
  background-color: var(--omabring-bg);
  border: none;
}

window.omabring selection {
  background-color: var(--omabring-accent);
  color: var(--omabring-accent-fg);
}

window.omabring popover,
window.omabring popover contents {
  background-color: var(--omabring-surface);
  color: var(--omabring-fg);
  border: 1px solid var(--omabring-border);
  border-radius: 0;
}
