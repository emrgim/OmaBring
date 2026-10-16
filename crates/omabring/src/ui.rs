#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use chrono::{Duration, NaiveDate};
use gtk::gio::{Cancellable, FileMonitorFlags};
use gtk::glib::{self, ControlFlow};
use gtk::prelude::FileExt;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use omabring_core::{
    home_dir, load_active_palette, theme_watch_paths, App, CartLine, ForecastStatus,
    InvoiceSummary, Offer, StoreId,
};
use omabring_stores::Registry;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Grain {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Spent,
    Frequency,
    Likely,
    Times,
    Name,
}

#[derive(Clone, PartialEq, Eq)]
enum Page {
    Search,
    Dashboard,
    Purchases,
    Stores,
    Settings,
    Forecast,
    Store(StoreId),
    Product(String),
    Receipt(i64),
}

struct Shell {
    core: Arc<App>,
    backend: Arc<Registry>,
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    content: gtk::Box,
    tab_search: gtk::Button,
    tab_dash: gtk::Button,
    tab_buy: gtk::Button,
    tab_stores: gtk::Button,
    tab_settings: gtk::Button,
    tab_forecast: gtk::Button,
    search_entry: gtk::Entry,
    mode_label: gtk::Label,
    service_label: gtk::Label,
    page: Page,
    grain: Grain,
    range_days: i64,
    custom: bool,
    custom_start: String,
    custom_end: String,
    sort: SortKey,
    store_filter: Option<StoreId>,
    checkout_note: String,
    import_open: bool,
    compare: Vec<Offer>,
    compare_note: String,
    compare_for: String,
    search_query: String,
    search_note: String,
    search_offers: Vec<Offer>,
    search_started: String,
    focus_nav: bool,
    focus_query: bool,
    consent_open: Rc<Cell<bool>>,
}

struct Prod {
    name: String,
    times: usize,
    qty: f64,
    spent: f64,
    first: NaiveDate,
    last: NaiveDate,
    first_price: f64,
    last_price: f64,
    likely: u8,
}

pub fn open_window(app: &adw::Application, core: Arc<App>, backend: Arc<Registry>) {
    seed_demo(&core);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaBring")
        .default_width(1100)
        .default_height(740)
        .build();
    window.add_css_class("omabring");
    window.set_decorated(false);
    if !css_disabled() {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        follow_theme(provider);
    }

    let toast = adw::ToastOverlay::new();
    let shell = build_chrome(&window, &toast, core, backend);
    let root = chrome_root(&shell);
    toast.set_child(Some(&root));
    window.set_content(Some(&toast));
    render(&shell);
    install_keys(&window, &shell);
    window.present();

    let shell_for_consent = shell.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(800), move || {
        poll_consent(&shell_for_consent);
        let dark = load_active_palette(&home_dir()).dark;
        shell_for_consent
            .borrow()
            .mode_label
            .set_text(if dark { "DARK" } else { "LIGHT" });
        let address = omabring_core::default_bind_addr();
        let up = std::net::TcpStream::connect_timeout(
            &address
                .parse()
                .unwrap_or_else(|_| "127.0.0.1:8787".parse().unwrap()),
            std::time::Duration::from_millis(200),
        )
        .is_ok();
        let text = if up {
            format!("Running at http://{address}")
        } else {
            format!("Not running. Start the user service. It listens at http://{address}")
        };
        shell_for_consent.borrow().service_label.set_text(&text);
        ControlFlow::Continue
    });
}

fn css_disabled() -> bool {
    std::env::var("OMABRING_NO_CSS").ok().as_deref() == Some("1")
}

thread_local! {
    static APPLYING_THEME: Cell<bool> = const { Cell::new(false) };
}

fn follow_theme(provider: gtk::CssProvider) {
    let css = Rc::new(RefCell::new(String::new()));
    refresh_theme(&provider, &css);
    let monitors = Rc::new(RefCell::new(Vec::<gtk::gio::FileMonitor>::new()));
    let armed = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let provider_for_watch = provider.clone();
    let css_for_watch = css.clone();
    let arm = move || {
        let paths = theme_watch_paths(&home_dir());
        if *armed.borrow() == paths {
            return;
        }
        *armed.borrow_mut() = paths.clone();
        let mut held = monitors.borrow_mut();
        held.clear();
        for path in paths {
            let file = gtk::gio::File::for_path(&path);
            let watched = if path.is_dir() {
                file.monitor_directory(FileMonitorFlags::WATCH_MOVES, Cancellable::NONE)
            } else {
                file.monitor_file(FileMonitorFlags::WATCH_MOVES, Cancellable::NONE)
            };
            let Ok(monitor) = watched else {
                continue;
            };
            let provider = provider_for_watch.clone();
            let css = css_for_watch.clone();
            monitor.connect_changed(move |_, _, _, _| {
                refresh_theme(&provider, &css);
            });
            held.push(monitor);
        }
    };
    arm();
    let provider_for_tick = provider.clone();
    let css_for_tick = css.clone();
    glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
        refresh_theme(&provider_for_tick, &css_for_tick);
        arm();
        ControlFlow::Continue
    });
}

fn refresh_theme(provider: &gtk::CssProvider, current: &RefCell<String>) {
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

fn build_chrome(
    window: &adw::ApplicationWindow,
    toast: &adw::ToastOverlay,
    core: Arc<App>,
    backend: Arc<Registry>,
) -> Rc<RefCell<Shell>> {
    let tab = |text: &str| {
        let button = gtk::Button::with_label(text);
        button.add_css_class("bf-tab");
        button
    };
    let mode = gtk::Label::new(Some(if load_active_palette(&home_dir()).dark {
        "DARK"
    } else {
        "LIGHT"
    }));
    mode.add_css_class("bf-theme");
    let service = gtk::Label::new(Some(""));
    service.set_xalign(0.0);
    service.set_selectable(true);
    wrap_label(&service);
    let search_entry = gtk::Entry::new();
    search_entry.set_placeholder_text(Some("Product"));
    search_entry.set_hexpand(true);
    let page = initial_page();
    let focus_query = matches!(page, Page::Search);
    let shell = Rc::new(RefCell::new(Shell {
        core,
        backend,
        window: window.clone(),
        toast: toast.clone(),
        content: gtk::Box::new(gtk::Orientation::Vertical, 12),
        tab_search: tab("SEARCH"),
        tab_dash: tab("DASHBOARD"),
        tab_buy: tab("PURCHASES"),
        tab_stores: tab("STORES"),
        tab_settings: tab("SETTINGS"),
        tab_forecast: tab("FORECAST"),
        search_entry,
        mode_label: mode,
        service_label: service,
        page: page.clone(),
        grain: Grain::Weekly,
        range_days: 30,
        custom: false,
        custom_start: String::new(),
        custom_end: String::new(),
        sort: SortKey::Spent,
        store_filter: None,
        checkout_note: "Checkout prepares the cart. Payment stays with the store.".into(),
        import_open: false,
        compare: Vec::new(),
        compare_note: String::new(),
        compare_for: String::new(),
        search_query: String::new(),
        search_note: String::new(),
        search_offers: Vec::new(),
        search_started: String::new(),
        focus_nav: !focus_query,
        focus_query,
        consent_open: Rc::new(Cell::new(false)),
    }));
    let shell_entry = shell.clone();
    let entry = shell.borrow().search_entry.clone();
    entry.connect_activate(move |entry| run_search(&shell_entry, entry));
    shell
}

fn chrome_root(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header.add_css_class("bf-header");
    header.set_margin_top(10);
    header.set_margin_bottom(10);
    header.set_margin_start(14);
    header.set_margin_end(14);

    let mark = gtk::Label::new(Some("OB"));
    mark.add_css_class("bf-mark");
    mark.set_xalign(0.5);
    mark.set_yalign(0.5);
    mark.set_size_request(34, 34);

    let brand = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let title = gtk::Label::new(Some("OMABRING"));
    title.add_css_class("bf-brand");
    title.set_xalign(0.0);
    let kicker = gtk::Label::new(Some("AGENT GROCERY"));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    brand.append(&title);
    brand.append(&kicker);

    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    {
        let shell_ref = shell.borrow();
        tabs.append(&shell_ref.tab_search);
        tabs.append(&shell_ref.tab_dash);
        tabs.append(&shell_ref.tab_buy);
        tabs.append(&shell_ref.tab_stores);
        tabs.append(&shell_ref.tab_settings);
        tabs.append(&shell_ref.tab_forecast);
    }
    let go = |shell: &Rc<RefCell<Shell>>, button: &gtk::Button, page: Page| {
        let shell = shell.clone();
        button.connect_clicked(move |_| select_page(&shell, page.clone()));
    };
    go(shell, &shell.borrow().tab_search, Page::Search);
    go(shell, &shell.borrow().tab_dash, Page::Dashboard);
    go(shell, &shell.borrow().tab_buy, Page::Purchases);
    go(shell, &shell.borrow().tab_stores, Page::Stores);
    go(shell, &shell.borrow().tab_settings, Page::Settings);
    go(shell, &shell.borrow().tab_forecast, Page::Forecast);

    header.append(&mark);
    header.append(&brand);
    header.append(&tabs);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&spacer);
    header.append(&shell.borrow().mode_label);

    let scroll = gtk::ScrolledWindow::builder()
        .child(&shell.borrow().content)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    shell.borrow().content.set_margin_top(16);
    shell.borrow().content.set_margin_bottom(24);
    shell.borrow().content.set_margin_start(16);
    shell.borrow().content.set_margin_end(16);
    root.append(&header);
    root.append(&scroll);
    root
}

