#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib::{self, ControlFlow};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use omabring_core::{
    home_dir, load_active_palette, App, CartLine, Offer, Session, SpendRange, StoreId, StoreSecrets,
};
use omabring_stores::Registry;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SavedSession {
    token: String,
}

struct Shell {
    core: Arc<App>,
    backend: Arc<Registry>,
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    user_id: i64,
    token: String,
    search_entry: gtk::SearchEntry,
    search_summary: gtk::Label,
    search_list: gtk::ListBox,
    cart_store: gtk::ComboBoxText,
    cart_name: gtk::Entry,
    cart_qty: gtk::SpinButton,
    cart_price: gtk::Entry,
    cart_list: gtk::ListBox,
    checkout_label: gtk::Label,
    invoice_view: gtk::TextView,
    range: gtk::ComboBoxText,
    spend_label: gtk::Label,
    invoice_list: gtk::ListBox,
    forecast_list: gtk::ListBox,
    forecast_label: gtk::Label,
    store_box: gtk::Box,
    account_label: gtk::Label,
    service_label: gtk::Label,
    consent_open: Rc<Cell<bool>>,
}

pub fn open_window(app: &adw::Application, core: Arc<App>, backend: Arc<Registry>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaBring")
        .default_width(1100)
        .default_height(740)
        .build();
    window.add_css_class("omabring");
    // OMABRING_NO_CSS=1 skips the provider, palette CSS, and color-scheme
    // updates. Styling is applied before the window is shown, never from a
    // map, show, or notify handler.
    if !css_disabled() {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        let css = Rc::new(RefCell::new(String::new()));
        refresh_theme(&provider, &css);
        let provider_for_tick = provider.clone();
        let css_for_tick = css.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            refresh_theme(&provider_for_tick, &css_for_tick);
            ControlFlow::Continue
        });
    }

    let toast = adw::ToastOverlay::new();
    let root = gtk::Stack::new();
    toast.set_child(Some(&root));
    window.set_content(Some(&toast));

    let session = read_session(&core).and_then(|saved| core.resume(&saved.token).ok());
    if let Some(session) = session {
        root.add_named(
            &build_shell(&window, &toast, core.clone(), backend.clone(), session),
            Some("app"),
        );
        root.set_visible_child_name("app");
    } else {
        root.add_named(
            &build_login(&window, &toast, &root, core, backend),
            Some("login"),
        );
        root.set_visible_child_name("login");
    }
    window.present();
}

fn css_disabled() -> bool {
    std::env::var("OMABRING_NO_CSS").ok().as_deref() == Some("1")
}

thread_local! {
    static APPLYING_THEME: Cell<bool> = const { Cell::new(false) };
}

fn refresh_theme(provider: &gtk::CssProvider, current: &RefCell<String>) {
    // load_from_data and set_color_scheme restyle synchronously. Applying
    // again from that restyle would recurse on the main thread.
    if APPLYING_THEME.get() {
        return;
    }
    APPLYING_THEME.set(true);
    struct ClearApply;
    impl Drop for ClearApply {
        fn drop(&mut self) {
            APPLYING_THEME.set(false);
        }
    }
    let _guard = ClearApply;

    let palette = load_active_palette(&home_dir());
    let css = palette.to_css();
    if css != *current.borrow() {
        // Record first so a restyle sees the same stylesheet and returns.
        *current.borrow_mut() = css.clone();
        provider.load_from_data(&css);
    }
    let scheme = if palette.dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    };
    let manager = adw::StyleManager::default();
    if manager.color_scheme() != scheme {
        manager.set_color_scheme(scheme);
    }
}

