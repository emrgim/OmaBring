mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use omabring_core::{
    default_data_dir, home_dir, load_window_theme, App, OfferSource, StoreId, VERSION,
};
use omabring_stores::Registry;

/// GTK 4.22 resolves a missing icon by calling the same lookup again with
/// the name `image-missing`. When that icon is absent from the theme, the
/// call never returns and `gtk_widget_show` overflows the main thread.
/// A loose PNG in an extra search path is the unthemed fallback GTK checks
/// after the theme directories.
fn install_icon_fallback() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let dir = fallback_icon_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let png = dir.join("image-missing.png");
    if std::fs::write(&png, include_bytes!("../../../assets/image-missing.png")).is_err() {
        return;
    }
    let actions = dir.join("hicolor/scalable/actions");
    if std::fs::create_dir_all(&actions).is_ok() {
        let _ = std::fs::write(
            actions.join("omabring-agent-symbolic.svg"),
            include_bytes!(
                "../../../assets/icons/hicolor/scalable/actions/omabring-agent-symbolic.svg"
            ),
        );
    }
    gtk::IconTheme::for_display(&display).add_search_path(&dir);
}

/// IBM Plex Mono is the grocery face. The Omarchy monospace stays in the CSS
/// stack for machines where this file cannot be registered.
fn install_plex_font() {
    if std::env::var_os("FONTCONFIG_FILE").is_some() {
        return;
    }
    let dir = default_data_dir().join("fonts");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let regular = dir.join("IBMPlexMono-Regular.ttf");
    let bold = dir.join("IBMPlexMono-Bold.ttf");
    if std::fs::write(
        &regular,
        include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf"),
    )
    .is_err()
    {
        return;
    }
    let _ = std::fs::write(
        &bold,
        include_bytes!("../../../assets/fonts/IBMPlexMono-Bold.ttf"),
    );
    let conf = default_data_dir().join("fonts.conf");
    let xml = format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <include ignore_missing="yes">/etc/fonts/fonts.conf</include>
  <dir>{}</dir>
</fontconfig>
"#,
        dir.display()
    );
    if std::fs::write(&conf, xml).is_ok() {
        std::env::set_var("FONTCONFIG_FILE", &conf);
    }
}