fn schedule(shell: &Rc<RefCell<Shell>>) {
    let shell = shell.clone();
    glib::idle_add_local(move || {
        render(&shell);
        ControlFlow::Break
    });
}

fn render(shell: &Rc<RefCell<Shell>>) {
    let restore_entry =
        matches!(shell.borrow().page, Page::Search) && shell.borrow().search_entry.has_focus();
    let content = shell.borrow().content.clone();
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
    update_tabs(shell);
    let page = shell.borrow().page.clone();
    let body: gtk::Widget = match page {
        Page::Search => search_page(shell).upcast(),
        Page::Dashboard => board(shell, false).upcast(),
        Page::Purchases => purchases(shell).upcast(),
        Page::Stores => stores_page(shell).upcast(),
        Page::Settings => settings_page(shell).upcast(),
        Page::Forecast => forecast_page(shell).upcast(),
        Page::Store(id) => store_page(shell, id).upcast(),
        Page::Product(_) => product_page(shell).upcast(),
        Page::Receipt(id) => receipt_page(shell, id).upcast(),
    };
    content.append(&body);
    if restore_entry {
        shell.borrow_mut().focus_query = true;
    }
    if shell.borrow().focus_nav || shell.borrow().focus_query {
        let shell = shell.clone();
        glib::idle_add_local(move || {
            apply_focus(&shell);
            ControlFlow::Break
        });
    }
}

fn update_tabs(shell: &Rc<RefCell<Shell>>) {
    let shell_ref = shell.borrow();
    let page = shell_ref.page.clone();
    let mark = |button: &gtk::Button, on: bool| {
        if on {
            button.add_css_class("on");
        } else {
            button.remove_css_class("on");
        }
    };
    mark(&shell_ref.tab_search, page == Page::Search);
    mark(&shell_ref.tab_dash, page == Page::Dashboard);
    mark(
        &shell_ref.tab_buy,
        matches!(page, Page::Purchases | Page::Product(_) | Page::Receipt(_)),
    );
    mark(
        &shell_ref.tab_stores,
        matches!(page, Page::Stores | Page::Store(_)),
    );
    mark(&shell_ref.tab_settings, page == Page::Settings);
    mark(&shell_ref.tab_forecast, page == Page::Forecast);
}

fn select_page(shell: &Rc<RefCell<Shell>>, page: Page) {
    let mut state = shell.borrow_mut();
    state.focus_query = matches!(page, Page::Search);
    state.focus_nav = !matches!(page, Page::Search);
    state.page = page;
    drop(state);
    schedule(shell);
}

fn nav_slot(page: &Page) -> usize {
    match page {
        Page::Search => 0,
        Page::Dashboard => 1,
        Page::Purchases | Page::Product(_) | Page::Receipt(_) => 2,
        Page::Stores | Page::Store(_) => 3,
        Page::Settings => 4,
        Page::Forecast => 5,
    }
}

fn page_at(index: usize) -> Page {
    match index % 6 {
        0 => Page::Search,
        1 => Page::Dashboard,
        2 => Page::Purchases,
        3 => Page::Stores,
        4 => Page::Settings,
        _ => Page::Forecast,
    }
}

fn page_from(value: Option<&str>) -> Page {
    match value {
        Some("dashboard") => Page::Dashboard,
        Some("purchases") | Some("receipts") => Page::Purchases,
        Some("stores") | Some("carts") => Page::Stores,
        Some("settings") | Some("service") => Page::Settings,
        Some("store") => Page::Store(StoreId::Carrefour),
        Some("product") => Page::Product("Milk".into()),
        Some("receipt") => Page::Receipt(0),
        Some("forecast") => Page::Forecast,
        _ => Page::Search,
    }
}

fn tab_button(shell: &Shell) -> gtk::Button {
    match &shell.page {
        Page::Search => shell.tab_search.clone(),
        Page::Dashboard => shell.tab_dash.clone(),
        Page::Purchases | Page::Product(_) | Page::Receipt(_) => shell.tab_buy.clone(),
        Page::Stores | Page::Store(_) => shell.tab_stores.clone(),
        Page::Settings => shell.tab_settings.clone(),
        Page::Forecast => shell.tab_forecast.clone(),
    }
}

fn apply_focus(shell: &Rc<RefCell<Shell>>) {
    let (nav, query) = {
        let mut state = shell.borrow_mut();
        let nav = state.focus_nav;
        let query = state.focus_query;
        state.focus_nav = false;
        state.focus_query = false;
        (nav, query)
    };
    if query {
        shell.borrow().search_entry.grab_focus();
    } else if nav {
        tab_button(&shell.borrow()).grab_focus();
    }
}

fn in_text_view(window: &adw::ApplicationWindow) -> bool {
    let mut current = gtk::prelude::RootExt::focus(window);
    while let Some(widget) = current {
        if widget.is::<gtk::TextView>() || widget.is::<gtk::Text>() {
            return true;
        }
        current = widget.parent();
    }
    false
}

fn alt_index(key: gtk::gdk::Key) -> Option<usize> {
    match key {
        gtk::gdk::Key::_1 | gtk::gdk::Key::KP_1 => Some(0),
        gtk::gdk::Key::_2 | gtk::gdk::Key::KP_2 => Some(1),
        gtk::gdk::Key::_3 | gtk::gdk::Key::KP_3 => Some(2),
        gtk::gdk::Key::_4 | gtk::gdk::Key::KP_4 => Some(3),
        gtk::gdk::Key::_5 | gtk::gdk::Key::KP_5 => Some(4),
        gtk::gdk::Key::_6 | gtk::gdk::Key::KP_6 => Some(5),
        _ => None,
    }
}