fn build_login(
    window: &adw::ApplicationWindow,
    toast: &adw::ToastOverlay,
    root: &gtk::Stack,
    core: Arc<App>,
    backend: Arc<Registry>,
) -> gtk::Widget {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
    box_.set_valign(gtk::Align::Center);
    box_.set_halign(gtk::Align::Center);
    box_.set_width_request(420);
    box_.set_margin_top(24);
    box_.set_margin_bottom(24);
    let title = gtk::Label::new(Some("OmaBring"));
    title.add_css_class("title-1");
    let subtitle = gtk::Label::new(Some(
        "Search UAE stores, compare prices, and keep carts, receipts, and forecasts on this machine.",
    ));
    wrap_label(&subtitle);
    subtitle.add_css_class("dim-label");
    let email = adw::EntryRow::builder().title("Email").build();
    let password = adw::PasswordEntryRow::builder().title("Password").build();
    let group = adw::PreferencesGroup::new();
    group.add(&email);
    group.add(&password);
    let sign_in = gtk::Button::with_label("Sign in");
    sign_in.add_css_class("suggested-action");
    let create = gtk::Button::with_label("Create account");
    let error = gtk::Label::new(None);
    error.add_css_class("error");
    wrap_label(&error);
    box_.append(&title);
    box_.append(&subtitle);
    box_.append(&group);
    box_.append(&sign_in);
    box_.append(&create);
    box_.append(&error);

    let submit = {
        let core = core.clone();
        let backend = backend.clone();
        let window = window.clone();
        let toast = toast.clone();
        let root = root.clone();
        let email = email.clone();
        let password = password.clone();
        let error = error.clone();
        move |create_account: bool| {
            let email_text = email.text().to_string();
            let password_text = password.text().to_string();
            let result = if create_account {
                core.register(&email_text, &password_text)
            } else {
                core.login(&email_text, &password_text)
            };
            match result {
                Ok(session) => {
                    if let Err(err) = write_session(&core, &session) {
                        error.set_text(&err.to_string());
                        return;
                    }
                    let shell =
                        build_shell(&window, &toast, core.clone(), backend.clone(), session);
                    root.add_named(&shell, Some("app"));
                    root.set_visible_child_name("app");
                }
                Err(err) => error.set_text(&err.to_string()),
            }
        }
    };
    sign_in.connect_clicked({
        let submit = submit.clone();
        move |_| submit(false)
    });
    create.connect_clicked(move |_| submit(true));
    box_.upcast()
}

