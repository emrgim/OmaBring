#![allow(deprecated)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use chrono::{Datelike, Duration, NaiveDate};
use gtk::gio::{Cancellable, FileMonitorFlags};
use gtk::glib::{self, ControlFlow};
use gtk::prelude::FileExt;
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;
use omabring_core::{
    home_dir, junk_product_name, load_window_theme, theme_watch_paths, App, CartLine,
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
    Last,
    Likely,
    Times,
    Qty,
    Spent,
    Frequency,
    Name,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChartKind {
    Price,
    PriceByStore,
    QtyMonth,
    SpendMonth,
    SpendYear,
    StoreCompare,
    Cumulative,
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
    filters: gtk::Box,
    agent_button: gtk::Button,
    service_label: gtk::Label,
    page: Page,
    grain: Grain,
    range_days: i64,
    custom: bool,
    custom_start: String,
    custom_end: String,
    sort: SortKey,
    sort_desc: bool,
    shortage: bool,
    chart: ChartKind,
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
    bar_focus: Option<String>,
    vote_guard: Rc<Cell<bool>>,
    bar_hit: Rc<Cell<bool>>,
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
    let (width, height, forced) = crate::window_size();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("OmaBring")
        .default_width(width)
        .default_height(height)
        .build();
    if forced {
        window.set_size_request(width, height);
    }
    window.add_css_class("omabring");
    window.set_decorated(false);
    let provider = if css_disabled() {
        None
    } else {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        Some(provider)
    };

    let toast = adw::ToastOverlay::new();
    let shell = build_chrome(&window, &toast, core, backend);
    let root = chrome_root(&shell);
    toast.set_child(Some(&root));
    window.set_content(Some(&toast));
    render(&shell);
    install_keys(&window, &shell);
    if let Some(provider) = provider {
        follow_theme(provider);
    }
    window.present();
    if std::env::var("OMABRING_SHOT_SCROLL").ok().as_deref() == Some("1") {
        let window = window.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(700), move || {
            scroll_main(&window, 260.0);
            ControlFlow::Break
        });
    }
    let backup = shell.borrow().core.clone();
    std::thread::spawn(move || {
        let _ = backup.backup_if_due();
    });

    let shell_for_consent = shell.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(800), move || {
        poll_consent(&shell_for_consent);
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
    static THEME_MONITORS: RefCell<Vec<gtk::gio::FileMonitor>> = RefCell::new(Vec::new());
}

fn follow_theme(provider: gtk::CssProvider) {
    let css = Rc::new(RefCell::new(String::new()));
    refresh_theme(&provider, &css);
    let mut held = Vec::new();
    for path in theme_watch_paths(&home_dir()) {
        let file = gtk::gio::File::for_path(&path);
        let Ok(monitor) = file.monitor_directory(FileMonitorFlags::WATCH_MOVES, Cancellable::NONE)
        else {
            continue;
        };
        let provider = provider.clone();
        let css = css.clone();
        monitor.connect_changed(move |_, _, _, _| {
            refresh_theme(&provider, &css);
        });
        held.push(monitor);
    }
    THEME_MONITORS.with(|slot| {
        *slot.borrow_mut() = held;
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

    let (palette, css) = load_window_theme(&home_dir());
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
    let filters = gtk::Box::new(gtk::Orientation::Vertical, 2);
    filters.add_css_class("bf-pin");
    let agent_button = agent_button();
    let service = gtk::Label::new(Some(""));
    service.set_xalign(0.0);
    service.set_selectable(true);
    wrap_label(&service);
    let search_entry = gtk::Entry::new();
    search_entry.set_placeholder_text(Some("Product"));
    search_entry.set_hexpand(true);
    let page = initial_page(&core);
    let focus_query = matches!(page, Page::Search);
    let shell = Rc::new(RefCell::new(Shell {
        core,
        backend,
        window: window.clone(),
        toast: toast.clone(),
        content: gtk::Box::new(gtk::Orientation::Vertical, 4),
        tab_search: tab("Search"),
        tab_dash: tab("Dashboard"),
        tab_buy: tab("Purchases"),
        tab_stores: tab("Stores"),
        tab_settings: tab("Settings"),
        tab_forecast: tab("Forecast"),
        search_entry,
        filters,
        agent_button,
        service_label: service,
        page: page.clone(),
        grain: Grain::Weekly,
        range_days: 30,
        custom: false,
        custom_start: String::new(),
        custom_end: String::new(),
        sort: SortKey::Spent,
        sort_desc: true,
        shortage: false,
        chart: initial_chart(),
        checkout_note: "Checkout prepares the cart. Payment stays with the store.".into(),
        import_open: false,
        compare: Vec::new(),
        compare_note: String::new(),
        compare_for: String::new(),
        search_query: startup_query(),
        search_note: String::new(),
        search_offers: Vec::new(),
        search_started: String::new(),
        focus_nav: !focus_query,
        focus_query,
        consent_open: Rc::new(Cell::new(false)),
        bar_focus: None,
        vote_guard: Rc::new(Cell::new(false)),
        bar_hit: Rc::new(Cell::new(false)),
    }));
    let shell_entry = shell.clone();
    let entry = shell.borrow().search_entry.clone();
    entry.connect_activate(move |entry| run_search(&shell_entry, entry));
    let shell_agent = shell.clone();
    shell
        .borrow()
        .agent_button
        .connect_clicked(move |_| launch_agent(&shell_agent));
    shell
}

fn chrome_root(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("bf-header");
    header.set_margin_top(2);
    header.set_margin_bottom(2);
    header.set_margin_start(6);
    header.set_margin_end(6);

    let mark = gtk::Label::new(Some("OB"));
    mark.add_css_class("bf-mark");
    mark.set_xalign(0.5);
    mark.set_yalign(0.5);
    mark.set_size_request(18, 18);

    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tabs.add_css_class("bf-tabs");
    {
        let shell_ref = shell.borrow();
        tabs.append(&shell_ref.tab_dash);
        tabs.append(&shell_ref.tab_buy);
        tabs.append(&shell_ref.tab_stores);
        tabs.append(&shell_ref.tab_search);
        tabs.append(&shell_ref.tab_forecast);
        tabs.append(&shell_ref.tab_settings);
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
    header.append(&tabs);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    header.append(&spacer);
    header.append(&shell.borrow().agent_button);

    let scroll = gtk::ScrolledWindow::builder()
        .child(&shell.borrow().content)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    shell.borrow().content.set_hexpand(true);
    shell.borrow().content.set_halign(gtk::Align::Fill);
    shell.borrow().content.set_margin_top(4);
    shell.borrow().content.set_margin_bottom(6);
    shell.borrow().content.set_margin_start(6);
    shell.borrow().content.set_margin_end(6);
    scroll.set_propagate_natural_width(false);
    root.append(&header);
    root.append(&shell.borrow().filters);
    root.append(&scroll);
    let outside = gtk::GestureClick::new();
    outside.set_propagation_phase(gtk::PropagationPhase::Bubble);
    let shell_out = shell.clone();
    outside.connect_pressed(move |_, _, _, _| {
        let hit = shell_out.borrow().bar_hit.clone();
        if hit.replace(false) {
            return;
        }
        if shell_out.borrow().bar_focus.is_some() {
            shell_out.borrow_mut().bar_focus = None;
            schedule(&shell_out);
        }
    });
    root.add_controller(outside);
    root
}

fn scroll_main(window: &adw::ApplicationWindow, by: f64) {
    fn walk(widget: &gtk::Widget, best: &mut Option<(f64, gtk::ScrolledWindow)>) {
        if let Some(scroll) = widget.downcast_ref::<gtk::ScrolledWindow>() {
            let upper = scroll.vadjustment().upper();
            let replace = best
                .as_ref()
                .map(|(score, _)| upper > *score)
                .unwrap_or(true);
            if replace {
                *best = Some((upper, scroll.clone()));
            }
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            walk(&current, best);
            child = current.next_sibling();
        }
    }
    let Some(child) = window.child() else {
        return;
    };
    let mut best = None;
    walk(&child, &mut best);
    if let Some((_, scroll)) = best {
        let adj = scroll.vadjustment();
        let next = (adj.value() + by).min((adj.upper() - adj.page_size()).max(0.0));
        adj.set_value(next);
    }
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
    paint_filters(shell);
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
        Page::Dashboard => 0,
        Page::Purchases | Page::Product(_) | Page::Receipt(_) => 1,
        Page::Stores | Page::Store(_) => 2,
        Page::Search => 3,
        Page::Forecast => 4,
        Page::Settings => 5,
    }
}

fn page_at(index: usize) -> Page {
    match index % 6 {
        0 => Page::Dashboard,
        1 => Page::Purchases,
        2 => Page::Stores,
        3 => Page::Search,
        4 => Page::Forecast,
        _ => Page::Settings,
    }
}

fn page_from(value: Option<&str>) -> Page {
    let value = value.map(|raw| raw.trim().to_ascii_lowercase());
    match value.as_deref() {
        Some("search") => Page::Search,
        Some("dashboard") | Some("home") | Some("spend") => Page::Dashboard,
        Some("purchases") | Some("receipts") => Page::Purchases,
        Some("stores") | Some("carts") => Page::Stores,
        Some("settings") | Some("service") => Page::Settings,
        Some("forecast") => Page::Forecast,
        Some("store") => Page::Store(StoreId::Carrefour),
        Some("product") => Page::Product(String::new()),
        Some("receipt") => Page::Receipt(0),
        _ => Page::Dashboard,
    }
}

fn chart_from(value: Option<&str>) -> Option<ChartKind> {
    let value = value.map(|raw| raw.trim().to_ascii_lowercase());
    match value.as_deref() {
        Some("price") => Some(ChartKind::Price),
        Some("price-by-store") | Some("price_by_store") | Some("by-store") => {
            Some(ChartKind::PriceByStore)
        }
        Some("qty-month") | Some("qty") => Some(ChartKind::QtyMonth),
        Some("spend-month") | Some("spend") => Some(ChartKind::SpendMonth),
        Some("spend-year") | Some("year") => Some(ChartKind::SpendYear),
        Some("store-compare") | Some("compare") => Some(ChartKind::StoreCompare),
        Some("cumulative") | Some("total") => Some(ChartKind::Cumulative),
        _ => None,
    }
}

fn initial_chart() -> ChartKind {
    chart_from(std::env::var("OMABRING_CHART").ok().as_deref()).unwrap_or(ChartKind::Price)
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
        if key == gtk::gdk::Key::Escape && shell.borrow().bar_focus.is_some() {
            shell.borrow_mut().bar_focus = None;
            schedule(&shell);
            return glib::Propagation::Stop;
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
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
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

    let find = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page.append(&heading_mark("", "FORECAST"));
    let today = chrono::Utc::now().date_naive();
    let items: Vec<_> = shell
        .borrow()
        .core
        .forecast(today)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| !junk_product_name(&item.name))
        .collect();
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("bf-card");
    card.add_css_class("bf-dash");
    let lead = gtk::Label::new(Some(
        "What to buy again, from receipts already on this machine.",
    ));
    lead.add_css_class("bf-muted");
    lead.set_xalign(0.0);
    wrap_label(&lead);
    card.append(&lead);
    let overdue = items.iter().filter(|item| item.days_until < 0).count();
    let kpis = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    kpis.set_hexpand(true);
    kpis.append(&kpi(&items.len().to_string(), "PRODUCTS"));
    kpis.append(&kpi(&overdue.to_string(), "OVERDUE"));
    card.append(&kpis);
    if items.is_empty() {
        let empty = gtk::Label::new(Some("Nothing is due from recent receipts."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        card.append(&empty);
    } else {
        let buckets: Vec<(String, f64)> = items
            .iter()
            .map(|item| (axis_name(&item.name), item.likely as f64))
            .collect();
        card.append(&bars(&buckets));
    }
    page.append(&card);
    let products: Vec<Prod> = items
        .into_iter()
        .map(|item| {
            let last = parse_date(&item.last_purchase).unwrap_or(today);
            let gap = item.median_interval_days.round() as i64;
            let steps = item.purchases.saturating_sub(1) as i64;
            Prod {
                name: item.name,
                times: item.purchases as usize,
                qty: item.purchases as f64,
                spent: item.typical_unit_price * item.purchases as f64,
                first: last - Duration::days(gap.max(0) * steps.max(0)),
                last,
                first_price: item.typical_unit_price,
                last_price: item.typical_unit_price,
                likely: item.likely,
            }
        })
        .collect();
    page.append(&product_table(shell, &products, true));
    page
}

fn purchases(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page.append(&board(shell, true));
    let view = load_view(shell);
    page.append(&receipt_list(shell, &view.invoices));
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
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let dash = gtk::Box::new(gtk::Orientation::Vertical, 6);
    dash.add_css_class("bf-card");
    dash.add_css_class("bf-dash");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
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
    if shell.borrow().bar_focus.is_some() {
        dash.add_css_class("is-focus");
    }
    if purchases {
        let kpis = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        kpis.set_hexpand(true);
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
    dash.append(&spend_bars(shell, &view.buckets));
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
        (Grain::Daily, "Daily"),
        (Grain::Weekly, "Weekly"),
        (Grain::Monthly, "Monthly"),
        (Grain::Yearly, "Yearly"),
    ] {
        let button = chip(label, shell.borrow().grain == grain);
        let shell_click = shell.clone();
        button.connect_clicked(move |_| {
            let mut shell = shell_click.borrow_mut();
            shell.grain = grain;
            shell.bar_focus = None;
            drop(shell);
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
            shell.bar_focus = None;
            drop(shell);
            schedule(&shell_click);
        });
        row.append(&button);
    }
    row
}

fn date_row(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let start = gtk::Entry::new();
    start.set_placeholder_text(Some("Start"));
    start.set_width_chars(10);
    start.set_text(&shell.borrow().custom_start);
    let end = gtk::Entry::new();
    end.set_placeholder_text(Some("End"));
    end.set_width_chars(10);
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
        shell.bar_focus = None;
        drop(shell);
        schedule(&shell_click);
    });
    row.append(&start);
    row.append(&end);
    row.append(&apply);
    row
}

struct BarLayout {
    width: i32,
    height: f64,
    centre: bool,
}

fn bar_layout(count: usize) -> BarLayout {
    let height = if count <= 6 { 72.0 } else { 52.0 };
    // A handful of bars keep a modest width and sit in the centre.
    // More than that share the card, with no per-bar minimum.
    let width = if count > 0 && count <= 6 { 56 } else { -1 };
    BarLayout {
        width,
        height,
        centre: width > 0,
    }
}

fn axis_name(name: &str) -> String {
    let mut chars = name.chars();
    let short: String = chars.by_ref().take(18).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

fn chart_rows(layout: &BarLayout) -> (gtk::Box, gtk::Box, gtk::Box) {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 2);
    wrap.set_hexpand(!layout.centre);
    let bars = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let labels = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    if layout.centre {
        bars.set_halign(gtk::Align::Center);
        bars.set_hexpand(false);
        labels.set_halign(gtk::Align::Center);
        labels.set_hexpand(false);
    } else {
        bars.set_homogeneous(true);
        bars.set_hexpand(true);
        bars.set_halign(gtk::Align::Fill);
        labels.set_homogeneous(true);
        labels.set_hexpand(true);
        labels.set_halign(gtk::Align::Fill);
    }
    wrap.append(&bars);
    wrap.append(&labels);
    (wrap, bars, labels)
}

fn axis_slots(count: usize, centre: bool) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    if centre || count <= 8 {
        return (0..count).collect();
    }
    let shown = 8;
    (0..shown)
        .map(|index| index * (count - 1) / (shown - 1))
        .collect()
}

fn push_axis(row: &gtk::Box, text: &str, layout: &BarLayout) {
    let caption = gtk::Label::new(Some(text));
    caption.add_css_class("bf-muted");
    caption.set_xalign(0.5);
    caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
    caption.set_max_width_chars(8);
    if layout.centre {
        caption.set_width_request(layout.width);
        caption.set_hexpand(false);
    } else {
        caption.set_hexpand(true);
    }
    row.append(&caption);
}

fn bars(buckets: &[(String, f64)]) -> gtk::Widget {
    let layout = bar_layout(buckets.len());
    let (wrap, row, labels) = chart_rows(&layout);
    let max = buckets.iter().fold(0.0_f64, |acc, (_, n)| acc.max(*n));
    let count = buckets.len();
    for (index, (label, amount)) in buckets.iter().enumerate() {
        let track = bar_fill(*amount, max, &layout);
        track.add_css_class("bf-bar");
        if index + 1 == count {
            track.add_css_class("on");
        }
        track.set_tooltip_text(Some(&format!("{label} · {amount:.2}")));
        row.append(&track);
    }
    for index in axis_slots(count, layout.centre) {
        push_axis(&labels, &buckets[index].0, &layout);
    }
    wrap.upcast()
}

fn spend_bars(shell: &Rc<RefCell<Shell>>, buckets: &[SpendBucket]) -> gtk::Widget {
    let layout = bar_layout(buckets.len());
    let (wrap, row, labels) = chart_rows(&layout);
    let max = buckets
        .iter()
        .fold(0.0_f64, |acc, bucket| acc.max(bucket.amount));
    let focus = shell.borrow().bar_focus.clone();
    for bucket in buckets {
        let track = bar_fill(bucket.amount, max, &layout);
        let button = gtk::Button::new();
        button.set_child(Some(&track));
        button.add_css_class("bf-bar");
        button.set_hexpand(!layout.centre);
        if layout.width > 0 {
            button.set_size_request(layout.width, layout.height as i32);
        } else {
            button.set_size_request(-1, layout.height as i32);
        }
        if focus.as_deref() == Some(bucket.key.as_str()) {
            button.add_css_class("focus");
        }
        button.set_tooltip_text(Some(&format!(
            "{} · AED {:.2}",
            bucket.label, bucket.amount
        )));
        let hit = gtk::GestureClick::new();
        hit.set_propagation_phase(gtk::PropagationPhase::Capture);
        let flag = shell.borrow().bar_hit.clone();
        hit.connect_pressed(move |_, _, _, _| {
            flag.set(true);
        });
        button.add_controller(hit);
        let shell_click = shell.clone();
        let key = bucket.key.clone();
        button.connect_clicked(move |_| {
            let mut state = shell_click.borrow_mut();
            if state.bar_focus.as_deref() == Some(key.as_str()) {
                state.bar_focus = None;
            } else {
                state.bar_focus = Some(key.clone());
            }
            drop(state);
            schedule(&shell_click);
        });
        row.append(&button);
    }
    for index in axis_slots(buckets.len(), layout.centre) {
        push_axis(&labels, &buckets[index].label, &layout);
    }
    wrap.upcast()
}

fn bar_fill(amount: f64, max: f64, layout: &BarLayout) -> gtk::Box {
    let track = gtk::Box::new(gtk::Orientation::Vertical, 0);
    track.set_hexpand(!layout.centre);
    if layout.width > 0 {
        track.set_size_request(layout.width, layout.height as i32);
    } else {
        track.set_size_request(-1, layout.height as i32);
    }
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    let fill = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fill.add_css_class("bf-bar-fill");
    fill.set_hexpand(true);
    let height = if max <= 0.0 {
        0
    } else {
        ((amount / max) * layout.height)
            .round()
            .clamp(0.0, layout.height) as i32
    };
    if height > 0 {
        fill.set_height_request(height.max(8).min(layout.height as i32));
    }
    track.append(&spacer);
    track.append(&fill);
    track
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
    let tools = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tools.set_halign(gtk::Align::End);
    tools.add_css_class("bf-thead");
    tools.append(&shortage_header(shell));
    card.append(&tools);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    head.add_css_class("bf-thead");
    head.append(&cell("", 22, false));
    let name = sort_header(shell, "Product", SortKey::Name, 0, false);
    name.set_hexpand(true);
    head.append(&name);
    if full {
        head.append(&sort_header(shell, "Last", SortKey::Last, 78, false));
    }
    head.append(&sort_header(shell, "Times", SortKey::Times, 42, true));
    if full {
        head.append(&sort_header(shell, "Units", SortKey::Qty, 48, true));
        head.append(&sort_header(
            shell,
            "Frequency",
            SortKey::Frequency,
            88,
            false,
        ));
        head.append(&sort_header(shell, "Likely", SortKey::Likely, 72, true));
    }
    head.append(&sort_header(shell, "Spent", SortKey::Spent, 78, true));
    card.append(&head);
    if products.is_empty() {
        let empty = gtk::Label::new(Some(if shell.borrow().shortage {
            "Nothing is short in this range."
        } else {
            "No invoices yet."
        }));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        empty.set_margin_top(4);
        card.append(&empty);
        return card;
    }
    for product in products {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.add_css_class("bf-row");
        row.append(&product_thumb(shell, &product.name, 22));
        let name = cell(&product.name, 0, false);
        name.set_hexpand(true);
        row.append(&name);
        if full {
            row.append(&cell(
                &product.last.format("%Y-%m-%d").to_string(),
                78,
                false,
            ));
        }
        row.append(&cell(&product.times.to_string(), 42, true));
        if full {
            row.append(&cell(&qty_text(product.qty), 48, true));
            row.append(&cell(&frequency_label(product), 88, false));
            row.append(&likely_votes(shell, &product.name, product.likely));
        }
        row.append(&cell(&format!("AED {:.2}", product.spent), 78, true));
        let click = gtk::GestureClick::new();
        let shell_click = shell.clone();
        let product_name = product.name.clone();
        let guard = shell.borrow().vote_guard.clone();
        click.connect_released(move |_, _, _, _| {
            if guard.replace(false) {
                return;
            }
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
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page.append(&heading_mark("YOUR ", "STORES"));
    let lead = gtk::Label::new(Some(
        "Every store can be searched and compared. Open a store to link it, or to change the cart it holds.",
    ));
    lead.add_css_class("bf-muted");
    wrap_label(&lead);
    page.append(&lead);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(6);
    grid.set_row_spacing(6);
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
    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
    card.add_css_class("bf-card");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let logo = store_logo(id);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.set_hexpand(true);
    let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let name = gtk::Label::new(Some(&store_label(id).to_uppercase()));
    name.add_css_class("bf-brand");
    name.set_xalign(0.0);
    let pill = gtk::Label::new(Some("ON"));
    pill.add_css_class("bf-pill");
    pill.add_css_class("on");
    name_row.append(&name);
    name_row.append(&pill);
    let status = store_status(id, linked, username);
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

fn store_status(id: StoreId, linked: bool, username: Option<String>) -> gtk::Label {
    let (text, ok) = if linked {
        let text = match username {
            Some(user) if !user.is_empty() => format!("Linked · {user}"),
            _ => "Linked".into(),
        };
        (text, true)
    } else if let Some(domain) = id.mail_domain() {
        (format!("Mail · {domain}"), false)
    } else if id.receipts() && !id.login() {
        ("Invoices by mail".into(), false)
    } else if id.login() {
        ("Not linked".into(), false)
    } else {
        ("Search only".into(), false)
    };
    let label = gtk::Label::new(Some(&text));
    label.add_css_class(if ok { "bf-ok" } else { "bf-muted" });
    label.set_xalign(0.0);
    label
}

fn caps_row(id: StoreId, linked: bool) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (label, on) in [
        ("SEARCH", id.search()),
        ("COMPARE", id.search()),
        ("CART", id.search()),
        ("CHECKOUT", id.remote_checkout()),
        ("RECEIPTS", id.receipts()),
        ("LOGIN", id.login() && linked),
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
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
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

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let logo = store_logo(id);
    let title = heading(&store_label(id).to_uppercase());
    head.append(&logo);
    head.append(&title);
    page.append(&head);
    page.append(&caps_row(id, linked));

    if !id.login() {
        let note = gtk::Label::new(Some(
            "Invoices arrive by mail. This store has no catalog and no sign-in.",
        ));
        note.add_css_class("bf-muted");
        note.set_xalign(0.0);
        wrap_label(&note);
        page.append(&note);
        return page;
    }

    let form = gtk::Box::new(gtk::Orientation::Vertical, 3);
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
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
    let card = gtk::Box::new(gtk::Orientation::Vertical, 3);
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
    let add = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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

    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let back = gtk::Button::with_label("← PURCHASES");
    back.add_css_class("bf-ghost");
    back.set_halign(gtk::Align::Start);
    let shell_back = shell.clone();
    back.connect_clicked(move |_| {
        shell_back.borrow_mut().page = Page::Purchases;
        schedule(&shell_back);
    });
    page.append(&back);

    let history = shell.borrow().core.product_history(&name).ok();
    let hero = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hero.append(&product_thumb(shell, &name, 64));
    let title = history
        .as_ref()
        .map(|item| item.name.to_uppercase())
        .unwrap_or_else(|| name.to_uppercase());
    hero.append(&heading(&title));
    page.append(&hero);

    let stats = gtk::FlowBox::new();
    stats.add_css_class("bf-stats");
    stats.set_selection_mode(gtk::SelectionMode::None);
    stats.set_column_spacing(8);
    stats.set_row_spacing(4);
    stats.set_min_children_per_line(2);
    stats.set_max_children_per_line(4);
    stats.set_homogeneous(true);
    stats.set_hexpand(true);
    if let Some(item) = &history {
        let last = if item.last.is_empty() {
            "—".to_string()
        } else {
            item.last.clone()
        };
        let next = next_label(&item.next);
        for (caption, value) in [
            ("TIMES", item.times.to_string()),
            ("SPENT", format!("AED {:.2}", item.spent)),
            ("AVG", format!("AED {:.2}", item.avg)),
            ("MIN", format!("AED {:.2}", item.min)),
            ("MAX", format!("AED {:.2}", item.max)),
            ("LAST", last),
            ("FREQUENCY", item.frequency.clone()),
            ("NEXT", next),
        ] {
            stats.insert(&kpi(&value, caption), -1);
        }
        stats.insert(&likely_kpi(shell, &name, item.likely), -1);
    }
    page.append(&stats);
    if let Some(item) = &history {
        page.append(&product_chart(shell, item));
        page.append(&store_prices(item));
        page.append(&purchase_list(shell, item));
    }

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
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("bf-row");
    let source = match offer.source {
        omabring_core::OfferSource::Live => "live",
        omabring_core::OfferSource::History => "receipt",
    };
    let store_name = cell(&store_label(offer.store).to_uppercase(), 96, false);
    let product = cell(&offer.name, 0, false);
    product.set_hexpand(true);
    let price_label = cell(
        &format!(
            "{:.2} {} / {} · {source}",
            offer.price, offer.currency, offer.unit
        ),
        148,
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
    row.append(&store_logo_small(offer.store));
    row.append(&store_name);
    row.append(&product);
    row.append(&price_label);
    row.append(&add);
    row
}

fn product_chart(shell: &Rc<RefCell<Shell>>, item: &omabring_core::ProductHistory) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("bf-card");
    let kicker = gtk::Label::new(Some("CHART"));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    card.append(&kicker);
    card.append(&chart_selector(shell));
    if item.points.is_empty() {
        let empty = gtk::Label::new(Some("No priced buys yet."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        card.append(&empty);
        return card;
    }
    let kind = shell.borrow().chart;
    let note = gtk::Label::new(Some(&chart_caption(kind, item)));
    note.add_css_class("bf-muted");
    note.set_xalign(0.0);
    wrap_label(&note);
    card.append(&note);
    if kind == ChartKind::PriceByStore {
        card.append(&price_by_store_chart(item));
    } else {
        card.append(&bars(&chart_buckets(kind, item)));
    }
    card
}

fn chart_selector(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("bf-charts");
    row.set_halign(gtk::Align::Start);
    for (kind, label) in [
        (ChartKind::Price, "Price"),
        (ChartKind::PriceByStore, "By store"),
        (ChartKind::QtyMonth, "Qty"),
        (ChartKind::SpendMonth, "Spend"),
        (ChartKind::SpendYear, "Year"),
        (ChartKind::StoreCompare, "Compare"),
        (ChartKind::Cumulative, "Total"),
    ] {
        let button = chip(label, shell.borrow().chart == kind);
        button.add_css_class("bf-chart");
        button.set_size_request(-1, 18);
        let shell_click = shell.clone();
        button.connect_clicked(move |_| {
            shell_click.borrow_mut().chart = kind;
            schedule(&shell_click);
        });
        row.append(&button);
    }
    row
}

fn chart_caption(kind: ChartKind, item: &omabring_core::ProductHistory) -> String {
    match kind {
        ChartKind::Price => format!(
            "Price paid · {} → {} · AED {:.2} to AED {:.2}",
            item.first, item.last, item.min, item.max
        ),
        ChartKind::PriceByStore => "Price paid over time, one row per store".into(),
        ChartKind::QtyMonth => "Units bought each month".into(),
        ChartKind::SpendMonth => "Spend each month".into(),
        ChartKind::SpendYear => "Spend each year".into(),
        ChartKind::StoreCompare => "Last price paid at each store".into(),
        ChartKind::Cumulative => "Cumulative spend".into(),
    }
}

fn price_by_store_chart(item: &omabring_core::ProductHistory) -> gtk::Box {
    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let mut stores: Vec<StoreId> = Vec::new();
    for point in &item.points {
        if !stores.contains(&point.store) {
            stores.push(point.store);
        }
    }
    for store in stores {
        let title = gtk::Label::new(Some(&store_label(store).to_uppercase()));
        title.add_css_class("bf-kicker");
        title.set_xalign(0.0);
        wrap.append(&title);
        let points: Vec<_> = item
            .points
            .iter()
            .filter(|point| point.store == store)
            .cloned()
            .collect();
        wrap.append(&bars(&average_by(&points, "%Y-%m", |point| {
            point.unit_price
        })));
    }
    wrap
}

fn chart_buckets(kind: ChartKind, item: &omabring_core::ProductHistory) -> Vec<(String, f64)> {
    match kind {
        ChartKind::Price | ChartKind::PriceByStore => {
            average_by(&item.points, "%Y-%m", |point| point.unit_price)
        }
        ChartKind::QtyMonth => sum_by(&item.points, "%Y-%m", |point| point.qty),
        ChartKind::SpendMonth => {
            sum_by(&item.points, "%Y-%m", |point| point.unit_price * point.qty)
        }
        ChartKind::SpendYear => sum_by(&item.points, "%Y", |point| point.unit_price * point.qty),
        ChartKind::StoreCompare => item
            .by_store
            .iter()
            .map(|row| (store_label(row.store).to_string(), row.last))
            .collect(),
        ChartKind::Cumulative => cumulative(&sum_by(&item.points, "%Y-%m", |point| {
            point.unit_price * point.qty
        })),
    }
}

fn average_by(
    points: &[omabring_core::PurchasePoint],
    fmt: &str,
    value: impl Fn(&omabring_core::PurchasePoint) -> f64,
) -> Vec<(String, f64)> {
    let mut buckets: Vec<(String, f64, u32)> = Vec::new();
    for point in points {
        let key = point.date.format(fmt).to_string();
        let amount = value(point);
        if let Some(last) = buckets.last_mut() {
            if last.0 == key {
                last.1 += amount;
                last.2 += 1;
                continue;
            }
        }
        buckets.push((key, amount, 1));
    }
    buckets
        .into_iter()
        .map(|(key, sum, count)| (key, sum / count.max(1) as f64))
        .collect()
}

fn sum_by(
    points: &[omabring_core::PurchasePoint],
    fmt: &str,
    value: impl Fn(&omabring_core::PurchasePoint) -> f64,
) -> Vec<(String, f64)> {
    let mut buckets: Vec<(String, f64)> = Vec::new();
    for point in points {
        let key = point.date.format(fmt).to_string();
        let amount = value(point);
        if let Some(last) = buckets.last_mut() {
            if last.0 == key {
                last.1 += amount;
                continue;
            }
        }
        buckets.push((key, amount));
    }
    buckets
}

fn cumulative(buckets: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut running = 0.0;
    buckets
        .iter()
        .map(|(key, amount)| {
            running += amount;
            (key.clone(), running)
        })
        .collect()
}

fn store_prices(item: &omabring_core::ProductHistory) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("bf-card");
    let kicker = gtk::Label::new(Some("PRICE BY STORE"));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    card.append(&kicker);
    for row in &item.by_store {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        line.add_css_class("bf-row");
        line.append(&store_logo_small(row.store));
        let name = cell(&store_label(row.store).to_uppercase(), 0, false);
        name.set_hexpand(true);
        line.append(&name);
        line.append(&cell(
            &format!(
                "{} buys · last {:.2} · {:.2}–{:.2} AED",
                row.buys, row.last, row.min, row.max
            ),
            180,
            true,
        ));
        card.append(&line);
    }
    card
}

fn purchase_list(shell: &Rc<RefCell<Shell>>, item: &omabring_core::ProductHistory) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("bf-card");
    let kicker = gtk::Label::new(Some("PURCHASES"));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    card.append(&kicker);
    for point in item.points.iter().rev() {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        line.add_css_class("bf-row");
        let date = cell(&point.date.format("%Y-%m-%d").to_string(), 96, false);
        let when = cell(
            &format!("{} · #{}", store_label(point.store), point.invoice_no),
            0,
            false,
        );
        when.set_hexpand(true);
        line.append(&date);
        line.append(&when);
        line.append(&cell(
            &format!("AED {:.2} · {} u", point.unit_price, qty_text(point.qty)),
            160,
            true,
        ));
        let click = gtk::GestureClick::new();
        let shell_click = shell.clone();
        let id = point.invoice_id;
        click.connect_released(move |_, _, _, _| {
            shell_click.borrow_mut().page = Page::Receipt(id);
            schedule(&shell_click);
        });
        line.add_controller(click);
        card.append(&line);
    }
    card
}

fn product_thumb(shell: &Rc<RefCell<Shell>>, name: &str, size: i32) -> gtk::Widget {
    // Scale once into a square paintable of exactly `size`. Picture then
    // reports that size as both minimum and natural, so a full photo cannot
    // widen the row or the page.
    if let Some(path) = shell.borrow().core.product_image(name) {
        if let Some(texture) = fitted_texture(&path, size, true) {
            let picture = gtk::Picture::for_paintable(&texture);
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_size_request(size, size);
            picture.set_hexpand(false);
            picture.set_vexpand(false);
            picture.set_halign(gtk::Align::Start);
            picture.set_valign(gtk::Align::Center);
            picture.set_overflow(gtk::Overflow::Hidden);
            picture.add_css_class("bf-thumb");
            return picture.upcast();
        }
    }
    let thumb = gtk::Label::new(Some(&initial(name)));
    thumb.add_css_class("bf-thumb");
    thumb.set_size_request(size, size);
    thumb.set_hexpand(false);
    thumb.set_vexpand(false);
    thumb.set_halign(gtk::Align::Start);
    thumb.set_valign(gtk::Align::Center);
    thumb.set_xalign(0.5);
    thumb.set_yalign(0.5);
    thumb.upcast()
}

thread_local! {
    static THUMB_CACHE: RefCell<HashMap<(String, i32, bool), gtk::gdk::Texture>> =
        RefCell::new(HashMap::new());
}

fn fitted_texture(path: &std::path::Path, size: i32, cover: bool) -> Option<gtk::gdk::Texture> {
    if size <= 0 {
        return None;
    }
    let key = (path.to_string_lossy().into_owned(), size, cover);
    if let Some(hit) = THUMB_CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(hit);
    }
    let file = gtk::gio::File::for_path(path);
    let source = gtk::gdk::Texture::from_file(&file).ok()?;
    let fitted = fit_texture(&source, size, cover)?;
    THUMB_CACHE.with(|cache| cache.borrow_mut().insert(key, fitted.clone()));
    Some(fitted)
}

/// Scale a logo to `height` and let the width follow the picture.
/// A wide wordmark stays wide instead of being letterboxed into a square.
fn fit_logo(texture: &gtk::gdk::Texture, height: i32) -> Option<gtk::gdk::Texture> {
    let width = texture.width();
    let src_h = texture.height();
    if width <= 0 || src_h <= 0 || height <= 0 {
        return None;
    }
    let scale = height as f64 / src_h as f64;
    let out_w = ((width as f64) * scale).round().clamp(1.0, 480.0) as i32;
    let src_stride = width as usize * 4;
    let mut src = vec![0u8; src_stride * src_h as usize];
    texture.download(&mut src, src_stride);
    let mut dst = vec![0u8; out_w as usize * height as usize * 4];
    for y in 0..height {
        let sy = ((y as f64 + 0.5) / scale).floor() as i32;
        let sy = sy.clamp(0, src_h - 1) as usize;
        for x in 0..out_w {
            let sx = ((x as f64 + 0.5) / scale).floor() as i32;
            let sx = sx.clamp(0, width - 1) as usize;
            let from = sy * src_stride + sx * 4;
            let to = (y as usize * out_w as usize + x as usize) * 4;
            dst[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    let bytes = gtk::glib::Bytes::from_owned(dst);
    Some(
        gtk::gdk::MemoryTexture::new(
            out_w,
            height,
            gtk::gdk::MemoryFormat::B8g8r8a8,
            &bytes,
            out_w as usize * 4,
        )
        .upcast(),
    )
}

fn fit_texture(texture: &gtk::gdk::Texture, size: i32, cover: bool) -> Option<gtk::gdk::Texture> {
    let width = texture.width();
    let height = texture.height();
    if width <= 0 || height <= 0 || size <= 0 {
        return None;
    }
    let src_stride = width as usize * 4;
    let mut src = vec![0u8; src_stride * height as usize];
    texture.download(&mut src, src_stride);
    let size_f = size as f64;
    let scale = if cover {
        (size_f / width as f64).max(size_f / height as f64)
    } else {
        (size_f / width as f64).min(size_f / height as f64)
    };
    if scale <= 0.0 {
        return None;
    }
    let mut dst = vec![0u8; size as usize * size as usize * 4];
    if cover {
        let crop_w = (size_f / scale).clamp(1.0, width as f64);
        let crop_h = (size_f / scale).clamp(1.0, height as f64);
        let x0 = ((width as f64 - crop_w) / 2.0).max(0.0);
        let y0 = ((height as f64 - crop_h) / 2.0).max(0.0);
        for y in 0..size {
            let sy = (y0 + (y as f64 + 0.5) * crop_h / size_f).floor() as i32;
            let sy = sy.clamp(0, height - 1) as usize;
            for x in 0..size {
                let sx = (x0 + (x as f64 + 0.5) * crop_w / size_f).floor() as i32;
                let sx = sx.clamp(0, width - 1) as usize;
                let from = sy * src_stride + sx * 4;
                let to = (y as usize * size as usize + x as usize) * 4;
                dst[to..to + 4].copy_from_slice(&src[from..from + 4]);
            }
        }
    } else {
        let draw_w = ((width as f64) * scale).round().clamp(1.0, size_f) as i32;
        let draw_h = ((height as f64) * scale).round().clamp(1.0, size_f) as i32;
        let x_off = (size - draw_w) / 2;
        let y_off = (size - draw_h) / 2;
        for y in 0..draw_h {
            let sy = ((y as f64 + 0.5) / scale).floor() as i32;
            let sy = sy.clamp(0, height - 1) as usize;
            let dy = (y + y_off).clamp(0, size - 1) as usize;
            for x in 0..draw_w {
                let sx = ((x as f64 + 0.5) / scale).floor() as i32;
                let sx = sx.clamp(0, width - 1) as usize;
                let dx = (x + x_off).clamp(0, size - 1) as usize;
                let from = sy * src_stride + sx * 4;
                let to = (dy * size as usize + dx) * 4;
                dst[to..to + 4].copy_from_slice(&src[from..from + 4]);
            }
        }
    }
    let bytes = gtk::glib::Bytes::from_owned(dst);
    Some(
        gtk::gdk::MemoryTexture::new(
            size,
            size,
            gtk::gdk::MemoryFormat::B8g8r8a8,
            &bytes,
            size as usize * 4,
        )
        .upcast(),
    )
}

fn paint_filters(shell: &Rc<RefCell<Shell>>) {
    let bar = shell.borrow().filters.clone();
    while let Some(child) = bar.first_child() {
        bar.remove(&child);
    }
    let show = matches!(
        shell.borrow().page,
        Page::Dashboard | Page::Purchases | Page::Product(_)
    );
    bar.set_visible(show);
    if !show {
        return;
    }
    bar.append(&range_row(shell));
    bar.append(&date_row(shell));
}

fn agent_button() -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("bf-agent");
    button.set_tooltip_text(Some("Open the Omarchy agent"));
    button.set_child(Some(&agent_mark()));
    button
}

fn agent_mark() -> gtk::Image {
    let name = if gtk::gdk::Display::default()
        .map(|display| gtk::IconTheme::for_display(&display).has_icon("omabring-agent-symbolic"))
        .unwrap_or(false)
    {
        "omabring-agent-symbolic"
    } else {
        "utilities-terminal-symbolic"
    };
    let image = gtk::Image::from_icon_name(name);
    image.set_pixel_size(14);
    image.add_css_class("bf-agent-icon");
    image
}

fn launch_agent(shell: &Rc<RefCell<Shell>>) {
    let core = shell.borrow().core.clone();
    let toast = shell.borrow().toast.clone();
    let (id, custom) = agent_choice(&core);
    let mcp = format!("http://{}/mcp", omabring_core::default_bind_addr());
    let config = core.data_dir().join("mcp.json");
    let _ = std::fs::write(
        &config,
        format!(r#"{{"mcpServers":{{"omabring":{{"type":"http","url":"{mcp}"}}}}}}"#),
    );
    let argv = agent_argv(&id, &custom, &config);
    match spawn_terminal(&argv, &mcp) {
        Some(name) => toast.add_toast(adw::Toast::new(&format!("Opened {name}"))),
        None => toast.add_toast(adw::Toast::new(
            "The agent launcher is not on this machine. It stays in a terminal, never a browser.",
        )),
    }
}

fn agent_choice(core: &App) -> (String, String) {
    let id = core
        .setting("agent")
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(default_omarchy_agent);
    let custom = core
        .setting("agent_command")
        .ok()
        .flatten()
        .unwrap_or_default();
    (id, custom)
}

fn default_omarchy_agent() -> String {
    if let Ok(output) = std::process::Command::new("omarchy-default-agent").output() {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout);
            let name = name.trim();
            if !name.is_empty() {
                return canonical_agent(name);
            }
        }
    }
    "hermes".into()
}

fn canonical_agent(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" | "claude code" => "claude".into(),
        "codex" => "codex".into(),
        "cursor" | "cursor-agent" | "cursor cli" | "cursor-cli" => "cursor".into(),
        "grok" => "grok".into(),
        "hermes" => "hermes".into(),
        "opencode" | "open-code" | "open code" => "opencode".into(),
        "custom" => "custom".into(),
        other => other.to_string(),
    }
}

fn agent_argv(id: &str, custom: &str, config: &std::path::Path) -> Vec<String> {
    let config = config.display().to_string();
    match id {
        "claude" => vec![
            "claude".into(),
            "--permission-mode".into(),
            "auto".into(),
            "--mcp-config".into(),
            config,
        ],
        "codex" => vec!["codex".into(), "--approve-for-me".into()],
        "cursor" => vec!["cursor-agent".into()],
        "grok" => vec!["grok".into()],
        "opencode" => vec!["opencode".into(), "--auto".into()],
        "custom" => split_command(custom),
        other => vec![other.to_string()],
    }
}

fn split_command(custom: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in custom.chars() {
        match (quote, ch) {
            (None, '"' | '\'') => quote = Some(ch),
            (Some(mark), ch) if ch == mark => quote = None,
            (None, ch) if ch.is_whitespace() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            (_, ch) => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    if args.is_empty() {
        vec!["hermes".into()]
    } else {
        args
    }
}

fn spawn_terminal(argv: &[String], mcp: &str) -> Option<String> {
    let bin = argv.first()?.clone();
    if !on_path(&bin) {
        return None;
    }
    let mut command = if on_path("omarchy-launch-tui") {
        let mut command = std::process::Command::new("omarchy-launch-tui");
        command.arg("--app-id=org.omarchy.agent");
        command.args(argv);
        command
    } else {
        let term = ["foot", "alacritty", "kitty", "ghostty", "wezterm"]
            .into_iter()
            .find(|name| on_path(name))?;
        let mut command = std::process::Command::new(term);
        command.arg("-e");
        command.args(argv);
        command
    };
    command
        .env("OMABRING_MCP_URL", mcp)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.spawn().ok()?;
    Some(bin)
}

fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}

fn settings_page(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
    page.append(&heading_mark("", "SETTINGS"));
    let lead = gtk::Label::new(Some(
        "The local service on this machine. Allow or deny a client in this window.",
    ));
    lead.add_css_class("bf-muted");
    wrap_label(&lead);
    page.append(&lead);
    let card = gtk::Box::new(gtk::Orientation::Vertical, 3);
    card.add_css_class("bf-card");
    card.add_css_class("bf-dashed");
    let title = gtk::Label::new(Some("LOCAL SERVICE"));
    title.add_css_class("bf-h1-sm");
    title.set_xalign(0.0);
    let address = omabring_core::default_bind_addr();
    let url = gtk::Label::new(Some(&format!("http://{address}")));
    url.add_css_class("bf-delta");
    url.add_css_class("bf-link");
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
    page.append(&backup_card(shell));
    page.append(&agent_card(shell));
    page
}

fn backup_card(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 3);
    card.add_css_class("bf-card");
    let title = gtk::Label::new(Some("BACKUP"));
    title.add_css_class("bf-h1-sm");
    title.set_xalign(0.0);
    let hint = gtk::Label::new(Some(
        "A copy of the database is written once a day. Backup now writes one immediately.",
    ));
    hint.add_css_class("bf-muted");
    wrap_label(&hint);
    let entry = gtk::Entry::new();
    entry.set_text(&shell.borrow().core.backup_dir().to_string_lossy());
    let last = shell
        .borrow()
        .core
        .last_backup_at()
        .unwrap_or_else(|| "never".into());
    let stamp = gtk::Label::new(Some(&format!("Last backup: {last}")));
    stamp.add_css_class("bf-muted");
    stamp.set_xalign(0.0);
    let button = gtk::Button::with_label("BACKUP NOW");
    let shell_click = shell.clone();
    let entry_click = entry.clone();
    button.connect_clicked(move |_| {
        let path = entry_click.text().trim().to_string();
        let core = shell_click.borrow().core.clone();
        let toast = shell_click.borrow().toast.clone();
        if let Err(err) = core.set_backup_dir(std::path::Path::new(&path)) {
            toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
        match core.backup_now() {
            Ok(when) => {
                toast.add_toast(adw::Toast::new(&format!("Backup written {when}")));
                schedule(&shell_click);
            }
            Err(err) => toast.add_toast(adw::Toast::new(&err.to_string())),
        }
    });
    card.append(&title);
    card.append(&hint);
    card.append(&entry);
    card.append(&stamp);
    card.append(&button);
    card
}

fn agent_card(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 3);
    card.add_css_class("bf-card");
    let title = gtk::Label::new(Some("AGENT"));
    title.add_css_class("bf-h1-sm");
    title.set_xalign(0.0);
    let hint = gtk::Label::new(Some(
        "The top-right button opens this agent in a terminal, with the OmaBring MCP address. It does not open a browser.",
    ));
    hint.add_css_class("bf-muted");
    wrap_label(&hint);
    let (current, custom) = agent_choice(&shell.borrow().core);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for (id, label) in [
        ("claude", "Claude Code"),
        ("codex", "Codex"),
        ("cursor", "Cursor CLI"),
        ("grok", "Grok"),
        ("hermes", "Hermes"),
        ("opencode", "OpenCode"),
        ("custom", "Custom"),
    ] {
        let button = chip(label, current == id);
        let shell_click = shell.clone();
        let id = id.to_string();
        button.connect_clicked(move |_| {
            if let Err(err) = shell_click.borrow().core.set_setting("agent", &id) {
                shell_click
                    .borrow()
                    .toast
                    .add_toast(adw::Toast::new(&err.to_string()));
                return;
            }
            schedule(&shell_click);
        });
        row.append(&button);
    }
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Custom command"));
    entry.set_text(&custom);
    let save = gtk::Button::with_label("SAVE COMMAND");
    save.add_css_class("bf-ghost");
    let shell_save = shell.clone();
    let entry_save = entry.clone();
    save.connect_clicked(move |_| {
        let command = entry_save.text().to_string();
        let core = shell_save.borrow().core.clone();
        let toast = shell_save.borrow().toast.clone();
        if let Err(err) = core.set_setting("agent_command", &command) {
            toast.add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
        let _ = core.set_setting("agent", "custom");
        toast.add_toast(adw::Toast::new("Custom agent saved"));
        schedule(&shell_save);
    });
    card.append(&title);
    card.append(&hint);
    card.append(&row);
    card.append(&entry);
    card.append(&save);
    card
}

fn import_card(shell: &Rc<RefCell<Shell>>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 3);
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
        .min_content_height(64)
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
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
    let page = gtk::Box::new(gtk::Orientation::Vertical, 4);
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
    let date = gtk::Label::new(Some(&invoice.purchased_on));
    date.add_css_class("bf-brand");
    date.set_xalign(0.0);
    let meta = gtk::Label::new(Some(&format!("#{}", invoice.invoice_no)));
    meta.add_css_class("bf-muted");
    meta.set_xalign(0.0);
    slip.append(&title);
    slip.append(&date);
    slip.append(&meta);

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    head.add_css_class("bf-thead");
    let item = cell("ITEM", 0, false);
    item.set_hexpand(true);
    head.append(&item);
    let qty = cell("QTY", 72, true);
    qty.set_hexpand(false);
    let amount = cell("AED", 96, true);
    head.append(&qty);
    head.append(&amount);
    slip.append(&head);
    for line in &invoice.lines {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.add_css_class("bf-row");
        let name = cell(&line.name, 0, false);
        name.set_hexpand(true);
        row.append(&name);
        row.append(&cell(&format!("{:.2}", line.qty), 72, true));
        row.append(&cell(&format!("{:.2}", line.line_total), 96, true));
        slip.append(&row);
    }
    let total = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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

struct SpendBucket {
    key: String,
    label: String,
    amount: f64,
    start: NaiveDate,
    end: NaiveDate,
}

struct ViewData {
    start: NaiveDate,
    end: NaiveDate,
    total: f64,
    average: f64,
    receipts: usize,
    invoices: Vec<InvoiceSummary>,
    buckets: Vec<SpendBucket>,
    products: Vec<Prod>,
}

fn load_view(shell: &Rc<RefCell<Shell>>) -> ViewData {
    let shell_ref = shell.borrow();
    let today = chrono::Utc::now().date_naive();
    let invoices = shell_ref.core.list_invoices().unwrap_or_default();
    let (start, end) = window_for(&shell_ref, &invoices, today);
    let grain = shell_ref.grain;
    let focus = shell_ref.bar_focus.clone();
    let sort = shell_ref.sort;
    let sort_desc = shell_ref.sort_desc;
    let shortage = shell_ref.shortage;
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
        .collect();
    let mut by_day: HashMap<NaiveDate, f64> = HashMap::new();
    for invoice in &filtered {
        if let Some(date) = parse_date(&invoice.purchased_on) {
            *by_day.entry(date).or_default() += invoice.total;
        }
    }
    let buckets = match grain {
        Grain::Daily => daily_buckets(start, end, &by_day),
        Grain::Weekly => week_buckets(start, end, &by_day),
        Grain::Monthly => month_buckets(start, end, &by_day),
        Grain::Yearly => year_buckets(start, end, &by_day),
    };
    let focus_span = focus.as_ref().and_then(|key| {
        buckets
            .iter()
            .find(|bucket| &bucket.key == key)
            .map(|bucket| (bucket.start, bucket.end))
    });
    if focus.is_some() && focus_span.is_none() {
        shell.borrow_mut().bar_focus = None;
    }
    let shown: Vec<&InvoiceSummary> = if let Some((from, to)) = focus_span {
        filtered
            .into_iter()
            .filter(|invoice| {
                parse_date(&invoice.purchased_on).is_some_and(|date| date >= from && date <= to)
            })
            .collect()
    } else {
        filtered
    };
    let total: f64 = shown.iter().map(|invoice| invoice.total).sum();
    let periods = if focus_span.is_some() {
        1.0
    } else {
        buckets.len().max(1) as f64
    };
    let (start, end) = focus_span.unwrap_or((start, end));
    let mut products = products_from(&shown, &likely, sort, sort_desc);
    if shortage {
        products.retain(|product| is_shortage(product, today));
    }
    let mut invoices: Vec<InvoiceSummary> = shown.into_iter().cloned().collect();
    invoices.sort_by(|a, b| {
        b.purchased_on
            .cmp(&a.purchased_on)
            .then(b.invoice_no.cmp(&a.invoice_no))
    });
    ViewData {
        start,
        end,
        total,
        average: total / periods,
        receipts: invoices.len(),
        invoices,
        buckets,
        products,
    }
}

fn receipt_list(shell: &Rc<RefCell<Shell>>, invoices: &[InvoiceSummary]) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("bf-card");
    let kicker = gtk::Label::new(Some(&format!("RECEIPTS · {}", invoices.len())));
    kicker.add_css_class("bf-kicker");
    kicker.set_xalign(0.0);
    card.append(&kicker);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    head.add_css_class("bf-thead");
    head.append(&cell("Date", 78, false));
    let store = cell("Store", 0, false);
    store.set_hexpand(true);
    head.append(&store);
    head.append(&cell("Receipt", 110, false));
    head.append(&cell("Total", 78, true));
    card.append(&head);
    if invoices.is_empty() {
        let empty = gtk::Label::new(Some("No receipts in this range."));
        empty.add_css_class("bf-muted");
        empty.set_xalign(0.0);
        card.append(&empty);
        return card;
    }
    for invoice in invoices {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.add_css_class("bf-row");
        row.append(&cell(&invoice.purchased_on, 78, false));
        let store = cell(&store_label(invoice.store), 0, false);
        store.set_hexpand(true);
        row.append(&store);
        row.append(&cell(&format!("#{}", invoice.invoice_no), 110, false));
        row.append(&cell(
            &format!("{} {:.2}", invoice.currency, invoice.total),
            78,
            true,
        ));
        let click = gtk::GestureClick::new();
        let shell_click = shell.clone();
        let id = invoice.id;
        click.connect_released(move |_, _, _, _| {
            shell_click.borrow_mut().page = Page::Receipt(id);
            schedule(&shell_click);
        });
        row.add_controller(click);
        card.append(&row);
    }
    card
}

fn next_label(next: &str) -> String {
    let Some(date) = parse_date(next) else {
        return if next.trim().is_empty() {
            "—".into()
        } else {
            next.to_string()
        };
    };
    if date < chrono::Utc::now().date_naive() {
        "overdue".into()
    } else {
        next.to_string()
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

fn sum_days(from: NaiveDate, to: NaiveDate, by_day: &HashMap<NaiveDate, f64>) -> f64 {
    let mut sum = 0.0;
    let mut day = from;
    while day <= to {
        sum += by_day.get(&day).copied().unwrap_or(0.0);
        day += Duration::days(1);
    }
    sum
}

fn monday_of(day: NaiveDate) -> NaiveDate {
    day - Duration::days(day.weekday().num_days_from_monday() as i64)
}

fn daily_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<SpendBucket> {
    let mut out = Vec::new();
    let mut cursor = start;
    while cursor <= end {
        out.push(SpendBucket {
            key: format!("day:{}", cursor.format("%Y-%m-%d")),
            label: cursor.format("%m-%d").to_string(),
            amount: by_day.get(&cursor).copied().unwrap_or(0.0),
            start: cursor,
            end: cursor,
        });
        cursor += Duration::days(1);
        if out.len() > 400 {
            break;
        }
    }
    out
}

fn week_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<SpendBucket> {
    let mut out = Vec::new();
    let mut cursor = monday_of(start);
    while cursor <= end {
        let week_end = cursor + Duration::days(6);
        let from = cursor.max(start);
        let to = week_end.min(end);
        if from <= to {
            out.push(SpendBucket {
                key: format!("week:{}", cursor.format("%Y-%m-%d")),
                label: cursor.format("%m-%d").to_string(),
                amount: sum_days(from, to, by_day),
                start: from,
                end: to,
            });
        }
        cursor += Duration::days(7);
    }
    out
}

fn month_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<SpendBucket> {
    let mut out = Vec::new();
    let mut cursor = NaiveDate::from_ymd_opt(start.year(), start.month(), 1).unwrap_or(start);
    while cursor <= end {
        let next = if cursor.month() == 12 {
            NaiveDate::from_ymd_opt(cursor.year() + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(cursor.year(), cursor.month() + 1, 1)
        };
        let Some(next) = next else {
            break;
        };
        let month_end = next - Duration::days(1);
        let from = cursor.max(start);
        let to = month_end.min(end);
        if from <= to {
            out.push(SpendBucket {
                key: format!("month:{}", cursor.format("%Y-%m")),
                label: cursor.format("%Y-%m").to_string(),
                amount: sum_days(from, to, by_day),
                start: from,
                end: to,
            });
        }
        cursor = next;
    }
    out
}

fn year_buckets(
    start: NaiveDate,
    end: NaiveDate,
    by_day: &HashMap<NaiveDate, f64>,
) -> Vec<SpendBucket> {
    let mut out = Vec::new();
    let mut cursor = NaiveDate::from_ymd_opt(start.year(), 1, 1).unwrap_or(start);
    while cursor <= end {
        let Some(next) = NaiveDate::from_ymd_opt(cursor.year() + 1, 1, 1) else {
            break;
        };
        let year_end = next - Duration::days(1);
        let from = cursor.max(start);
        let to = year_end.min(end);
        if from <= to {
            out.push(SpendBucket {
                key: format!("year:{}", cursor.format("%Y")),
                label: cursor.format("%Y").to_string(),
                amount: sum_days(from, to, by_day),
                start: from,
                end: to,
            });
        }
        cursor = next;
    }
    out
}

fn products_from(
    invoices: &[&InvoiceSummary],
    likely: &HashMap<String, u8>,
    sort: SortKey,
    desc: bool,
) -> Vec<Prod> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Prod> = HashMap::new();
    for invoice in invoices {
        let Some(date) = parse_date(&invoice.purchased_on) else {
            continue;
        };
        for line in &invoice.lines {
            if junk_product_name(&line.name) {
                continue;
            }
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
    products.sort_by(|a, b| {
        let ordering = match sort {
            SortKey::Last => a.last.cmp(&b.last),
            SortKey::Spent => a
                .spent
                .partial_cmp(&b.spent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortKey::Qty => a
                .qty
                .partial_cmp(&b.qty)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortKey::Frequency => freq_days(b)
                .partial_cmp(&freq_days(a))
                .unwrap_or(std::cmp::Ordering::Equal),
            SortKey::Likely => a.likely.cmp(&b.likely),
            SortKey::Times => a.times.cmp(&b.times),
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if desc {
            ordering.reverse()
        } else {
            ordering
        }
    });
    products
}

fn freq_days(product: &Prod) -> f64 {
    if product.times < 2 {
        return f64::MAX;
    }
    (product.last - product.first).num_days() as f64 / (product.times - 1) as f64
}

fn is_shortage(product: &Prod, today: NaiveDate) -> bool {
    if product.times < 2 {
        return false;
    }
    let age = (today - product.last).num_days();
    if age > 90 {
        return false;
    }
    let every = freq_days(product);
    if !every.is_finite() || every <= 0.0 {
        return false;
    }
    let next = product.last + Duration::days(every.round() as i64);
    (next - today).num_days() <= 7
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

fn likely_kpi(shell: &Rc<RefCell<Shell>>, name: &str, likely: u8) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.add_css_class("bf-stat");
    box_.set_hexpand(true);
    let caption = gtk::Label::new(Some("LIKELY"));
    caption.add_css_class("bf-kicker");
    caption.set_xalign(0.0);
    let votes = likely_votes(shell, name, likely);
    votes.set_halign(gtk::Align::Start);
    box_.append(&caption);
    box_.append(&votes);
    box_
}

fn likely_votes(shell: &Rc<RefCell<Shell>>, name: &str, likely: u8) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    row.set_halign(gtk::Align::End);
    row.set_width_request(72);
    let value = gtk::Label::new(Some(&likely.to_string()));
    value.add_css_class("bf-brand");
    value.set_xalign(1.0);
    value.set_width_request(22);
    row.append(&value);
    row.append(&vote_button(shell, name, 1));
    row.append(&vote_button(shell, name, -1));
    row
}

fn vote_button(shell: &Rc<RefCell<Shell>>, name: &str, delta: i32) -> gtk::Button {
    let button = gtk::Button::with_label(if delta > 0 { "▲" } else { "▼" });
    button.add_css_class("bf-vote");
    button.set_tooltip_text(Some(if delta > 0 {
        "More likely to buy"
    } else {
        "Less likely to buy"
    }));
    let press = gtk::GestureClick::new();
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    let guard = shell.borrow().vote_guard.clone();
    press.connect_pressed(move |_, _, _, _| {
        guard.set(true);
    });
    button.add_controller(press);
    let shell_click = shell.clone();
    let name = name.to_string();
    button.connect_clicked(move |_| {
        if let Err(err) = shell_click.borrow().core.nudge_vote(&name, delta) {
            shell_click
                .borrow()
                .toast
                .add_toast(adw::Toast::new(&err.to_string()));
            return;
        }
        schedule(&shell_click);
    });
    button
}

fn kpi(value: &str, caption: &str) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.add_css_class("bf-stat");
    box_.set_hexpand(true);
    let caption = gtk::Label::new(Some(caption));
    caption.add_css_class("bf-kicker");
    caption.set_xalign(0.0);
    caption.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let value = gtk::Label::new(Some(value));
    value.add_css_class("bf-brand");
    value.set_xalign(0.0);
    value.set_ellipsize(gtk::pango::EllipsizeMode::End);
    box_.append(&caption);
    box_.append(&value);
    box_
}

fn sort_header(
    shell: &Rc<RefCell<Shell>>,
    text: &str,
    key: SortKey,
    width: i32,
    end: bool,
) -> gtk::Button {
    let state = shell.borrow();
    let caption = if state.sort == key {
        format!("{text} {}", if state.sort_desc { "↓" } else { "↑" })
    } else {
        text.to_string()
    };
    drop(state);
    let label = gtk::Label::new(Some(&caption));
    label.set_xalign(if end { 1.0 } else { 0.0 });
    let button = gtk::Button::new();
    button.set_child(Some(&label));
    button.add_css_class("bf-th");
    if shell.borrow().sort == key {
        button.add_css_class("on");
    }
    if width > 0 {
        button.set_width_request(width);
    }
    button.set_halign(if end {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });
    let shell_click = shell.clone();
    button.connect_clicked(move |_| {
        let mut state = shell_click.borrow_mut();
        if state.sort == key {
            state.sort_desc = !state.sort_desc;
        } else {
            state.sort = key;
            state.sort_desc = key != SortKey::Name;
        }
        drop(state);
        schedule(&shell_click);
    });
    button
}

fn shortage_header(shell: &Rc<RefCell<Shell>>) -> gtk::Button {
    let on = shell.borrow().shortage;
    let button = gtk::Button::with_label(if on { "Shortage ↓" } else { "Shortage" });
    button.add_css_class("bf-th");
    if on {
        button.add_css_class("on");
    }
    let shell_click = shell.clone();
    button.connect_clicked(move |_| {
        let mut state = shell_click.borrow_mut();
        state.shortage = !state.shortage;
        drop(state);
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
    label.set_max_width_chars(42);
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
        StoreId::AfricanEastern => "African + Eastern",
        StoreId::Careem => "Careem",
        StoreId::Mcdonalds => "McDonald's",
        StoreId::AmazonIt => "Amazon.it",
        StoreId::AmazonAe => "Amazon.ae",
    }
}

fn startup_query() -> String {
    std::env::var("OMABRING_QUERY")
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn store_logo(id: StoreId) -> gtk::Widget {
    logo_picture(id, 32)
}

fn store_logo_small(id: StoreId) -> gtk::Widget {
    logo_picture(id, 16)
}

fn logo_picture(id: StoreId, size: i32) -> gtk::Widget {
    let bytes = gtk::glib::Bytes::from_static(logo_png(id));
    if let Ok(texture) = gtk::gdk::Texture::from_bytes(&bytes) {
        if let Some(fitted) = fit_logo(&texture, size) {
            let width = fitted.width();
            let height = fitted.height();
            let picture = gtk::Picture::for_paintable(&fitted);
            picture.set_can_shrink(false);
            picture.set_size_request(width, height);
            picture.set_hexpand(false);
            picture.set_vexpand(false);
            picture.set_halign(gtk::Align::Start);
            picture.set_valign(gtk::Align::Center);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.add_css_class("bf-logo");
            if size < 24 {
                picture.add_css_class("bf-logo-sm");
            }
            return picture.upcast();
        }
    }
    let logo = gtk::Label::new(Some(store_mark(id)));
    logo.add_css_class("bf-logo");
    logo.add_css_class(logo_class(id));
    logo.set_size_request(size, size);
    logo.set_xalign(0.5);
    logo.set_yalign(0.5);
    logo.upcast()
}

fn logo_png(id: StoreId) -> &'static [u8] {
    match id {
        StoreId::Carrefour => include_bytes!("../../../assets/logos/carrefour.png"),
        StoreId::UnionCoop => include_bytes!("../../../assets/logos/unioncoop.png"),
        StoreId::Grandiose => include_bytes!("../../../assets/logos/grandiose.png"),
        StoreId::Waitrose => include_bytes!("../../../assets/logos/waitrose.png"),
        StoreId::Spinneys => include_bytes!("../../../assets/logos/spinneys.png"),
        StoreId::Mmi => include_bytes!("../../../assets/logos/mmi.png"),
        StoreId::AfricanEastern => include_bytes!("../../../assets/logos/africaneastern.png"),
        StoreId::Careem => include_bytes!("../../../assets/logos/careem.png"),
        StoreId::Mcdonalds => include_bytes!("../../../assets/logos/mcdonalds.png"),
        StoreId::AmazonIt | StoreId::AmazonAe => include_bytes!("../../../assets/logos/amazon.png"),
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
        StoreId::Careem => "C",
        StoreId::Mcdonalds => "M",
        StoreId::AmazonIt => "it",
        StoreId::AmazonAe => "ae",
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
        StoreId::Careem => "logo-careem",
        StoreId::Mcdonalds => "logo-mcdonalds",
        StoreId::AmazonIt | StoreId::AmazonAe => "logo-amazon",
    }
}

fn initial_page(core: &App) -> Page {
    let product = std::env::var("OMABRING_PRODUCT").ok();
    let page = startup_page(
        std::env::var("OMABRING_PAGE").ok().as_deref(),
        product.as_deref(),
    );
    if let Page::Product(name) = page {
        if !name.trim().is_empty() {
            return Page::Product(name);
        }
        return match latest_product_name(core) {
            Some(name) => Page::Product(name),
            None => Page::Purchases,
        };
    }
    let page_chosen = std::env::var("OMABRING_PAGE")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    if !page_chosen && chart_from(std::env::var("OMABRING_CHART").ok().as_deref()).is_some() {
        return match latest_product_name(core) {
            Some(name) => Page::Product(name),
            None => Page::Purchases,
        };
    }
    page
}

fn latest_product_name(core: &App) -> Option<String> {
    let invoices = core.list_invoices().ok()?;
    let mut best: Option<(String, String)> = None;
    for invoice in invoices {
        for line in invoice.lines {
            let replace = best
                .as_ref()
                .map(|(date, _)| invoice.purchased_on.as_str() >= date.as_str())
                .unwrap_or(true);
            if replace {
                best = Some((invoice.purchased_on.clone(), line.name));
            }
        }
    }
    best.map(|(_, name)| name)
}

fn startup_page(page: Option<&str>, product: Option<&str>) -> Page {
    if let Some(name) = product.map(str::trim).filter(|name| !name.is_empty()) {
        return Page::Product(name.to_string());
    }
    if page.map(str::trim).is_some_and(|value| !value.is_empty()) {
        return page_from(page);
    }
    Page::Dashboard
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
        (400, "carrefour", "A-0901", "Milk", 1.0, 7.20),
        (220, "waitrose", "W-0902", "Milk", 1.0, 8.10),
        (40, "carrefour", "A-1001", "Milk", 2.0, 8.50),
        (26, "carrefour", "A-1003", "Milk", 1.0, 8.75),
        (12, "union_coop", "U-2008", "Milk", 1.0, 9.10),
        (3, "carrefour", "A-1002", "Bread", 1.0, 4.25),
        (6, "union_coop", "U-2001", "Eggs", 1.0, 12.00),
        (8, "grandiose", "G-3001", "Water", 6.0, 2.15),
        (15, "spinneys", "S-5001", "Rice", 1.0, 18.40),
        (22, "mmi", "M-6001", "Bread", 2.0, 3.90),
        (26, "african_eastern", "E-7001", "Coffee", 1.0, 32.00),
        (27, "union_coop", "U-2002", "Eggs", 1.0, 11.50),
        (231, "carrefour", "C-0", "Coca Cola Zero", 1.0, 3.50),
        (228, "carrefour", "C-1", "Coca Cola Zero", 1.0, 3.50),
        (225, "carrefour", "C-2", "Coca Cola Zero", 1.0, 3.50),
        (222, "carrefour", "C-3", "Coca Cola Zero", 1.0, 3.50),
        (219, "carrefour", "C-4", "Coca Cola Zero", 1.0, 3.50),
        (216, "carrefour", "C-5", "Coca Cola Zero", 1.0, 3.50),
    ];
    for (days_ago, store, invoice_no, name, qty, price) in rows {
        let date = today - Duration::days(days_ago);
        let document = format!(
            r#"{{"store":"{store}","invoice_no":"{invoice_no}","date":"{date}","currency":"AED","items":[{{"name":"{name}","qty":{qty},"unit":"piece","unit_price":{price}}}]}}"#
        );
        let _ = core.import_document(&document);
    }
    for (name, bytes) in [
        (
            "Milk",
            include_bytes!("../../../assets/demo/milk.png").as_slice(),
        ),
        (
            "Bread",
            include_bytes!("../../../assets/demo/bread.png").as_slice(),
        ),
        (
            "Eggs",
            include_bytes!("../../../assets/demo/eggs.png").as_slice(),
        ),
        (
            "Water",
            include_bytes!("../../../assets/demo/water.png").as_slice(),
        ),
        (
            "Rice",
            include_bytes!("../../../assets/demo/rice.png").as_slice(),
        ),
        (
            "Coffee",
            include_bytes!("../../../assets/demo/coffee.png").as_slice(),
        ),
    ] {
        let _ = core.store_product_image(name, bytes);
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
    use super::{
        alt_index, chart_buckets, chart_from, nav_slot, page_at, page_from, products_from,
        startup_page, week_buckets, ChartKind, Page, SortKey,
    };
    use omabring_core::{InvoiceLine, InvoiceSummary, StoreId};
    use std::collections::HashMap;

    #[test]
    fn product_deep_link_opens_that_product() {
        assert!(matches!(startup_page(None, None), Page::Dashboard));
        assert!(matches!(
            startup_page(Some("purchases"), Some("  ")),
            Page::Purchases
        ));
        assert!(matches!(
            startup_page(Some("dashboard"), Some(" Full Fat Milk ")),
            Page::Product(name) if name == "Full Fat Milk"
        ));
    }

    #[test]
    fn every_tab_and_chart_has_a_startup_name() {
        assert!(matches!(page_from(Some("Dashboard")), Page::Dashboard));
        assert!(matches!(page_from(Some("purchases")), Page::Purchases));
        assert!(matches!(page_from(Some("STORES")), Page::Stores));
        assert!(matches!(page_from(Some("search")), Page::Search));
        assert!(matches!(page_from(Some("forecast")), Page::Forecast));
        assert!(matches!(page_from(Some("settings")), Page::Settings));
        assert!(matches!(page_from(Some("product")), Page::Product(name) if name.is_empty()));
        assert_eq!(chart_from(Some("price")), Some(ChartKind::Price));
        assert_eq!(
            chart_from(Some("price-by-store")),
            Some(ChartKind::PriceByStore)
        );
        assert_eq!(chart_from(Some("qty-month")), Some(ChartKind::QtyMonth));
        assert_eq!(chart_from(Some("spend-month")), Some(ChartKind::SpendMonth));
        assert_eq!(chart_from(Some("spend-year")), Some(ChartKind::SpendYear));
        assert_eq!(
            chart_from(Some("store-compare")),
            Some(ChartKind::StoreCompare)
        );
        assert_eq!(chart_from(Some("cumulative")), Some(ChartKind::Cumulative));
        assert_eq!(chart_from(Some("nope")), None);
    }

    #[test]
    fn weekly_bars_start_on_monday() {
        use chrono::NaiveDate;
        let start = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
        let mut by_day = HashMap::new();
        by_day.insert(start, 4.0);
        let buckets = week_buckets(start, end, &by_day);
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].key, "week:2026-10-12");
        assert_eq!(buckets[0].start, start);
        assert_eq!(buckets[0].end, end);
        assert!((buckets[0].amount - 4.0).abs() < 0.01);
    }

    #[test]
    fn column_sort_flips_direction() {
        let invoices = vec![
            sample_invoice(1, "2026-01-01", "Milk", 1.0, 8.0),
            sample_invoice(2, "2026-02-01", "Milk", 2.0, 9.0),
            sample_invoice(3, "2026-02-02", "Bread", 1.0, 4.0),
        ];
        let refs: Vec<&InvoiceSummary> = invoices.iter().collect();
        let likely = HashMap::new();
        let spent = products_from(&refs, &likely, SortKey::Spent, true);
        assert_eq!(spent[0].name, "Milk");
        let spent_asc = products_from(&refs, &likely, SortKey::Spent, false);
        assert_eq!(spent_asc[0].name, "Bread");
        let times = products_from(&refs, &likely, SortKey::Times, true);
        assert_eq!(times[0].name, "Milk");
        let units = products_from(&refs, &likely, SortKey::Qty, true);
        assert_eq!(units[0].name, "Milk");
        assert!((units[0].qty - 3.0).abs() < 0.01);
        let latest = products_from(&refs, &likely, SortKey::Last, true);
        assert_eq!(latest[0].name, "Bread");
        assert_eq!(latest[0].last.format("%Y-%m-%d").to_string(), "2026-02-02");
        let earliest = products_from(&refs, &likely, SortKey::Last, false);
        assert_eq!(earliest[0].name, "Milk");
        assert_eq!(
            earliest[0].last.format("%Y-%m-%d").to_string(),
            "2026-02-01"
        );
    }

    #[test]
    fn product_charts_cover_price_qty_spend_and_total() {
        use chrono::NaiveDate;
        use omabring_core::{ProductHistory, PurchasePoint, StorePrice};
        let item = ProductHistory {
            name: "Milk".into(),
            times: 2,
            spent: 26.5,
            avg: 8.75,
            min: 8.5,
            max: 9.0,
            first: "2024-01-15".into(),
            last: "2025-06-02".into(),
            frequency: "every 500 days".into(),
            likely: 40,
            next: String::new(),
            points: vec![
                PurchasePoint {
                    date: NaiveDate::from_ymd_opt(2024, 1, 15).unwrap(),
                    store: StoreId::Carrefour,
                    qty: 1.0,
                    unit_price: 8.5,
                    invoice_id: 1,
                    invoice_no: "A".into(),
                },
                PurchasePoint {
                    date: NaiveDate::from_ymd_opt(2025, 6, 2).unwrap(),
                    store: StoreId::UnionCoop,
                    qty: 2.0,
                    unit_price: 9.0,
                    invoice_id: 2,
                    invoice_no: "B".into(),
                },
            ],
            by_store: vec![
                StorePrice {
                    store: StoreId::Carrefour,
                    last: 8.5,
                    min: 8.5,
                    max: 8.5,
                    buys: 1,
                },
                StorePrice {
                    store: StoreId::UnionCoop,
                    last: 9.0,
                    min: 9.0,
                    max: 9.0,
                    buys: 1,
                },
            ],
        };
        let price = chart_buckets(ChartKind::Price, &item);
        assert_eq!(price.len(), 2);
        assert!((price[0].1 - 8.5).abs() < 0.01);
        let qty = chart_buckets(ChartKind::QtyMonth, &item);
        assert!((qty[1].1 - 2.0).abs() < 0.01);
        let spend = chart_buckets(ChartKind::SpendMonth, &item);
        assert!((spend[1].1 - 18.0).abs() < 0.01);
        assert_eq!(chart_buckets(ChartKind::SpendYear, &item).len(), 2);
        let total = chart_buckets(ChartKind::Cumulative, &item);
        assert!((total[1].1 - 26.5).abs() < 0.01);
        let compare = chart_buckets(ChartKind::StoreCompare, &item);
        assert_eq!(compare.len(), 2);
        assert!((compare[0].1 - 8.5).abs() < 0.01);
    }

    fn sample_invoice(id: i64, date: &str, name: &str, qty: f64, price: f64) -> InvoiceSummary {
        InvoiceSummary {
            id,
            store: StoreId::Carrefour,
            invoice_no: id.to_string(),
            purchased_on: date.into(),
            currency: "AED".into(),
            total: qty * price,
            lines: vec![InvoiceLine {
                name: name.into(),
                qty,
                unit: "piece".into(),
                unit_price: price,
                line_total: qty * price,
                product_key: String::new(),
            }],
        }
    }

    #[test]
    fn window_opens_on_dashboard() {
        assert!(matches!(page_from(None), Page::Dashboard));
        assert!(matches!(page_from(Some("")), Page::Dashboard));
        assert!(matches!(page_from(Some("search")), Page::Search));
        assert!(matches!(page_from(Some("dashboard")), Page::Dashboard));
        assert!(matches!(page_from(Some("receipts")), Page::Purchases));
        assert!(matches!(page_from(Some("service")), Page::Settings));
        assert!(matches!(page_from(Some("forecast")), Page::Forecast));
    }

    #[test]
    fn alt_numbers_follow_the_header() {
        assert_eq!(nav_slot(&Page::Dashboard), 0);
        assert_eq!(nav_slot(&Page::Purchases), 1);
        assert_eq!(nav_slot(&Page::Product("Milk".into())), 1);
        assert_eq!(nav_slot(&Page::Stores), 2);
        assert_eq!(nav_slot(&Page::Store(StoreId::AfricanEastern)), 2);
        assert_eq!(nav_slot(&Page::Search), 3);
        assert_eq!(nav_slot(&Page::Forecast), 4);
        assert_eq!(nav_slot(&Page::Settings), 5);
        assert!(matches!(page_at(0), Page::Dashboard));
        assert!(matches!(page_at(5), Page::Settings));
        assert!(matches!(page_at(6), Page::Dashboard));
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