fn install_keys(window: &adw::ApplicationWindow, shell: &Rc<RefCell<Shell>>) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let shell = shell.clone();
    keys.connect_key_pressed(move |_, key, _, state| {
        if shell.borrow().consent_open.get() {
            return glib::Propagation::Proceed;
        }
        if state.contains(gtk::gdk::ModifierType::ALT_MASK) {
            if let Some(index) = alt_index(key) {
                select_page(&shell, page_at(index));
                return glib::Propagation::Stop;
            }
        }
        let arrow = matches!(
            key,
            gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up | gtk::gdk::Key::Down | gtk::gdk::Key::KP_Down
        );
        if arrow && !in_text_view(&shell.borrow().window) {
            let current = nav_slot(&shell.borrow().page);
            let next = if matches!(key, gtk::gdk::Key::Up | gtk::gdk::Key::KP_Up) {
                (current + 5) % 6
            } else {
                (current + 1) % 6
            };
            select_page(&shell, page_at(next));
            return glib::Propagation::Stop;
        }
        // Enter activates the focused tab through the button itself.
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
}

fn run_search(shell: &Rc<RefCell<Shell>>, entry: &gtk::Entry) {
    let query = entry.text().trim().to_string();
    {
        let mut state = shell.borrow_mut();
        state.search_query = query.clone();
        state.search_started.clear();
        state.focus_query = false;
        state.focus_nav = false;
        state.page = Page::Search;
        if query.is_empty() {
            state.search_offers.clear();
            state.search_note = "Enter a product to search.".into();
        }
    }
    schedule(shell);
}

fn search_page(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.append(&heading_mark("", "SEARCH"));
    let lead = gtk::Label::new(Some(
        "Compare a product across the stores on this machine, then add it to a cart.",
    ));
    lead.add_css_class("bf-muted");
    wrap_label(&lead);
    page.append(&lead);

    let query = shell.borrow().search_query.clone();
    let started = shell.borrow().search_started.clone();
    if !query.is_empty() && started != query {
        shell.borrow_mut().search_started = query.clone();
        shell.borrow_mut().search_offers.clear();
        shell.borrow_mut().search_note = "Searching…".into();
        let (core, backend) = {
            let shell = shell.borrow();
            (shell.core.clone(), shell.backend.clone())
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(core.search(&query, backend.as_ref()));
        });
        let shell_done = shell.clone();
        let expected = shell.borrow().search_query.clone();
        glib::idle_add_local(move || match rx.try_recv() {
            Ok(result) => {
                let mut state = shell_done.borrow_mut();
                if state.search_query == expected {
                    match result {
                        Ok(comparison) => {
                            let count = comparison.offers.len();
                            state.search_offers = comparison.offers;
                            state.search_note = if count == 0 {
                                "No matching products.".into()
                            } else {
                                format!("{count} offers")
                            };
                        }
                        Err(err) => state.search_note = err.to_string(),
                    }
                    let still = state.page == Page::Search;
                    drop(state);
                    if still {
                        schedule(&shell_done);
                    }
                }
                ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => ControlFlow::Break,
        });
    } else if query.is_empty() && shell.borrow().search_note.is_empty() {
        shell.borrow_mut().search_note = "Enter a product to search.".into();
    }

    let find = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = shell.borrow().search_entry.clone();
    if entry.text().as_str() != shell.borrow().search_query {
        entry.set_text(&shell.borrow().search_query);
    }
    let button = gtk::Button::with_label("SEARCH");
    let shell_find = shell.clone();
    let entry_find = entry.clone();
    button.connect_clicked(move |_| run_search(&shell_find, &entry_find));
    find.append(&entry);
    find.append(&button);
    page.append(&find);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("bf-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("bf-thead");
    for (text, end) in [("STORE", false), ("PRODUCT", false), ("PRICE", true)] {
        let label = cell(text, 0, end);
        if !end {
            label.set_hexpand(true);
        }
        label.add_css_class("bf-kicker");
        head.append(&label);
    }
    card.append(&head);
    let note = gtk::Label::new(Some(&shell.borrow().search_note));
    note.add_css_class("bf-muted");
    note.set_xalign(0.0);
    wrap_label(&note);
    card.append(&note);
    let offers = shell.borrow().search_offers.clone();
    for offer in offers {
        card.append(&offer_row(shell, &offer));
    }
    page.append(&card);
    page
}

fn forecast_page(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.append(&heading_mark("", "FORECAST"));
    let lead = gtk::Label::new(Some(
        "What to buy again, from receipts already on this machine.",
    ));
    lead.add_css_class("bf-muted");
    wrap_label(&lead);
    page.append(&lead);
    let today = chrono::Utc::now().date_naive();
    let items = shell.borrow().core.forecast(today).unwrap_or_default();
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("bf-card");
    if items.is_empty() {
        let empty = gtk::Label::new(Some("Nothing to buy again yet."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        card.append(&empty);
        page.append(&card);
        return page;
    }
    for item in items {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("bf-row");
        let status = match item.status {
            ForecastStatus::Overdue => "overdue",
            ForecastStatus::DueToday => "due today",
            ForecastStatus::DueSoon => "due soon",
            ForecastStatus::Upcoming => "upcoming",
        };
        let label = gtk::Label::new(Some(&format!(
            "{}   ·   {}   ·   {status}   ·   {}   ·   likely {}",
            item.name,
            store_label(item.store),
            item.next_purchase,
            item.likely
        )));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&label);
        card.append(&row);
    }
    page.append(&card);
    page
}

fn open_product(shell: &Rc<RefCell<Shell>>, entry: &gtk::Entry) {
    let name = entry.text().trim().to_string();
    if name.is_empty() {
        return;
    }
    let mut state = shell.borrow_mut();
    state.compare_for.clear();
    state.page = Page::Product(name);
    drop(state);
    schedule(shell);
}

fn purchases(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.append(&board(shell, true));
    let stores = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let all = chip("All", shell.borrow().store_filter.is_none());
    let shell_all = shell.clone();
    all.connect_clicked(move |_| {
        shell_all.borrow_mut().store_filter = None;
        schedule(&shell_all);
    });
    stores.append(&all);
    for id in StoreId::all() {
        let on = shell.borrow().store_filter == Some(id);
        let button = chip(store_chip_label(id), on);
        let shell_click = shell.clone();
        button.connect_clicked(move |_| {
            shell_click.borrow_mut().store_filter = Some(id);
            schedule(&shell_click);
        });
        stores.append(&button);
    }
    page.append(&stores);

    let find = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Find a product"));
    entry.set_hexpand(true);
    let button = gtk::Button::with_label("COMPARE");
    let shell_find = shell.clone();
    let entry_find = entry.clone();
    button.connect_clicked(move |_| open_product(&shell_find, &entry_find));
    let shell_find = shell.clone();
    let entry_find = entry.clone();
    entry.connect_activate(move |_| open_product(&shell_find, &entry_find));
    find.append(&entry);
    find.append(&button);
    page.append(&find);

    let view = load_view(shell);
    page.append(&product_table(shell, &view.products, true));

    let import = gtk::Button::with_label("IMPORT RECEIPT");
    import.add_css_class("bf-ghost");
    import.set_halign(gtk::Align::Start);
    let shell_import = shell.clone();
    import.connect_clicked(move |_| {
        let open = shell_import.borrow().import_open;
        shell_import.borrow_mut().import_open = !open;
        schedule(&shell_import);
    });
    page.append(&import);
    if shell.borrow().import_open {
        page.append(&import_card(shell));
    }
    page
}

fn board(shell: &Rc<RefCell<Shell>>, purchases: bool) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let filters = gtk::Box::new(gtk::Orientation::Vertical, 6);
    filters.add_css_class("bf-filters");
    filters.append(&range_row(shell));
    filters.append(&date_row(shell));
    page.append(&filters);

    let dash = gtk::Box::new(gtk::Orientation::Vertical, 6);
    dash.add_css_class("bf-card");
    dash.add_css_class("bf-dash");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_hexpand(true);
    titles.set_halign(gtk::Align::Start);
    if purchases {
        let h1 = heading("PURCHASES");
        h1.add_css_class("bf-h1-sm");
        titles.append(&h1);
    } else {
        titles.append(&heading_mark("YOUR ", "SPEND"));
    }
    top.append(&titles);
    top.append(&grain_row(shell));
    dash.append(&top);

    let view = load_view(shell);
    if purchases {
        let kpis = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        kpis.append(&kpi(&format!("AED {:.0}", view.total), "SPENT"));
        kpis.append(&kpi(
            &view.buckets.len().to_string(),
            &period_unit(shell.borrow().grain, view.buckets.len()),
        ));
        kpis.append(&kpi(&view.receipts.to_string(), "RECEIPTS"));
        dash.append(&kpis);
    }

    let avg = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let who = gtk::Label::new(Some(&format!(
        "{} AVERAGE THIS PERIOD",
        grain_word(shell.borrow().grain)
    )));
    who.add_css_class("bf-kicker");
    who.set_xalign(0.0);
    let delta = gtk::Label::new(Some(&format!("AED {:.2}", view.average)));
    delta.add_css_class("bf-delta");
    delta.set_xalign(0.0);
    let fine = gtk::Label::new(Some(&format!(
        "AED {:.2}  ÷  {} {}   ·   {}  →  {}",
        view.total,
        view.buckets.len(),
        period_unit(shell.borrow().grain, view.buckets.len()),
        view.start,
        view.end
    )));
    fine.add_css_class("bf-muted");
    fine.set_xalign(0.0);
    avg.append(&who);
    avg.append(&delta);
    avg.append(&fine);
    dash.append(&avg);
    dash.append(&bars(&view.buckets));
    page.append(&dash);

    if !purchases {
        page.append(&trend_card(&view.products));
        page.append(&product_table(shell, &view.products, false));
    }
    page
}

fn grain_row(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (grain, label) in [
        (Grain::Daily, "DAILY"),
        (Grain::Weekly, "WEEKLY"),
        (Grain::Monthly, "MONTHLY"),
        (Grain::Yearly, "YEARLY"),
    ] {
        let button = chip(label, shell.borrow().grain == grain);
        let shell_click = shell.clone();
        button.connect_clicked(move |_| {
            shell_click.borrow_mut().grain = grain;
            schedule(&shell_click);
        });
        row.append(&button);
    }
    row
}

fn range_row(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let custom = shell.borrow().custom;
    let days = shell.borrow().range_days;
    for (len, label) in [
        (7, "1w"),
        (14, "2w"),
        (30, "1m"),
        (90, "3m"),
        (365, "1y"),
        (730, "2y"),
        (1095, "3y"),
        (0, "All"),
    ] {
        let on = !custom && days == len;
        let button = chip(label, on);
        let shell_click = shell.clone();
        button.connect_clicked(move |_| {
            let mut shell = shell_click.borrow_mut();
            shell.range_days = len;
            shell.custom = false;
            drop(shell);
            schedule(&shell_click);
        });
        row.append(&button);
    }
    row
}

fn date_row(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let start = gtk::Entry::new();
    start.set_placeholder_text(Some("Start"));
    start.set_text(&shell.borrow().custom_start);
    let end = gtk::Entry::new();
    end.set_placeholder_text(Some("End"));
    end.set_text(&shell.borrow().custom_end);
    let apply = gtk::Button::with_label("APPLY");
    apply.add_css_class("bf-ghost");
    let shell_click = shell.clone();
    let start_c = start.clone();
    let end_c = end.clone();
    apply.connect_clicked(move |_| {
        let mut shell = shell_click.borrow_mut();
        shell.custom_start = start_c.text().trim().to_string();
        shell.custom_end = end_c.text().trim().to_string();
        shell.custom = NaiveDate::parse_from_str(&shell.custom_start, "%Y-%m-%d").is_ok()
            && NaiveDate::parse_from_str(&shell.custom_end, "%Y-%m-%d").is_ok();
        drop(shell);
        schedule(&shell_click);
    });
    row.append(&start);
    row.append(&end);
    row.append(&apply);
    row
}

fn bars(buckets: &[(String, f64)]) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    row.set_halign(gtk::Align::Start);
    row.set_size_request(-1, 128);
    let max = buckets.iter().fold(0.0_f64, |acc, (_, n)| acc.max(*n));
    let wide = buckets.len() <= 16;
    let bar_width = if wide { 22 } else { 9 };
    for (index, (label, amount)) in buckets.iter().enumerate() {
        let track = gtk::Box::new(gtk::Orientation::Vertical, 0);
        track.add_css_class("bf-bar");
        track.set_hexpand(false);
        track.set_size_request(bar_width, 110);
        if index + 1 == buckets.len() {
            track.add_css_class("on");
        }
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        let fill = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fill.add_css_class("bf-bar-fill");
        fill.set_hexpand(true);
        let height = if max <= 0.0 {
            0
        } else {
            ((*amount / max) * 110.0).round().clamp(0.0, 110.0) as i32
        };
        if height > 0 {
            fill.set_height_request(height.max(2));
        }
        track.append(&spacer);
        track.append(&fill);
        track.set_tooltip_text(Some(&format!("{label} · AED {amount:.2}")));
        let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
        col.set_hexpand(false);
        col.append(&track);
        if wide {
            let caption = gtk::Label::new(Some(label));
            caption.add_css_class("bf-muted");
            caption.set_xalign(0.5);
            col.append(&caption);
        }
        row.append(&col);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .child(&row)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(140)
        .propagate_natural_height(true)
        .build();
    scroll.upcast()
}

fn trend_card(products: &[Prod]) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("bf-card");
    let kicker = gtk::Label::new(Some("PRICE TREND"));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    card.append(&kicker);
    let changes: Vec<f64> = products
        .iter()
        .filter(|p| p.times >= 2 && p.first_price > 0.0)
        .map(|p| (p.last_price - p.first_price) / p.first_price * 100.0)
        .collect();
    if changes.is_empty() {
        let line = gtk::Label::new(Some("Need two buys on a product"));
        line.set_xalign(0.0);
        card.append(&line);
        return card;
    }
    let avg = changes.iter().sum::<f64>() / changes.len() as f64;
    let mut sorted = changes.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = sorted[sorted.len() / 2];
    let up = changes.iter().filter(|n| **n > 1.0).count();
    let down = changes.iter().filter(|n| **n < -1.0).count();
    let flat = changes.len() - up - down;
    let word = if avg > 1.0 {
        "Prices are rising"
    } else if avg < -1.0 {
        "Prices are falling"
    } else {
        "Prices are stable"
    };
    let arrow = if avg > 1.0 {
        "↑"
    } else if avg < -1.0 {
        "↓"
    } else {
        "→"
    };
    let delta = gtk::Label::new(Some(&format!("{arrow} {avg:+.1}%")));
    delta.add_css_class("bf-delta");
    delta.set_xalign(0.0);
    let detail = gtk::Label::new(Some(&format!(
        "{word}\n{} products · typical {median:+.1}% · {up} up · {down} down · {flat} flat",
        changes.len()
    )));
    detail.set_xalign(0.0);
    wrap_label(&detail);
    card.append(&delta);
    card.append(&detail);
    card
}

fn product_table(shell: &Rc<RefCell<Shell>>, products: &[Prod], full: bool) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("bf-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("bf-thead");
    head.append(&cell("", 48, false));
    if full {
        let name = sort_header(shell, "Product", SortKey::Name, 0);
        name.set_hexpand(true);
        head.append(&name);
        head.append(&sort_header(shell, "Times", SortKey::Times, 64));
        head.append(&cell("Units", 72, true));
        head.append(&sort_header(shell, "Frequency", SortKey::Frequency, 120));
        head.append(&sort_header(shell, "Likely", SortKey::Likely, 64));
        head.append(&sort_header(shell, "Spent", SortKey::Spent, 110));
    } else {
        let name = cell("Product", 0, false);
        name.set_hexpand(true);
        head.append(&name);
        head.append(&cell("Times", 64, true));
        head.append(&cell("Spent", 110, true));
    }
    card.append(&head);
    if products.is_empty() {
        let empty = gtk::Label::new(Some("No invoices yet."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        empty.set_margin_top(12);
        card.append(&empty);
        return card;
    }
    for product in products {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("bf-row");
        let thumb = gtk::Label::new(Some(&initial(&product.name)));
        thumb.add_css_class("bf-thumb");
        thumb.set_size_request(48, 48);
        thumb.set_xalign(0.5);
        thumb.set_yalign(0.5);
        row.append(&thumb);
        let name = cell(&product.name, 0, false);
        name.set_hexpand(true);
        row.append(&name);
        row.append(&cell(&product.times.to_string(), 64, true));
        if full {
            row.append(&cell(&qty_text(product.qty), 72, true));
            row.append(&cell(&frequency_label(product), 120, false));
            row.append(&cell(&product.likely.to_string(), 64, true));
        }
        row.append(&cell(&format!("AED {:.2}", product.spent), 110, true));
        let click = gtk::GestureClick::new();
        let shell_click = shell.clone();
        let product_name = product.name.clone();
        click.connect_released(move |_, _, _, _| {
            let mut shell = shell_click.borrow_mut();
            shell.compare_for.clear();
            shell.page = Page::Product(product_name.clone());
            drop(shell);
            schedule(&shell_click);
        });
        row.add_controller(click);
        card.append(&row);
    }
    card
}

fn stores_page(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.append(&heading_mark("YOUR ", "STORES"));
    let lead = gtk::Label::new(Some(
        "Every store can be searched and compared. Open a store to link it, or to change the cart it holds.",
    ));
    lead.add_css_class("bf-muted");
    wrap_label(&lead);
    page.append(&lead);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(12);
    grid.set_column_homogeneous(true);
    let links = shell.borrow().core.list_store_links().unwrap_or_default();
    for (index, id) in StoreId::all().into_iter().enumerate() {
        let linked = links.iter().find(|link| link.id == id);
        let card = store_card(
            shell,
            id,
            linked.and_then(|link| link.username.clone()),
            linked.map(|link| link.linked).unwrap_or(false),
        );
        grid.attach(&card, (index % 2) as i32, (index / 2) as i32, 1, 1);
    }
    page.append(&grid);
    page
}

fn store_card(
    shell: &Rc<RefCell<Shell>>,
    id: StoreId,
    username: Option<String>,
    linked: bool,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
    card.add_css_class("bf-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let logo = gtk::Label::new(Some(store_mark(id)));
    logo.add_css_class("bf-logo");
    logo.add_css_class(logo_class(id));
    logo.set_size_request(64, 64);
    logo.set_xalign(0.5);
    logo.set_yalign(0.5);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Label::new(Some(&store_label(id).to_uppercase()));
    name.add_css_class("bf-brand");
    name.set_xalign(0.0);
    let pill = gtk::Label::new(Some("ON"));
    pill.add_css_class("bf-pill");
    pill.add_css_class("on");
    name_row.append(&name);
    name_row.append(&pill);
    let status = if linked {
        let text = match username {
            Some(user) if !user.is_empty() => format!("Linked · {user}"),
            _ => "Linked".into(),
        };
        let label = gtk::Label::new(Some(&text));
        label.add_css_class("bf-ok");
        label.set_xalign(0.0);
        label
    } else {
        let label = gtk::Label::new(Some("Not linked"));
        label.add_css_class("bf-muted");
        label.set_xalign(0.0);
        label
    };
    text.append(&name_row);
    text.append(&status);
    head.append(&logo);
    head.append(&text);
    card.append(&head);
    card.append(&caps_row(id, linked));
    let go = gtk::Label::new(Some(&format!("OPEN {} →", store_label(id).to_uppercase())));
    go.add_css_class("bf-go");
    go.set_xalign(0.0);
    card.append(&go);
    let click = gtk::GestureClick::new();
    let shell_click = shell.clone();
    click.connect_released(move |_, _, _, _| {
        shell_click.borrow_mut().page = Page::Store(id);
        schedule(&shell_click);
    });
    card.add_controller(click);
    card
}

fn caps_row(id: StoreId, linked: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (label, on) in [
        ("SEARCH", true),
        ("COMPARE", true),
        ("CART", true),
        ("CHECKOUT", id.remote_checkout()),
        ("RECEIPTS", true),
        ("LOGIN", linked),
    ] {
        let cap = gtk::Label::new(Some(label));
        cap.add_css_class("bf-cap");
        if !on {
            cap.add_css_class("off");
        }
        row.append(&cap);
    }
    row
}

fn store_page(shell: &Rc<RefCell<Shell>>, id: StoreId) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let back = gtk::Button::with_label("← ALL STORES");
    back.add_css_class("bf-ghost");
    back.set_halign(gtk::Align::Start);
    let shell_back = shell.clone();
    back.connect_clicked(move |_| {
        shell_back.borrow_mut().page = Page::Stores;
        schedule(&shell_back);
    });
    page.append(&back);

    let links = shell.borrow().core.list_store_links().unwrap_or_default();
    let link = links.into_iter().find(|link| link.id == id);
    let linked = link.as_ref().map(|link| link.linked).unwrap_or(false);
    let username = link.as_ref().and_then(|link| link.username.clone());

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let logo = gtk::Label::new(Some(store_mark(id)));
    logo.add_css_class("bf-logo");
    logo.add_css_class(logo_class(id));
    logo.set_size_request(64, 64);
    logo.set_xalign(0.5);
    logo.set_yalign(0.5);
    let title = heading(&store_label(id).to_uppercase());
    head.append(&logo);
    head.append(&title);
    page.append(&head);
    page.append(&caps_row(id, linked));

    let form = gtk::Box::new(gtk::Orientation::Vertical, 8);
    form.add_css_class("bf-card");
    let user = gtk::Entry::new();
    user.set_placeholder_text(Some("Username"));
    if let Some(name) = username {
        user.set_text(&name);
    }
    let password = gtk::PasswordEntry::new();
    password.set_placeholder_text(Some("Password"));
    form.append(&field("EMAIL", &user));
    form.append(&field("PASSWORD", &password));
    let hint = gtk::Label::new(Some(
        "Search works without a sign-in. Sign-in is the store's own login, used for its cart.",
    ));
    hint.add_css_class("bf-muted");
    wrap_label(&hint);
    form.append(&hint);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let save = gtk::Button::with_label("SAVE");
    let remove = gtk::Button::with_label("REMOVE");
    remove.add_css_class("bf-ghost");
    let core = shell.borrow().core.clone();
    let toast = shell.borrow().toast.clone();
    let shell_save = shell.clone();
    save.connect_clicked(move |_| {
        let username = user.text().trim().to_string();
        let password = password.text().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let registry = Registry::live();
            let _ = tx.send(registry.sign_in(id, &username, &password));
        });
        let core = core.clone();
        let toast = toast.clone();
        let shell_save = shell_save.clone();
        glib::idle_add_local(move || match rx.try_recv() {
            Ok(Ok(secrets)) => {
                match core.link_store(id, &secrets) {
                    Ok(()) => {
                        toast.add_toast(adw::Toast::new(&format!("{} saved", store_label(id))))
                    }
                    Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
                }
                shell_save.borrow_mut().page = Page::Store(id);
                schedule(&shell_save);
                ControlFlow::Break
            }
            Ok(Err(err)) => {
                toast.add_toast(adw::Toast::new(&err.detail));
                ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => ControlFlow::Break,
        });
    });
    let core = shell.borrow().core.clone();
    let toast = shell.borrow().toast.clone();
    let shell_remove = shell.clone();
    remove.connect_clicked(move |_| {
        match core.unlink_store(id) {
            Ok(()) => toast.add_toast(adw::Toast::new(&format!("{} removed", store_label(id)))),
            Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
        }
        shell_remove.borrow_mut().page = Page::Store(id);
        schedule(&shell_remove);
    });
    actions.append(&save);
    actions.append(&remove);
    form.append(&actions);
    page.append(&form);
    page.append(&cart_card(shell, id));
    page
}

fn cart_card(shell: &Rc<RefCell<Shell>>, id: StoreId) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("bf-card");
    let title = gtk::Label::new(Some("CART"));
    title.add_css_class("bf-kicker");
    title.set_xalign(0.0);
    card.append(&title);
    let cart = shell.borrow().core.cart(id).ok();
    let lines = cart.map(|cart| cart.lines).unwrap_or_default();
    if lines.is_empty() {
        let empty = gtk::Label::new(Some("This cart is empty."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        card.append(&empty);
    }
    for line in &lines {
        card.append(&cart_line(shell, id, line));
    }
    let add = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let name = gtk::Entry::new();
    name.set_placeholder_text(Some("Product name"));
    name.set_hexpand(true);
    let qty = gtk::SpinButton::with_range(1.0, 99.0, 1.0);
    let price = gtk::Entry::new();
    price.set_placeholder_text(Some("Price"));
    price.set_width_request(80);
    let button = gtk::Button::with_label("ADD");
    let core = shell.borrow().core.clone();
    let toast = shell.borrow().toast.clone();
    let shell_add = shell.clone();
    let name_for_add = name.clone();
    let qty_for_add = qty.clone();
    let price_for_add = price.clone();
    button.connect_clicked(move |_| {
        let price_text = price_for_add.text().to_string();
        let unit_price = if price_text.trim().is_empty() {
            None
        } else {
            match price_text.trim().parse::<f64>() {
                Ok(value) => Some(value),
                Err(_) => {
                    toast.add_toast(adw::Toast::new("Price must be a number"));
                    return;
                }
            }
        };
        match core.cart_add(
            id,
            "",
            &name_for_add.text(),
            qty_for_add.value(),
            "piece",
            unit_price,
            "AED",
        ) {
            Ok(_) => {}
            Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
        }
        shell_add.borrow_mut().page = Page::Store(id);
        schedule(&shell_add);
    });
    add.append(&name);
    add.append(&qty);
    add.append(&price);
    add.append(&button);
    card.append(&add);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let checkout = gtk::Button::with_label("PREPARE CHECKOUT");
    let clear = gtk::Button::with_label("CLEAR CART");
    clear.add_css_class("bf-ghost");
    let shell_checkout = shell.clone();
    checkout.connect_clicked(move |_| prepare_checkout(&shell_checkout, id));
    let shell_clear = shell.clone();
    clear.connect_clicked(move |_| {
        let result = shell_clear.borrow().core.cart_clear(id);
        if let Err(err) = result {
            shell_clear
                .borrow()
                .toast
                .add_toast(adw::Toast::new(&err.to_string()));
        }
        shell_clear.borrow_mut().page = Page::Store(id);
        schedule(&shell_clear);
    });
    actions.append(&checkout);
    actions.append(&clear);
    card.append(&actions);
    let note = gtk::Label::new(Some(&shell.borrow().checkout_note));
    note.set_selectable(true);
    wrap_label(&note);
    card.append(&note);
    card
}

fn cart_line(shell: &Rc<RefCell<Shell>>, id: StoreId, line: &CartLine) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("bf-row");
    let price = line
        .unit_price
        .map(|price| format!("{price:.2} {}", line.currency))
        .unwrap_or_else(|| line.currency.clone());
    let label = gtk::Label::new(Some(&format!(
        "{}   ·   {} × {}   ·   {price}",
        line.name, line.qty, line.unit
    )));
    label.set_hexpand(true);
    label.set_xalign(0.0);
    let remove = gtk::Button::with_label("REMOVE");
    remove.add_css_class("bf-ghost");
    let core = shell.borrow().core.clone();
    let toast = shell.borrow().toast.clone();
    let line_id = line.line_id.clone();
    let shell_after = shell.clone();
    remove.connect_clicked(move |_| {
        if let Err(err) = core.cart_remove(id, &line_id) {
            toast.add_toast(adw::Toast::new(&err.to_string()));
        }
        shell_after.borrow_mut().page = Page::Store(id);
        schedule(&shell_after);
    });
    row.append(&label);
    row.append(&remove);
    row
}

fn prepare_checkout(shell: &Rc<RefCell<Shell>>, id: StoreId) {
    let (core, backend) = {
        let shell = shell.borrow();
        (shell.core.clone(), shell.backend.clone())
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(core.checkout(id, backend.as_ref()));
    });
    let shell = shell.clone();
    glib::idle_add_local(move || match rx.try_recv() {
        Ok(result) => {
            let note = match result {
                Ok(record) => {
                    let url = record.url.unwrap_or_default();
                    shell
                        .borrow()
                        .toast
                        .add_toast(adw::Toast::new("Checkout prepared"));
                    format!("{} — {}\n{url}", record.status, record.note)
                }
                Err(err) => err.to_string(),
            };
            shell.borrow_mut().checkout_note = note;
            shell.borrow_mut().page = Page::Store(id);
            schedule(&shell);
            ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => ControlFlow::Continue,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => ControlFlow::Break,
    });
}

fn product_page(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let name = match &shell.borrow().page {
        Page::Product(name) => name.clone(),
        _ => String::new(),
    };
    let should_search = shell.borrow().compare_for != name && !name.is_empty();
    if should_search {
        shell.borrow_mut().compare_for = name.clone();
        shell.borrow_mut().compare.clear();
        shell.borrow_mut().compare_note = "Searching…".into();
        let (core, backend) = {
            let shell = shell.borrow();
            (shell.core.clone(), shell.backend.clone())
        };
        let query = name.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(core.search(&query, backend.as_ref()));
        });
        let shell_done = shell.clone();
        let expected = name.clone();
        glib::idle_add_local(move || match rx.try_recv() {
            Ok(result) => {
                let mut shell = shell_done.borrow_mut();
                match result {
                    Ok(comparison) => {
                        let count = comparison.offers.len();
                        shell.compare = comparison.offers;
                        shell.compare_note = if count == 0 {
                            "No matching products in live results or receipts.".into()
                        } else {
                            format!("{count} offers")
                        };
                    }
                    Err(err) => shell.compare_note = err.to_string(),
                }
                let still = shell.page == Page::Product(expected.clone());
                drop(shell);
                if still {
                    schedule(&shell_done);
                }
                ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => ControlFlow::Break,
        });
    }

    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let back = gtk::Button::with_label("← PURCHASES");
    back.add_css_class("bf-ghost");
    back.set_halign(gtk::Align::Start);
    let shell_back = shell.clone();
    back.connect_clicked(move |_| {
        shell_back.borrow_mut().page = Page::Purchases;
        schedule(&shell_back);
    });
    page.append(&back);

    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let thumb = gtk::Label::new(Some(&initial(&name)));
    thumb.add_css_class("bf-thumb");
    thumb.set_size_request(72, 72);
    thumb.set_xalign(0.5);
    thumb.set_yalign(0.5);
    hero.append(&thumb);
    hero.append(&heading(&name.to_uppercase()));
    page.append(&hero);

    let view = load_view(shell);
    let product = view
        .products
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(&name));
    let stats = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let (freq, likely, times) = match product {
        Some(product) => (
            frequency_label(product),
            product.likely.to_string(),
            product.times.to_string(),
        ),
        None => ("—".into(), "0".into(), "0".into()),
    };
    stats.append(&kpi(&freq, "FREQUENCY"));
    stats.append(&kpi(&likely, "LIKELY"));
    stats.append(&kpi(&times, "BUYS"));
    page.append(&stats);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("bf-card");
    let kicker = gtk::Label::new(Some("COMPARE"));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    card.append(&kicker);
    let note = gtk::Label::new(Some(&shell.borrow().compare_note));
    note.add_css_class("bf-muted");
    note.set_xalign(0.0);
    wrap_label(&note);
    card.append(&note);
    let offers = shell.borrow().compare.clone();
    for offer in offers {
        card.append(&offer_row(shell, &offer));
    }
    page.append(&card);
    page
}