fn build_shell(
    window: &adw::ApplicationWindow,
    toast: &adw::ToastOverlay,
    core: Arc<App>,
    backend: Arc<Registry>,
    session: Session,
) -> gtk::Widget {
    let shell = Rc::new(RefCell::new(Shell {
        core,
        backend,
        window: window.clone(),
        toast: toast.clone(),
        user_id: session.user_id,
        token: session.token,
        search_entry: gtk::SearchEntry::new(),
        search_summary: gtk::Label::new(Some(
            "Search a product across linked stores and your receipts.",
        )),
        search_list: gtk::ListBox::new(),
        cart_store: gtk::ComboBoxText::new(),
        cart_name: gtk::Entry::new(),
        cart_qty: gtk::SpinButton::with_range(1.0, 99.0, 1.0),
        cart_price: gtk::Entry::new(),
        cart_list: gtk::ListBox::new(),
        checkout_label: gtk::Label::new(Some(
            "Checkout prepares the cart. Payment stays with the store.",
        )),
        invoice_view: gtk::TextView::new(),
        range: gtk::ComboBoxText::new(),
        spend_label: gtk::Label::new(None),
        invoice_list: gtk::ListBox::new(),
        forecast_list: gtk::ListBox::new(),
        forecast_label: gtk::Label::new(Some(
            "Forecasts use receipt dates. One-off and lapsed items stay off the list.",
        )),
        store_box: gtk::Box::new(gtk::Orientation::Vertical, 12),
        account_label: gtk::Label::new(Some(&format!("Signed in as {}", session.email))),
        service_label: gtk::Label::new(None),
        consent_open: Rc::new(Cell::new(false)),
    }));
    {
        let shell_ref = shell.borrow();
        for store in StoreId::all() {
            shell_ref
                .cart_store
                .append(Some(store.as_str()), store.name());
        }
        shell_ref.cart_store.set_active(Some(0));
        shell_ref
            .cart_name
            .set_placeholder_text(Some("Product name"));
        shell_ref.cart_price.set_placeholder_text(Some("Price"));
        shell_ref.invoice_view.set_monospace(true);
        shell_ref
            .invoice_view
            .set_wrap_mode(gtk::WrapMode::WordChar);
        shell_ref.invoice_view.buffer().set_text(
            r#"{"store":"carrefour","invoice_no":"1001","date":"2026-03-01","currency":"AED","items":[{"name":"Milk","qty":1,"unit":"piece","unit_price":5.25}]}"#,
        );
        for (id, label) in [
            ("this_month", "This month"),
            ("last_month", "Last month"),
            ("7", "Last 7 days"),
            ("30", "Last 30 days"),
            ("365", "Last 365 days"),
        ] {
            shell_ref.range.append(Some(id), label);
        }
        shell_ref.range.set_active(Some(0));
        wrap_label(&shell_ref.search_summary);
        wrap_label(&shell_ref.checkout_label);
        wrap_label(&shell_ref.spend_label);
        wrap_label(&shell_ref.forecast_label);
        shell_ref.account_label.set_xalign(0.0);
        shell_ref.service_label.set_xalign(0.0);
        shell_ref.search_list.add_css_class("boxed-list");
        shell_ref.cart_list.add_css_class("boxed-list");
        shell_ref.invoice_list.add_css_class("boxed-list");
        shell_ref.forecast_list.add_css_class("boxed-list");
    }

    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.set_selection_mode(gtk::SelectionMode::Single);
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    for (id, title) in [
        ("search", "Search"),
        ("carts", "Carts"),
        ("receipts", "Receipts"),
        ("forecast", "Forecast"),
        ("stores", "Stores"),
        ("account", "Account"),
    ] {
        let row = gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .margin_top(10)
            .margin_bottom(10)
            .margin_start(12)
            .margin_end(12)
            .build();
        row.set_widget_name(id);
        sidebar.append(&row);
        stack.add_named(&page(id, &shell), Some(id));
    }
    let stack_for_select = stack.clone();
    sidebar.connect_row_selected(move |_, row| {
        if let Some(row) = row {
            if let Some(child) = row.child() {
                stack_for_select.set_visible_child_name(&child.widget_name());
            }
        }
    });
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

    let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let side_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar)
        .width_request(180)
        .vexpand(true)
        .build();
    side_scroll.add_css_class("sidebar");
    split.append(&side_scroll);
    split.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    split.append(&stack);

    let header = adw::HeaderBar::new();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&split));

    wire(&shell);
    refresh_cart(&shell);
    refresh_receipts(&shell);
    refresh_forecast(&shell);
    refresh_stores(&shell);
    refresh_service(&shell);
    let shell_for_consent = shell.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(800), move || {
        poll_consent(&shell_for_consent);
        refresh_service(&shell_for_consent);
        ControlFlow::Continue
    });
    view.upcast()
}

