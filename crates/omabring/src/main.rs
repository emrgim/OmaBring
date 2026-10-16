mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use omabring_core::{default_data_dir, home_dir, load_active_palette, App, VERSION};
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
                let dialog = adw::AlertDialog::new(
                    Some("OmaBring could not open its data"),
                    Some(&err.to_string()),
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