fn offer_row(shell: &Rc<RefCell<Shell>>, offer: &Offer) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("bf-row");
    let source = match offer.source {
        omabring_core::OfferSource::Live => "live",
        omabring_core::OfferSource::History => "receipt",
    };
    let store_name = cell(&store_label(offer.store).to_uppercase(), 140, false);
    let product = cell(&offer.name, 0, false);
    product.set_hexpand(true);
    let price_label = cell(
        &format!(
            "{:.2} {} / {} · {source}",
            offer.price, offer.currency, offer.unit
        ),
        220,
        true,
    );
    let add = gtk::Button::with_label("ADD");
    let core = shell.borrow().core.clone();
    let toast = shell.borrow().toast.clone();
    let store = offer.store;
    let product_id = offer.product_id.clone();
    let offer_name = offer.name.clone();
    let unit = offer.unit.clone();
    let price = offer.price;
    let currency = offer.currency.clone();
    add.connect_clicked(move |_| {
        match core.cart_add(
            store,
            &product_id,
            &offer_name,
            1.0,
            &unit,
            Some(price),
            &currency,
        ) {
            Ok(_) => toast.add_toast(adw::Toast::new(&format!(
                "{offer_name} added to {}",
                store_label(store)
            ))),
            Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
        }
    });
    row.append(&store_name);
    row.append(&product);
    row.append(&price_label);
    row.append(&add);
    row
}