fn page(id: &str, shell_rc: &Rc<RefCell<Shell>>) -> gtk::Widget {
    let shell = shell_rc.borrow();
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_margin_top(16);
    page.set_margin_bottom(16);
    page.set_margin_start(16);
    page.set_margin_end(16);
    match id {
        "search" => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            shell.search_entry.set_hexpand(true);
            let button = gtk::Button::with_label("Search");
            button.add_css_class("suggested-action");
            let shell_click = shell_rc.clone();
            button.connect_clicked(move |_| run_search(&shell_click));
            row.append(&shell.search_entry);
            row.append(&button);
            page.append(&row);
            page.append(&shell.search_summary);
            page.append(&scrolled(&shell.search_list));
        }
        "carts" => {
            let add = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            shell.cart_name.set_hexpand(true);
            let button = gtk::Button::with_label("Add");
            let shell_click = shell_rc.clone();
            button.connect_clicked(move |_| add_cart_line(&shell_click));
            add.append(&shell.cart_store);
            add.append(&shell.cart_name);
            add.append(&shell.cart_qty);
            add.append(&shell.cart_price);
            add.append(&button);
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let checkout = gtk::Button::with_label("Prepare checkout");
            checkout.add_css_class("suggested-action");
            let clear = gtk::Button::with_label("Clear cart");
            let shell_checkout = shell_rc.clone();
            checkout.connect_clicked(move |_| prepare_checkout(&shell_checkout));
            let shell_clear = shell_rc.clone();
            clear.connect_clicked(move |_| clear_cart(&shell_clear));
            let shell_switch = shell_rc.clone();
            shell
                .cart_store
                .connect_changed(move |_| refresh_cart(&shell_switch));
            actions.append(&checkout);
            actions.append(&clear);
            page.append(&add);
            page.append(&actions);
            page.append(&shell.checkout_label);
            page.append(&scrolled(&shell.cart_list));
        }
        "receipts" => {
            let view_scroll = gtk::ScrolledWindow::builder()
                .child(&shell.invoice_view)
                .min_content_height(140)
                .build();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let import = gtk::Button::with_label("Import receipt");
            import.add_css_class("suggested-action");
            let shell_import = shell_rc.clone();
            import.connect_clicked(move |_| import_receipt(&shell_import));
            let shell_range = shell_rc.clone();
            shell
                .range
                .connect_changed(move |_| refresh_receipts(&shell_range));
            row.append(&shell.range);
            row.append(&import);
            page.append(&view_scroll);
            page.append(&row);
            page.append(&shell.spend_label);
            page.append(&scrolled(&shell.invoice_list));
        }
        "forecast" => {
            page.append(&shell.forecast_label);
            page.append(&scrolled(&shell.forecast_list));
        }
        "stores" => {
            let intro = gtk::Label::new(Some(
                "Save each store sign-in on this machine. Add a contract address only when that store speaks the OmaBring store contract. Without one, search still uses your receipts and the cart stays local.",
            ));
            wrap_label(&intro);
            page.append(&intro);
            let scroll = scrolled(&shell.store_box);
            page.append(&scroll);
        }
        "account" => {
            page.append(&shell.account_label);
            page.append(&shell.service_label);
            let note = gtk::Label::new(Some(
                "The background service lets an approved client use the same carts, receipts, search, and forecasts. Approve each client in this window.",
            ));
            wrap_label(&note);
            let sign_out = gtk::Button::with_label("Sign out");
            sign_out.add_css_class("destructive-action");
            sign_out.set_halign(gtk::Align::Start);
            let shell_out = shell_rc.clone();
            sign_out.connect_clicked(move |_| sign_out_clicked(&shell_out));
            page.append(&note);
            page.append(&sign_out);
        }
        _ => {}
    }
    page.upcast()
}

/// Bound wrapped text so measure does not chase an unconstrained width.
fn wrap_label(label: &gtk::Label) {
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(48);
    label.set_xalign(0.0);
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(child)
        .vexpand(true)
        .hexpand(true)
        .build()
}

fn wire(shell: &Rc<RefCell<Shell>>) {
    let search = shell.clone();
    shell
        .borrow()
        .search_entry
        .connect_activate(move |_| run_search(&search));
}

fn run_search(shell: &Rc<RefCell<Shell>>) {
    let (core, backend, user, query) = {
        let shell = shell.borrow();
        (
            shell.core.clone(),
            shell.backend.clone(),
            shell.user_id,
            shell.search_entry.text().to_string(),
        )
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(core.search(user, &query, backend.as_ref()));
    });
    let shell = shell.clone();
    glib::idle_add_local(move || match rx.try_recv() {
        Ok(result) => {
            show_search(&shell, result);
            ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => ControlFlow::Continue,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => ControlFlow::Break,
    });
}

