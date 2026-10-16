mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use omabring_core::{
    default_data_dir, home_dir, load_active_palette, App, OfferSource, StoreId, VERSION,
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
    gtk::IconTheme::for_display(&display).add_search_path(&dir);
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
        let palette = load_active_palette(&home_dir());
        println!("{}", palette.to_css());
        return glib::ExitCode::SUCCESS;
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
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "wayland");
    }
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
    use super::{canonical_view, flag_value};

    #[test]
    fn reads_search_and_view_flags() {
        let args = vec!["omabring".into(), "--search".into(), "milk".into()];
        assert_eq!(flag_value(&args, "--search").unwrap().unwrap(), "milk");
        let args = vec!["omabring".into(), "--view=search".into()];
        assert_eq!(flag_value(&args, "--view").unwrap().unwrap(), "search");
        let args = vec!["omabring".into(), "--search".into()];
        assert!(flag_value(&args, "--search").unwrap().is_err());
        assert!(flag_value(&["omabring".into()], "--search").is_none());
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
}