fn settings_page(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.append(&heading_mark("", "SETTINGS"));
    let lead = gtk::Label::new(Some(
        "The local service on this machine. Allow or deny a client in this window.",
    ));
    lead.add_css_class("bf-muted");
    wrap_label(&lead);
    page.append(&lead);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("bf-card");
    card.add_css_class("bf-dashed");
    let title = gtk::Label::new(Some("LOCAL SERVICE"));
    title.add_css_class("bf-h1-sm");
    title.set_xalign(0.0);
    let address = omabring_core::default_bind_addr();
    let url = gtk::Label::new(Some(&format!("http://{address}")));
    url.add_css_class("bf-delta");
    url.set_xalign(0.0);
    url.set_selectable(true);
    let note = gtk::Label::new(Some(
        "An approved client uses the same carts, receipts, search, and forecasts. Approve or deny each client in this window.",
    ));
    wrap_label(&note);
    card.append(&title);
    card.append(&url);
    card.append(&shell.borrow().service_label);
    card.append(&note);
    page.append(&card);
    page
}

fn import_card(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("bf-card");
    card.add_css_class("bf-dashed");
    let hint = gtk::Label::new(Some("Paste a JSON or CSV receipt"));
    hint.add_css_class("bf-muted");
    hint.set_xalign(0.0);
    let view = gtk::TextView::new();
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&view)
        .min_content_height(120)
        .build();
    let button = gtk::Button::with_label("IMPORT");
    let shell_click = shell.clone();
    button.connect_clicked(move |_| {
        let buffer = view.buffer();
        let document = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let shell_ref = shell_click.borrow();
        match shell_ref.core.import_document(&document) {
            Ok(saved) => {
                shell_ref.toast.add_toast(adw::Toast::new(&format!(
                    "Imported {} receipt(s)",
                    saved.len()
                )));
                drop(shell_ref);
                schedule(&shell_click);
            }
            Err(err) => shell_ref.toast.add_toast(adw::Toast::new(&err.to_string())),
        }
    });
    card.append(&hint);
    card.append(&scroll);
    card.append(&button);
    let invoices = shell.borrow().core.list_invoices().unwrap_or_default();
    for invoice in invoices.iter().rev().take(8) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&format!(
            "{} · {} · {} · {:.2} {}",
            store_label(invoice.store),
            invoice.invoice_no,
            invoice.purchased_on,
            invoice.total,
            invoice.currency
        )));
        label.set_hexpand(true);
        label.set_xalign(0.0);
        let remove = gtk::Button::with_label("DELETE");
        remove.add_css_class("bf-ghost");
        let core = shell.borrow().core.clone();
        let toast = shell.borrow().toast.clone();
        let id = invoice.id;
        let open_id = id;
        let shell_after = shell.clone();
        remove.connect_clicked(move |_| {
            if let Err(err) = core.delete_invoice(id) {
                toast.add_toast(adw::Toast::new(&err.to_string()));
                return;
            }
            schedule(&shell_after);
        });
        let open = gtk::GestureClick::new();
        let shell_open = shell.clone();
        open.connect_released(move |_, _, _, _| {
            shell_open.borrow_mut().page = Page::Receipt(open_id);
            schedule(&shell_open);
        });
        label.add_controller(open);
        row.append(&label);
        row.append(&remove);
        card.append(&row);
    }
    card
}