fn show_search(
    shell: &Rc<RefCell<Shell>>,
    result: omabring_core::Result<omabring_core::Comparison>,
) {
    let shell = shell.borrow();
    clear_list(&shell.search_list);
    match result {
        Err(err) => shell.search_summary.set_text(&err.to_string()),
        Ok(comparison) => {
            if comparison.groups.is_empty() {
                shell
                    .search_summary
                    .set_text("No matching products in live results or receipts.");
            } else {
                let mut lines = Vec::new();
                for group in &comparison.groups {
                    lines.push(format!(
                        "{} · cheapest {} {:.2} {}",
                        group.name,
                        group.cheapest_store.name(),
                        group.cheapest_price,
                        group.currency
                    ));
                }
                shell.search_summary.set_text(&lines.join("\n"));
            }
            for offer in comparison.offers {
                shell.search_list.append(&offer_row(&shell, &offer));
            }
        }
    }
}

fn offer_row(shell: &Shell, offer: &Offer) -> gtk::ListBoxRow {
    let row = adw::ActionRow::new();
    row.set_title(&offer.name);
    let source = match offer.source {
        omabring_core::OfferSource::Live => "live",
        omabring_core::OfferSource::History => "receipt",
    };
    row.set_subtitle(&format!(
        "{} · {:.2} {} / {} · {source}",
        offer.store.name(),
        offer.price,
        offer.currency,
        offer.unit
    ));
    let button = gtk::Button::with_label("Add");
    let core = shell.core.clone();
    let user = shell.user_id;
    let store = offer.store;
    let product_id = offer.product_id.clone();
    let name = offer.name.clone();
    let unit = offer.unit.clone();
    let price = offer.price;
    let currency = offer.currency.clone();
    let toast = shell.toast.clone();
    button.connect_clicked(move |_| {
        match core.cart_add(
            user,
            store,
            &product_id,
            &name,
            1.0,
            &unit,
            Some(price),
            &currency,
        ) {
            Ok(_) => toast.add_toast(adw::Toast::new(&format!(
                "{name} added to {}",
                store.name()
            ))),
            Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
        }
    });
    row.add_suffix(&button);
    let wrapper = gtk::ListBoxRow::new();
    wrapper.set_child(Some(&row));
    wrapper.set_activatable(false);
    wrapper
}

fn add_cart_line(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    let Some(store) = active_store(&shell_ref.cart_store) else {
        return;
    };
    let name = shell_ref.cart_name.text().to_string();
    let qty = shell_ref.cart_qty.value();
    let price_text = shell_ref.cart_price.text().to_string();
    let price = if price_text.trim().is_empty() {
        None
    } else {
        match price_text.trim().parse::<f64>() {
            Ok(value) => Some(value),
            Err(_) => {
                shell_ref
                    .toast
                    .add_toast(adw::Toast::new("Price must be a number"));
                return;
            }
        }
    };
    match shell_ref.core.cart_add(
        shell_ref.user_id,
        store,
        "",
        &name,
        qty,
        "piece",
        price,
        "AED",
    ) {
        Ok(_) => {
            drop(shell_ref);
            refresh_cart(shell);
        }
        Err(err) => shell_ref.toast.add_toast(adw::Toast::new(&err.to_string())),
    }
}

fn clear_cart(shell: &Rc<RefCell<Shell>>) {
    let store = active_store(&shell.borrow().cart_store);
    if let Some(store) = store {
        let shell_ref = shell.borrow();
        if let Err(err) = shell_ref.core.cart_clear(shell_ref.user_id, store) {
            shell_ref.toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
    }
    refresh_cart(shell);
}

fn prepare_checkout(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    let Some(store) = active_store(&shell_ref.cart_store) else {
        return;
    };
    let core = shell_ref.core.clone();
    let backend = shell_ref.backend.clone();
    let user = shell_ref.user_id;
    drop(shell_ref);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(core.checkout(user, store, backend.as_ref()));
    });
    let shell = shell.clone();
    glib::idle_add_local(move || match rx.try_recv() {
        Ok(result) => {
            let shell_ref = shell.borrow();
            match result {
                Ok(record) => {
                    let url = record.url.unwrap_or_default();
                    shell_ref
                        .checkout_label
                        .set_text(&format!("{} — {}\n{}", record.status, record.note, url));
                    shell_ref
                        .toast
                        .add_toast(adw::Toast::new("Checkout prepared"));
                }
                Err(err) => {
                    shell_ref.checkout_label.set_text(&err.to_string());
                }
            }
            ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => ControlFlow::Continue,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => ControlFlow::Break,
    });
}