fn fallback_icon_dir() -> PathBuf {
    let preferred = default_data_dir().join("icon-fallback");
    if std::fs::create_dir_all(&preferred).is_ok() {
        preferred
    } else {
        std::env::temp_dir().join("omabring-icon-fallback")
    }
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--version") {
        println!("omabring {VERSION}");
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == "--print-theme") {
        let (_palette, css) = load_window_theme(&home_dir());
        println!("{css}");
        return glib::ExitCode::SUCCESS;
    }
    if let Some(import) = flag_value(&args, "--import-bringfast") {
        let path = match import {
            Ok(path) => path,
            Err(()) => {
                eprintln!("omabring --import-bringfast needs a sqlite file or a plain SQL dump");
                return glib::ExitCode::from(2);
            }
        };
        let dry_run = args.iter().any(|arg| arg == "--dry-run");
        let store_accounts = args.iter().any(|arg| arg == "--with-store-accounts");
        let receipts_dir = match flag_value(&args, "--receipts-dir") {
            Some(Ok(dir)) => Some(dir),
            Some(Err(())) => {
                eprintln!("omabring --receipts-dir needs a directory");
                return glib::ExitCode::from(2);
            }
            None => None,
        };
        let fetch_images = !args.iter().any(|arg| arg == "--no-fetch-images")
            && std::env::var("OMABRING_FETCH_IMAGES").ok().as_deref() != Some("0");
        return import_bringfast(
            &path,
            dry_run,
            receipts_dir.as_deref(),
            store_accounts,
            fetch_images,
        );
    }
    // These commands print and exit before GTK starts, so an SSH session
    // with no display can smoke-test a local install.
    if let Some(query) = flag_value(&args, "--search") {
        let query = match query {
            Ok(query) => query,
            Err(()) => {
                eprintln!("omabring --search needs a product name");
                return glib::ExitCode::from(2);
            }
        };
        return print_search(&query);
    }
    if let Some(view) = flag_value(&args, "--view") {
        let view = match view {
            Ok(view) => view,
            Err(()) => {
                eprintln!("omabring --view needs a view name");
                return glib::ExitCode::from(2);
            }
        };
        return print_view(&view);
    }
    if let Some(query) = flag_value(&args, "--gui-search") {
        match query {
            Ok(query) => std::env::set_var("OMABRING_QUERY", query),
            Err(()) => {
                eprintln!("omabring --gui-search needs a product name");
                return glib::ExitCode::from(2);
            }
        }
    }
    if let Some(product) = flag_value(&args, "--product") {
        match product {
            Ok(name) => std::env::set_var("OMABRING_PRODUCT", name),
            Err(()) => {
                eprintln!("omabring --product needs a product name");
                return glib::ExitCode::from(2);
            }
        }
    }
    if let Some(page) = flag_value(&args, "--page") {
        match page {
            Ok(name) => std::env::set_var("OMABRING_PAGE", name),
            Err(()) => {
                eprintln!("omabring --page needs a tab name");
                return glib::ExitCode::from(2);
            }
        }
    }
    if let Some(chart) = flag_value(&args, "--chart") {
        match chart {
            Ok(name) => std::env::set_var("OMABRING_CHART", name),
            Err(()) => {
                eprintln!("omabring --chart needs a chart name");
                return glib::ExitCode::from(2);
            }
        }
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "wayland");
    }
    install_plex_font();
    let smoke = args.iter().any(|arg| arg == "--smoke");
    let app = adw::Application::builder()
        .application_id("im.mrg.OmaBring")
        .build();
    app.connect_activate(move |app| {
        install_icon_fallback();
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let core = match App::open(default_data_dir()) {
            Ok(core) => Arc::new(core),
            Err(err) => {
                let body = glib::markup_escape_text(&err.to_string());
                let dialog = adw::AlertDialog::new(
                    Some("OmaBring could not open its data"),
                    Some(body.as_str()),
                );
                dialog.add_response("close", "Close");
                dialog.connect_response(None, {
                    let app = app.clone();
                    move |_, _| app.quit()
                });
                let window = adw::ApplicationWindow::builder()
                    .application(app)
                    .title("OmaBring")
                    .default_width(480)
                    .default_height(240)
                    .build();
                window.present();
                dialog.present(Some(&window));
                return;
            }
        };
        let backend = Arc::new(Registry::live());
        ui::open_window(app, core, backend);
        if smoke {
            let app = app.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
                app.quit();
                glib::ControlFlow::Break
            });
        }
    });
    app.run_with_args(&["omabring"])
}

/// `--search milk` or `--search=milk`. Absent flag is `None`. A bare flag is an error.
fn flag_value(args: &[String], name: &str) -> Option<Result<String, ()>> {
    let mut args = args.iter().skip(1);
    let prefix = format!("{name}=");
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix(&prefix) {
            return Some(if value.is_empty() {
                Err(())
            } else {
                Ok(value.to_string())
            });
        }
        if arg == name {
            return Some(match args.next() {
                Some(value) if !value.starts_with('-') && !value.is_empty() => Ok(value.clone()),
                _ => Err(()),
            });
        }
    }
    None
}

fn canonical_view(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "search" => Some("search"),
        "dashboard" => Some("dashboard"),
        "purchases" | "receipts" => Some("purchases"),
        "stores" | "carts" | "store" => Some("stores"),
        "settings" | "service" => Some("settings"),
        "forecast" => Some("forecast"),
        "product" => Some("product"),
        _ => None,
    }
}