fn receipt_page(shell: &Rc<RefCell<Shell>>, id: i64) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let back = gtk::Button::with_label("PURCHASES");
    back.add_css_class("bf-ghost");
    back.set_halign(gtk::Align::Start);
    let shell_back = shell.clone();
    back.connect_clicked(move |_| select_page(&shell_back, Page::Purchases));
    page.append(&back);

    let invoices = shell.borrow().core.list_invoices().unwrap_or_default();
    let invoice = if id == 0 {
        invoices.last().cloned()
    } else {
        invoices.into_iter().find(|invoice| invoice.id == id)
    };
    let Some(invoice) = invoice else {
        let empty = gtk::Label::new(Some("No receipt on this machine."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        page.append(&empty);
        return page;
    };

    let slip = gtk::Box::new(gtk::Orientation::Vertical, 6);
    slip.add_css_class("bf-card");
    let title = heading(&store_label(invoice.store).to_uppercase());
    title.add_css_class("bf-h1-sm");
    let meta = gtk::Label::new(Some(&format!(
        "{} · #{}",
        invoice.purchased_on, invoice.invoice_no
    )));
    meta.add_css_class("bf-muted");
    meta.set_xalign(0.0);
    slip.append(&title);
    slip.append(&meta);

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("bf-thead");
    head.append(&cell("ITEM", 0, false));
    let qty = cell("QTY", 72, true);
    qty.set_hexpand(false);
    let amount = cell("AED", 96, true);
    head.append(&qty);
    head.append(&amount);
    slip.append(&head);
    for line in &invoice.lines {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("bf-row");
        let name = cell(&line.name, 0, false);
        name.set_hexpand(true);
        row.append(&name);
        row.append(&cell(&format!("{:.2}", line.qty), 72, true));
        row.append(&cell(&format!("{:.2}", line.line_total), 96, true));
        slip.append(&row);
    }
    let total = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some("TOTAL"));
    label.add_css_class("bf-brand");
    label.set_hexpand(true);
    label.set_xalign(0.0);
    let figure = gtk::Label::new(Some(&format!("{} {:.2}", invoice.currency, invoice.total)));
    figure.add_css_class("bf-delta");
    total.append(&label);
    total.append(&figure);
    slip.append(&total);
    page.append(&slip);
    page
}