fn refresh_cart(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    clear_list(&shell_ref.cart_list);
    let Some(store) = active_store(&shell_ref.cart_store) else {
        return;
    };
    let cart = match shell_ref.core.cart(shell_ref.user_id, store) {
        Ok(cart) => cart,
        Err(err) => {
            shell_ref.toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
    };
    if cart.lines.is_empty() {
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&gtk::Label::new(Some("This cart is empty."))));
        row.set_selectable(false);
        shell_ref.cart_list.append(&row);
        return;
    }
    for line in cart.lines {
        shell_ref
            .cart_list
            .append(&cart_row(shell, &shell_ref, store, &line));
    }
}

fn cart_row(
    shell_rc: &Rc<RefCell<Shell>>,
    shell: &Shell,
    store: StoreId,
    line: &CartLine,
) -> gtk::ListBoxRow {
    let price = line
        .unit_price
        .map(|price| format!("{price:.2} {}", line.currency))
        .unwrap_or_else(|| line.currency.clone());
    let row = adw::ActionRow::new();
    row.set_title(&line.name);
    row.set_subtitle(&format!("{} × {} · {price}", line.qty, line.unit));
    let remove = gtk::Button::with_label("Remove");
    let core = shell.core.clone();
    let user = shell.user_id;
    let line_id = line.line_id.clone();
    let toast = shell.toast.clone();
    let shell_after = shell_rc.clone();
    remove.connect_clicked(move |_| {
        if let Err(err) = core.cart_remove(user, store, &line_id) {
            toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
        refresh_cart(&shell_after);
    });
    row.add_suffix(&remove);
    let wrapper = gtk::ListBoxRow::new();
    wrapper.set_child(Some(&row));
    wrapper.set_activatable(false);
    wrapper
}

fn import_receipt(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    let buffer = shell_ref.invoice_view.buffer();
    let document = buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), false)
        .to_string();
    match shell_ref.core.import_document(shell_ref.user_id, &document) {
        Ok(saved) => {
            shell_ref.toast.add_toast(adw::Toast::new(&format!(
                "Imported {} receipt(s)",
                saved.len()
            )));
            drop(shell_ref);
            refresh_receipts(shell);
            refresh_forecast(shell);
        }
        Err(err) => shell_ref.toast.add_toast(adw::Toast::new(&err.to_string())),
    }
}