fn import_bringfast(
    path: &str,
    dry_run: bool,
    receipts_dir: Option<&str>,
    store_accounts: bool,
    fetch_images: bool,
) -> glib::ExitCode {
    let core = match App::open(default_data_dir()) {
        Ok(core) => core,
        Err(err) => {
            eprintln!("{err}");
            return glib::ExitCode::from(1);
        }
    };
    let options = omabring_core::BringfastImport {
        dry_run,
        receipts_dir: receipts_dir.map(std::path::PathBuf::from),
        store_accounts,
    };
    match core.import_bringfast(std::path::Path::new(path), &options) {
        Ok(report) => {
            print!("{report}");
            if fetch_images && !dry_run {
                backfill_images(
                    &core,
                    std::path::Path::new(path),
                    receipts_dir.map(std::path::Path::new),
                );
            }
            glib::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            glib::ExitCode::from(1)
        }
    }
}

/// Pictures from Bring Fast's cache and image URLs, then every catalog search.
fn backfill_images(core: &App, source: &std::path::Path, receipts_dir: Option<&std::path::Path>) {
    let missing = match core.products_missing_images() {
        Ok(rows) => rows,
        Err(err) => {
            eprintln!("image search skipped: {err}");
            return;
        }
    };
    if missing.is_empty() {
        return;
    }
    eprintln!("searching stores for {} product images", missing.len());
    let dirs = omabring_core::product_image_dirs(Some(source), receipts_dir);
    let mut urls = omabring_core::bringfast_image_index(source);
    if let Ok(stored) = core.stored_image_urls() {
        for (key, list) in stored {
            urls.entry(key).or_default().extend(list);
        }
    }
    let backend = Registry::live();
    let mut saved = 0usize;
    for (index, item) in missing.iter().enumerate() {
        let mut keys = vec![omabring_core::normalize_name(&item.name)];
        if !item.product_key.trim().is_empty() {
            keys.push(item.product_key.clone());
        }
        let mut item_urls = Vec::new();
        for key in &keys {
            if let Some(list) = urls.get(key) {
                item_urls.extend(list.clone());
            }
        }
        if core
            .adopt_product_image(&item.name, &keys, &item_urls, &dirs)
            .unwrap_or(false)
        {
            saved += 1;
        } else {
            for store in image_stores(item.store) {
                let url = match backend.product_image(store, &item.name) {
                    Ok(Some(url)) => url,
                    _ => continue,
                };
                if core.cache_remote_image(&item.name, &url).unwrap_or(false) {
                    saved += 1;
                    break;
                }
            }
        }
        if (index + 1) % 25 == 0 {
            eprintln!("images {}/{} saved {saved}", index + 1, missing.len());
        }
    }
    eprintln!("saved {saved} product images");
}

fn image_stores(primary: StoreId) -> Vec<StoreId> {
    let mut stores = Vec::new();
    if primary.search() {
        stores.push(primary);
    }
    if !stores.contains(&StoreId::Carrefour) {
        stores.push(StoreId::Carrefour);
    }
    for store in StoreId::all() {
        if store.search() && !stores.contains(&store) {
            stores.push(store);
        }
    }
    stores
}

fn print_search(query: &str) -> glib::ExitCode {
    let query = query.trim();
    if query.is_empty() {
        eprintln!("omabring --search needs a product name");
        return glib::ExitCode::from(2);
    }
    let core = match App::open(default_data_dir()) {
        Ok(core) => core,
        Err(err) => {
            eprintln!("{err}");
            return glib::ExitCode::from(1);
        }
    };
    let backend = Registry::live();
    match core.search(query, &backend) {
        Ok(comparison) => {
            for note in &comparison.notes {
                if note.count == 0 {
                    if let Some(detail) = &note.detail {
                        eprintln!("{}: {detail}", note.store.name());
                    }
                }
            }
            if comparison.offers.is_empty() {
                println!("no matching products");
            } else {
                for offer in comparison.offers {
                    let source = match offer.source {
                        OfferSource::Live => "live",
                        OfferSource::History => "history",
                    };
                    println!(
                        "{}\t{}\t{:.2} {} / {}\t{source}",
                        offer.store.name(),
                        offer.name,
                        offer.price,
                        offer.currency,
                        offer.unit
                    );
                }
            }
            glib::ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            glib::ExitCode::from(1)
        }
    }
}