struct ViewData {
    start: NaiveDate,
    end: NaiveDate,
    total: f64,
    average: f64,
    receipts: usize,
    buckets: Vec<(String, f64)>,
    products: Vec<Prod>,
}

fn load_view(shell: &Rc<RefCell<Shell>>) -> ViewData {
    let shell_ref = shell.borrow();
    let today = chrono::Utc::now().date_naive();
    let invoices = shell_ref.core.list_invoices().unwrap_or_default();
    let (start, end) = window_for(&shell_ref, &invoices, today);
    let store_filter = shell_ref.store_filter;
    let grain = shell_ref.grain;
    let sort = shell_ref.sort;
    drop(shell_ref);
    let likely = likely_map(shell, today);
    let filtered: Vec<&InvoiceSummary> = invoices
        .iter()
        .filter(|invoice| {
            let Some(date) = parse_date(&invoice.purchased_on) else {
                return false;
            };
            date >= start && date <= end
        })
        .filter(|invoice| store_filter.is_none() || store_filter == Some(invoice.store))
        .collect();
    let mut by_day: HashMap<NaiveDate, f64> = HashMap::new();
    for invoice in &filtered {
        if let Some(date) = parse_date(&invoice.purchased_on) {
            *by_day.entry(date).or_default() += invoice.total;
        }
    }
    let buckets = match grain {
        Grain::Daily => daily_buckets(start, end, &by_day),
        Grain::Weekly => span_buckets(start, end, &by_day, 7, "W"),
        Grain::Monthly => month_buckets(start, end, &by_day),
        Grain::Yearly => year_buckets(start, end, &by_day),
    };
    let total: f64 = buckets.iter().map(|(_, n)| n).sum();
    let periods = buckets.len().max(1) as f64;
    let products = products_from(&filtered, &likely, sort);
    ViewData {
        start,
        end,
        total,
        average: total / periods,
        receipts: filtered.len(),
        buckets,
        products,
    }
}

fn likely_map(shell: &Rc<RefCell<Shell>>, today: NaiveDate) -> HashMap<String, u8> {
    let mut map = HashMap::new();
    if let Ok(items) = shell.borrow().core.forecast(today) {
        for item in items {
            map.insert(item.name.to_lowercase(), item.likely);
        }
    }
    map
}

fn window_for(
    shell: &Shell,
    invoices: &[InvoiceSummary],
    today: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    if shell.custom {
        if let (Ok(start), Ok(end)) = (
            NaiveDate::parse_from_str(shell.custom_start.trim(), "%Y-%m-%d"),
            NaiveDate::parse_from_str(shell.custom_end.trim(), "%Y-%m-%d"),
        ) {
            return if start <= end {
                (start, end)
            } else {
                (end, start)
            };
        }
    }
    if shell.range_days <= 0 {
        let start = invoices
            .iter()
            .filter_map(|invoice| parse_date(&invoice.purchased_on))
            .min()
            .unwrap_or(today);
        return (start, today);
    }
    (today - Duration::days(shell.range_days - 1), today)
}

fn daily_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        out.push((
            cursor.format("%m-%d").to_string(),
            by_day.get(&cursor).copied().unwrap_or(0.0),
        ));
        cursor += Duration::days(1);
        if out.len() > 400 {
            break;
        }
    }
    out
}

fn span_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
    days: i64,
    prefix: &str,
) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut cursor = start;
    let mut index = 1;
    while cursor <= end {
        let stop = (cursor + Duration::days(days - 1)).min(end);
        let mut sum = 0.0;
        let mut day = cursor;
        while day <= stop {
            sum += by_day.get(&day).copied().unwrap_or(0.0);
            day += Duration::days(1);
        }
        out.push((format!("{prefix}{index}"), sum));
        index += 1;
        cursor = stop + Duration::days(1);
    }
    out
}

fn month_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        let key = cursor.format("%Y-%m").to_string();
        let add = by_day.get(&cursor).copied().unwrap_or(0.0);
        if let Some(last) = out.last_mut() {
            if last.0 == key {
                last.1 += add;
            } else {
                out.push((key, add));
            }
        } else {
            out.push((key, add));
        }
        cursor += Duration::days(1);
    }
    out
}

fn year_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        let key = cursor.format("%Y").to_string();
        let add = by_day.get(&cursor).copied().unwrap_or(0.0);
        if let Some(last) = out.last_mut() {
            if last.0 == key {
                last.1 += add;
            } else {
                out.push((key, add));
            }
        } else {
            out.push((key, add));
        }
        cursor += Duration::days(1);
    }
    out
}

fn products_from(
    invoices: &[&InvoiceSummary],
    likely: &HashMap<String, u8>,
    sort: SortKey,
) -> Vec<Prod> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Prod> = HashMap::new();
    for invoice in invoices {
        let Some(date) = parse_date(&invoice.purchased_on) else {
            continue;
        };
        for line in &invoice.lines {
            let key = line.name.to_lowercase();
            if !map.contains_key(&key) {
                order.push(key.clone());
                map.insert(
                    key.clone(),
                    Prod {
                        name: line.name.clone(),
                        times: 0,
                        qty: 0.0,
                        spent: 0.0,
                        first: date,
                        last: date,
                        first_price: line.unit_price,
                        last_price: line.unit_price,
                        likely: likely.get(&key).copied().unwrap_or(0),
                    },
                );
            }
            let prod = map.get_mut(&key).expect("just inserted");
            prod.times += 1;
            prod.qty += line.qty;
            prod.spent += line.line_total;
            if date < prod.first {
                prod.first = date;
                prod.first_price = line.unit_price;
            }
            if date >= prod.last {
                prod.last = date;
                prod.last_price = line.unit_price;
            }
            prod.likely = likely.get(&key).copied().unwrap_or(0);
        }
    }
    let mut products: Vec<Prod> = order
        .into_iter()
        .filter_map(|key| map.remove(&key))
        .collect();
    products.sort_by(|a, b| match sort {
        SortKey::Spent => b
            .spent
            .partial_cmp(&a.spent)
            .unwrap_or(std::cmp::Ordering::Equal),
        SortKey::Frequency => freq_days(a)
            .partial_cmp(&freq_days(b))
            .unwrap_or(std::cmp::Ordering::Equal),
        SortKey::Likely => b.likely.cmp(&a.likely),
        SortKey::Times => b.times.cmp(&a.times),
        SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    products
}

fn freq_days(product: &Prod) -> f64 {
    if product.times < 2 {
        return f64::MAX;
    }
    (product.last - product.first).num_days() as f64 / (product.times - 1) as f64
}

fn frequency_label(product: &Prod) -> String {
    if product.times < 2 {
        "once".into()
    } else {
        format!("every {:.0} days", freq_days(product))
    }
}

fn parse_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok()
}

fn heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("bf-h1");
    label.set_xalign(0.0);
    label.set_text(text);
    label
}

fn heading_mark(before: &str, word: &str) -> gtk::Label {
    let label = heading("");
    let markup = format!(
        "{}<span underline=\"single\">{}</span>",
        glib::markup_escape_text(before),
        glib::markup_escape_text(word)
    );
    label.set_markup(&markup);
    label
}