fn refresh_receipts(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    clear_list(&shell_ref.invoice_list);
    let range = match shell_ref.range.active() {
        Some(1) => SpendRange::LastMonth,
        Some(2) => SpendRange::LastDays(7),
        Some(3) => SpendRange::LastDays(30),
        Some(4) => SpendRange::LastDays(365),
        _ => SpendRange::ThisMonth,
    };
    let today = chrono::Utc::now().date_naive();
    match shell_ref.core.spend(shell_ref.user_id, range, today) {
        Ok(report) => {
            let totals = if report.totals.is_empty() {
                "0".into()
            } else {
                report
                    .totals
                    .iter()
                    .map(|total| format!("{:.2} {}", total.amount, total.currency))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            shell_ref.spend_label.set_text(&format!(
                "{} ({} to {}) · {} receipt(s) · {totals}",
                report.range_label, report.start, report.end, report.invoice_count
            ));
        }
        Err(err) => shell_ref.spend_label.set_text(&err.to_string()),
    }
    let invoices = match shell_ref.core.list_invoices(shell_ref.user_id) {
        Ok(invoices) => invoices,
        Err(err) => {
            shell_ref.toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
    };
    for invoice in invoices {
        let row = adw::ActionRow::new();
        row.set_title(&format!(
            "{} · {}",
            invoice.store.name(),
            invoice.invoice_no
        ));
        row.set_subtitle(&format!(
            "{} · {:.2} {}",
            invoice.purchased_on, invoice.total, invoice.currency
        ));
        let remove = gtk::Button::with_label("Delete");
        let core = shell_ref.core.clone();
        let user = shell_ref.user_id;
        let id = invoice.id;
        let toast = shell_ref.toast.clone();
        let shell_after = shell.clone();
        remove.connect_clicked(move |_| {
            if let Err(err) = core.delete_invoice(user, id) {
                toast.add_toast(adw::Toast::new(&err.to_string()));
                return;
            }
            refresh_receipts(&shell_after);
            refresh_forecast(&shell_after);
        });
        row.add_suffix(&remove);
        let wrapper = gtk::ListBoxRow::new();
        wrapper.set_child(Some(&row));
        wrapper.set_activatable(false);
        shell_ref.invoice_list.append(&wrapper);
    }
}

fn refresh_forecast(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    clear_list(&shell_ref.forecast_list);
    let today = chrono::Utc::now().date_naive();
    let items = match shell_ref.core.forecast(shell_ref.user_id, today) {
        Ok(items) => items,
        Err(err) => {
            shell_ref.forecast_label.set_text(&err.to_string());
            return;
        }
    };
    if items.is_empty() {
        shell_ref.forecast_label.set_text(
            "No repeat purchases yet. Import at least two receipts for the same product.",
        );
        return;
    }
    shell_ref.forecast_label.set_text("Buy again when a product is due. The score is higher when the gap is regular and the date is close.");
    for item in items {
        let row = adw::ActionRow::new();
        row.set_title(&item.name);
        row.set_subtitle(&format!(
            "{} · {} · next {} · in {} days · likely {} · typical {:.2} {}",
            item.store.name(),
            status_label(item.status),
            item.next_purchase,
            item.days_until,
            item.likely,
            item.typical_unit_price,
            item.currency
        ));
        let wrapper = gtk::ListBoxRow::new();
        wrapper.set_child(Some(&row));
        wrapper.set_activatable(false);
        shell_ref.forecast_list.append(&wrapper);
    }
}

fn refresh_stores(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    while let Some(child) = shell_ref.store_box.first_child() {
        shell_ref.store_box.remove(&child);
    }
    let links = match shell_ref.core.list_store_links(shell_ref.user_id) {
        Ok(links) => links,
        Err(err) => {
            shell_ref.toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
    };
    for link in links {
        let group = adw::PreferencesGroup::new();
        group.set_title(link.name);
        let status = if link.linked {
            format!(
                "Linked{}{}",
                link.username
                    .as_ref()
                    .map(|name| format!(" as {name}"))
                    .unwrap_or_default(),
                if link.remote_checkout {
                    ". Remote checkout is available when a contract address is set."
                } else {
                    ". Cart stays on this machine. Payment stays with the store."
                }
            )
        } else {
            "Not linked.".into()
        };
        group.set_description(Some(&status));
        let username = adw::EntryRow::builder().title("Username").build();
        if let Some(name) = &link.username {
            username.set_text(name);
        }
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        let base = adw::EntryRow::builder()
            .title("Contract address (optional)")
            .build();
        if let Some(url) = &link.base_url {
            base.set_text(url);
        }
        group.add(&username);
        group.add(&password);
        group.add(&base);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions.set_margin_top(8);
        let save = gtk::Button::with_label("Save");
        save.add_css_class("suggested-action");
        let remove = gtk::Button::with_label("Remove");
        let core = shell_ref.core.clone();
        let user = shell_ref.user_id;
        let toast = shell_ref.toast.clone();
        let store = link.id;
        save.connect_clicked(move |_| {
            let secrets = StoreSecrets {
                username: username.text().to_string(),
                password: password.text().to_string(),
                base_url: {
                    let url = base.text().to_string();
                    if url.trim().is_empty() {
                        None
                    } else {
                        Some(url)
                    }
                },
            };
            match core.link_store(user, store, &secrets) {
                Ok(()) => toast.add_toast(adw::Toast::new(&format!("{} saved", store.name()))),
                Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
            }
        });
        let core = shell_ref.core.clone();
        let toast = shell_ref.toast.clone();
        remove.connect_clicked(move |_| match core.unlink_store(user, store) {
            Ok(()) => toast.add_toast(adw::Toast::new(&format!("{} removed", store.name()))),
            Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
        });
        actions.append(&save);
        actions.append(&remove);
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 6);
        holder.append(&group);
        holder.append(&actions);
        shell_ref.store_box.append(&holder);
    }
}

fn refresh_service(shell: &Rc<RefCell<Shell>>) {
    let address = omabring_core::default_bind_addr();
    let up = std::net::TcpStream::connect_timeout(
        &address
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8787".parse().unwrap()),
        std::time::Duration::from_millis(200),
    )
    .is_ok();
    let text = if up {
        format!("Local service is running at http://{address}")
    } else {
        format!("Local service is not running. Start it with the user service, or run omabring-service. It listens at http://{address}")
    };
    shell.borrow().service_label.set_text(&text);
}

fn poll_consent(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    if shell_ref.consent_open.get() {
        return;
    }
    let pending = match shell_ref.core.list_pending() {
        Ok(pending) => pending,
        Err(_) => return,
    };
    let Some(request) = pending.into_iter().next() else {
        return;
    };
    shell_ref.consent_open.set(true);
    let dialog = adw::AlertDialog::new(
        Some("Allow a client to use OmaBring?"),
        Some(&format!(
            "{} wants to use your carts, receipts, search, and forecasts.",
            request.client_name
        )),
    );
    dialog.add_response("deny", "Deny");
    dialog.add_response("allow", "Allow");
    dialog.set_response_appearance("allow", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("deny"));
    dialog.set_close_response("deny");
    let core = shell_ref.core.clone();
    let user = shell_ref.user_id;
    let flag = shell_ref.consent_open.clone();
    let id = request.id;
    dialog.connect_response(None, move |_, response| {
        let allow = response == "allow";
        let _ = core.decide_authorization(&id, user, allow);
        flag.set(false);
    });
    dialog.present(Some(&shell_ref.window));
}

fn sign_out_clicked(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    let _ = shell_ref.core.logout(&shell_ref.token);
    let _ = fs::remove_file(session_path(&shell_ref.core));
    shell_ref.window.close();
}

fn status_label(status: omabring_core::ForecastStatus) -> &'static str {
    match status {
        omabring_core::ForecastStatus::Overdue => "overdue",
        omabring_core::ForecastStatus::DueToday => "due today",
        omabring_core::ForecastStatus::DueSoon => "due soon",
        omabring_core::ForecastStatus::Upcoming => "upcoming",
    }
}

fn active_store(combo: &gtk::ComboBoxText) -> Option<StoreId> {
    let index = combo.active()?;
    StoreId::all().get(index as usize).copied()
}

fn clear_list(list: &gtk::ListBox) {
    while let Some(row) = list.row_at_index(0) {
        list.remove(&row);
    }
}

fn session_path(core: &App) -> PathBuf {
    core.data_dir().join("session.json")
}

fn read_session(core: &App) -> Option<SavedSession> {
    let text = fs::read_to_string(session_path(core)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_session(core: &App, session: &Session) -> omabring_core::Result<()> {
    let text = serde_json::to_string(&SavedSession {
        token: session.token.clone(),
    })
    .map_err(|err| omabring_core::Error::Message(err.to_string()))?;
    let path = session_path(core);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())?;
    }
    Ok(())
}