fn print_view(name: &str) -> glib::ExitCode {
    let Some(name) = canonical_view(name) else {
        eprintln!("unknown view {name}");
        return glib::ExitCode::from(2);
    };
    let core = match App::open(default_data_dir()) {
        Ok(core) => core,
        Err(err) => {
            eprintln!("{err}");
            return glib::ExitCode::from(1);
        }
    };
    let receipts = match core.list_invoices() {
        Ok(invoices) => invoices.len(),
        Err(err) => {
            eprintln!("{err}");
            return glib::ExitCode::from(1);
        }
    };
    println!("view {name}");
    println!("stores {}", StoreId::all().len());
    println!("receipts {receipts}");
    glib::ExitCode::SUCCESS
}

#[cfg(test)]
mod cli_tests {
    use super::{canonical_view, flag_value, image_stores};
    use omabring_core::StoreId;

    #[test]
    fn reads_search_and_view_flags() {
        let args = vec!["omabring".into(), "--search".into(), "milk".into()];
        assert_eq!(flag_value(&args, "--search").unwrap().unwrap(), "milk");
        let args = vec!["omabring".into(), "--view=search".into()];
        assert_eq!(flag_value(&args, "--view").unwrap().unwrap(), "search");
        let args = vec!["omabring".into(), "--search".into()];
        assert!(flag_value(&args, "--search").unwrap().is_err());
        assert!(flag_value(&["omabring".into()], "--search").is_none());
        let args = vec![
            "omabring".into(),
            "--gui-search".into(),
            "milk".into(),
            "--dry-run".into(),
        ];
        assert_eq!(flag_value(&args, "--gui-search").unwrap().unwrap(), "milk");
        let args = vec!["omabring".into(), "--import-bringfast=bringfast.db".into()];
        assert_eq!(
            flag_value(&args, "--import-bringfast").unwrap().unwrap(),
            "bringfast.db"
        );
        let args = vec![
            "omabring".into(),
            "--product".into(),
            "Full Fat Milk".into(),
        ];
        assert_eq!(
            flag_value(&args, "--product").unwrap().unwrap(),
            "Full Fat Milk"
        );
    }

    #[test]
    fn view_names_cover_the_keyboard_slots() {
        assert_eq!(canonical_view("search"), Some("search"));
        assert_eq!(canonical_view("receipts"), Some("purchases"));
        assert_eq!(canonical_view("service"), Some("settings"));
        assert_eq!(canonical_view("carts"), Some("stores"));
        assert_eq!(canonical_view("forecast"), Some("forecast"));
        assert_eq!(canonical_view("nope"), None);
    }

    #[test]
    fn image_search_uses_every_catalog_after_the_selling_store() {
        let stores = image_stores(StoreId::Waitrose);
        assert_eq!(stores[0], StoreId::Waitrose);
        assert_eq!(stores[1], StoreId::Carrefour);
        assert!(stores.contains(&StoreId::Grandiose));
        assert!(stores.contains(&StoreId::UnionCoop));
        assert!(stores.contains(&StoreId::Spinneys));
        assert!(stores.contains(&StoreId::Mmi));
        assert!(stores.contains(&StoreId::AfricanEastern));
        assert!(stores.iter().all(|store| store.search()));
        assert!(!stores.contains(&StoreId::Careem));
        assert!(!stores.contains(&StoreId::Mcdonalds));
        let careem = image_stores(StoreId::Careem);
        assert_eq!(careem[0], StoreId::Carrefour);
        assert!(careem.len() > 2);
    }

    #[test]
    fn page_and_chart_flags_keep_their_values() {
        let args = vec!["omabring".into(), "--page".into(), "forecast".into()];
        assert_eq!(flag_value(&args, "--page").unwrap().unwrap(), "forecast");
        let args = vec!["omabring".into(), "--chart=qty-month".into()];
        assert_eq!(flag_value(&args, "--chart").unwrap().unwrap(), "qty-month");
        let args = vec!["omabring".into(), "--chart".into()];
        assert!(flag_value(&args, "--chart").unwrap().is_err());
    }
}