fn kpi(value: &str, caption: &str) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let value = gtk::Label::new(Some(value));
    value.add_css_class("bf-brand");
    value.set_xalign(0.0);
    let caption = gtk::Label::new(Some(caption));
    caption.add_css_class("bf-kicker");
    caption.set_xalign(0.0);
    box_.append(&value);
    box_.append(&caption);
    box_
}

fn sort_header(shell: &Rc<RefCell<Shell>>, text: &str, key: SortKey, width: i32) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("bf-th");
    if shell.borrow().sort == key {
        button.add_css_class("on");
    }
    if width > 0 {
        button.set_width_request(width);
    }
    button.set_halign(gtk::Align::Start);
    let shell_click = shell.clone();
    button.connect_clicked(move |_| {
        shell_click.borrow_mut().sort = key;
        schedule(&shell_click);
    });
    button
}

fn chip(text: &str, on: bool) -> gtk::Button {
    let button = gtk::Button::with_label(text);
    button.add_css_class("bf-chip");
    if on {
        button.add_css_class("on");
    }
    button
}

fn cell(text: &str, width: i32, end: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(if end { 1.0 } else { 0.0 });
    if width > 0 {
        label.set_width_request(width);
    }
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

fn field(title: &str, entry: &impl IsA<gtk::Widget>) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("bf-field");
    label.set_xalign(0.0);
    box_.append(&label);
    box_.append(entry);
    box_
}

fn wrap_label(label: &gtk::Label) {
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(72);
    label.set_xalign(0.0);
}

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "·".into())
}

fn qty_text(qty: f64) -> String {
    if (qty - qty.round()).abs() < 0.001 {
        format!("{}", qty.round() as i64)
    } else {
        format!("{qty:.1}")
    }
}

fn grain_word(grain: Grain) -> &'static str {
    match grain {
        Grain::Daily => "DAILY",
        Grain::Weekly => "WEEKLY",
        Grain::Monthly => "MONTHLY",
        Grain::Yearly => "YEARLY",
    }
}

fn period_unit(grain: Grain, count: usize) -> String {
    let one = count == 1;
    match grain {
        Grain::Daily => {
            if one {
                "DAY"
            } else {
                "DAYS"
            }
        }
        Grain::Weekly => {
            if one {
                "WEEK"
            } else {
                "WEEKS"
            }
        }
        Grain::Monthly => {
            if one {
                "MONTH"
            } else {
                "MONTHS"
            }
        }
        Grain::Yearly => {
            if one {
                "YEAR"
            } else {
                "YEARS"
            }
        }
    }
    .into()
}

fn store_label(id: StoreId) -> &'static str {
    match id {
        StoreId::Carrefour => "Carrefour UAE",
        StoreId::UnionCoop => "Union Coop",
        StoreId::Grandiose => "Grandiose",
        StoreId::Waitrose => "Waitrose UAE",
        StoreId::Spinneys => "Spinneys UAE",
        StoreId::Mmi => "MMI",
        StoreId::AfricanEastern => "African & Eastern",
    }
}

fn store_chip_label(id: StoreId) -> &'static str {
    match id {
        StoreId::Carrefour => "Carrefour",
        StoreId::UnionCoop => "Union",
        StoreId::Grandiose => "Grandiose",
        StoreId::Waitrose => "Waitrose",
        StoreId::Spinneys => "Spinneys",
        StoreId::Mmi => "MMI",
        StoreId::AfricanEastern => "African",
    }
}

fn store_mark(id: StoreId) -> &'static str {
    match id {
        StoreId::Carrefour => "C",
        StoreId::UnionCoop => "U",
        StoreId::Grandiose => "G",
        StoreId::Waitrose => "W",
        StoreId::Spinneys => "S",
        StoreId::Mmi => "M",
        StoreId::AfricanEastern => "A",
    }
}

fn logo_class(id: StoreId) -> &'static str {
    match id {
        StoreId::Carrefour => "logo-carrefour",
        StoreId::UnionCoop => "logo-union",
        StoreId::Grandiose => "logo-grandiose",
        StoreId::Waitrose => "logo-waitrose",
        StoreId::Spinneys => "logo-spinneys",
        StoreId::Mmi => "logo-mmi",
        StoreId::AfricanEastern => "logo-african",
    }
}

fn initial_page() -> Page {
    page_from(std::env::var("OMABRING_PAGE").ok().as_deref())
}

fn seed_demo(core: &App) {
    if std::env::var("OMABRING_DEMO").ok().as_deref() != Some("1") {
        return;
    }
    let Ok(existing) = core.list_invoices() else {
        return;
    };
    if !existing.is_empty() {
        return;
    }
    let today = chrono::Utc::now().date_naive();
    let rows = [
        (1, "carrefour", "A-1001", "Milk", 2.0, 8.50),
        (3, "carrefour", "A-1002", "Bread", 1.0, 4.25),
        (6, "union_coop", "U-2001", "Eggs", 1.0, 12.00),
        (8, "grandiose", "G-3001", "Water", 6.0, 2.15),
        (12, "waitrose", "W-4001", "Milk", 1.0, 9.10),
        (15, "spinneys", "S-5001", "Rice", 1.0, 18.40),
        (18, "carrefour", "A-1003", "Milk", 1.0, 8.75),
        (22, "mmi", "M-6001", "Bread", 2.0, 3.90),
        (26, "african_eastern", "E-7001", "Coffee", 1.0, 32.00),
        (27, "union_coop", "U-2002", "Eggs", 1.0, 11.50),
    ];
    for (days_ago, store, invoice_no, name, qty, price) in rows {
        let date = today - Duration::days(days_ago);
        let document = format!(
            r#"{{"store":"{store}","invoice_no":"{invoice_no}","date":"{date}","currency":"AED","items":[{{"name":"{name}","qty":{qty},"unit":"piece","unit_price":{price}}}]}}"#
        );
        let _ = core.import_document(&document);
    }
    let _ = core.cart_add(
        StoreId::Carrefour,
        "milk",
        "Milk",
        1.0,
        "piece",
        Some(8.50),
        "AED",
    );
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
    let body = format!(
        "{} wants to use your carts, receipts, search, and forecasts.",
        glib::markup_escape_text(&request.client_name)
    );
    let dialog = adw::AlertDialog::new(Some("Allow a client to use OmaBring?"), Some(&body));
    dialog.add_css_class("omabring");
    dialog.add_response("deny", "Deny");
    dialog.add_response("allow", "Allow");
    dialog.set_response_appearance("allow", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("deny"));
    dialog.set_close_response("deny");
    let core = shell_ref.core.clone();
    let flag = shell_ref.consent_open.clone();
    let id = request.id;
    dialog.connect_response(None, move |_, response| {
        let allow = response == "allow";
        let _ = core.decide_authorization(&id, allow);
        flag.set(false);
    });
    dialog.present(Some(&shell_ref.window));
}

#[cfg(test)]
mod tests {
    use super::{alt_index, nav_slot, page_at, page_from, Page};
    use omabring_core::StoreId;

    #[test]
    fn window_opens_on_search() {
        assert!(matches!(page_from(None), Page::Search));
        assert!(matches!(page_from(Some("")), Page::Search));
        assert!(matches!(page_from(Some("search")), Page::Search));
        assert!(matches!(page_from(Some("dashboard")), Page::Dashboard));
        assert!(matches!(page_from(Some("receipts")), Page::Purchases));
        assert!(matches!(page_from(Some("service")), Page::Settings));
        assert!(matches!(page_from(Some("forecast")), Page::Forecast));
    }

    #[test]
    fn alt_numbers_follow_the_header() {
        assert_eq!(nav_slot(&Page::Search), 0);
        assert_eq!(nav_slot(&Page::Dashboard), 1);
        assert_eq!(nav_slot(&Page::Purchases), 2);
        assert_eq!(nav_slot(&Page::Product("Milk".into())), 2);
        assert_eq!(nav_slot(&Page::Stores), 3);
        assert_eq!(nav_slot(&Page::Store(StoreId::AfricanEastern)), 3);
        assert_eq!(nav_slot(&Page::Settings), 4);
        assert_eq!(nav_slot(&Page::Forecast), 5);
        assert!(matches!(page_at(0), Page::Search));
        assert!(matches!(page_at(5), Page::Forecast));
        assert!(matches!(page_at(6), Page::Search));
        assert_eq!(alt_index(gtk4::gdk::Key::_1), Some(0));
        assert_eq!(alt_index(gtk4::gdk::Key::KP_6), Some(5));
        assert_eq!(alt_index(gtk4::gdk::Key::a), None);
    }

    #[test]
    fn markup_escapes_the_store_ampersand() {
        let escaped = gtk4::glib::markup_escape_text("African & Eastern");
        assert_eq!(escaped.as_str(), "African &amp; Eastern");
    }
}
