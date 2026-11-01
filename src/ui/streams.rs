//! The Logs and Traffic inspectors.
//!
//! Both pages keep a `gio::ListStore` that is appended to incrementally from
//! the manager's snapshot, filtered through a `gtk::CustomFilter`, and shown in
//! a recycling `gtk::ListView`, so long histories stay cheap to render.

use super::*;
use crate::backend::{LogEntry, LogLevel, TrafficEntry, TrafficOutcome};
use glib::BoxedAnyObject;
use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
};

const CSS: &str = r#"
.stream-list { background: none; }
.stream-list > row { padding: 0; }
.stream-list > row:hover { background-color: color-mix(in srgb, currentColor 4%, transparent); }
.log-line { padding: 3px 12px 3px 9px; border-left: 3px solid transparent; }
.log-line.error {
  background-color: color-mix(in srgb, var(--error-bg-color) 10%, transparent);
  border-left-color: var(--error-bg-color);
}
.log-line.warning {
  background-color: color-mix(in srgb, var(--warning-bg-color) 10%, transparent);
  border-left-color: var(--warning-bg-color);
}
.log-line.error .log-message { color: var(--error-color); }
.log-line.warning .log-message { color: var(--warning-color); }
.log-line.debug .log-message { opacity: 0.6; }
.log-time { font-family: monospace; font-size: 0.85em; opacity: 0.55; }
.log-message { font-family: monospace; font-size: 0.92em; }
.log-repeat {
  font-size: 0.8em; font-weight: bold; padding: 0 6px; border-radius: 99px;
  background-color: color-mix(in srgb, currentColor 12%, transparent);
}
.source-tag {
  font-size: 0.8em; font-weight: bold; padding: 1px 7px; border-radius: 99px;
}
.source-tag.c0 { color: var(--blue-3); background-color: color-mix(in srgb, var(--blue-3) 15%, transparent); }
.source-tag.c1 { color: var(--green-5); background-color: color-mix(in srgb, var(--green-4) 15%, transparent); }
.source-tag.c2 { color: var(--purple-2); background-color: color-mix(in srgb, var(--purple-3) 18%, transparent); }
.source-tag.c3 { color: var(--orange-4); background-color: color-mix(in srgb, var(--orange-4) 15%, transparent); }
.source-tag.c4 { color: var(--brown-2); background-color: color-mix(in srgb, var(--brown-3) 18%, transparent); }
.source-tag.c5 { color: color-mix(in srgb, currentColor 65%, transparent); background-color: color-mix(in srgb, currentColor 10%, transparent); }
.traffic-line { padding: 8px 12px; }
.traffic-line.failed { background-color: color-mix(in srgb, var(--error-bg-color) 8%, transparent); }
.traffic-status.ok { color: var(--success-color); }
.traffic-status.error { color: var(--error-color); }
.traffic-status.tool-error { color: var(--warning-color); }
.traffic-status.notification { opacity: 0.5; }
.traffic-error { color: var(--error-color); font-size: 0.9em; }
.traffic-duration.slow { color: var(--warning-color); font-weight: bold; }
.stat-tiles > flowboxchild,
.stat-tiles > flowboxchild:hover,
.stat-tiles > flowboxchild:active { background: none; padding: 0; }
.stat-card { padding: 12px 8px; }
.stat-card .stat-value { font-size: 1.6em; font-weight: 800; }
.stat-card.bad .stat-value { color: var(--error-color); }
.json-view { font-family: monospace; font-size: 0.9em; padding: 12px; background: none; }
statuspage.compact > scrolledwindow > viewport > box { margin: 0 12px; }
statuspage.compact .icon { -gtk-icon-size: 64px; opacity: 0.45; margin-bottom: 12px; }
statuspage.compact .title { font-size: 1.25em; }
"#;

pub(super) fn ensure_css() {
    thread_local! { static LOADED: Cell<bool> = const { Cell::new(false) }; }
    if LOADED.replace(true) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// Children of a widget in order.
fn kids(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
    let mut out = vec![];
    let mut child = widget.first_child();
    while let Some(c) = child {
        child = c.next_sibling();
        out.push(c);
    }
    out
}

fn label(classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

/// Highlights case-insensitive occurrences of `needle` in `text` as Pango markup.
fn highlight(text: &str, needle: &str) -> String {
    let lower = text.to_lowercase();
    if needle.is_empty() || lower.len() != text.len() {
        return glib::markup_escape_text(text).to_string();
    }
    let mut out = String::new();
    let mut last = 0;
    for (start, _) in lower.match_indices(needle) {
        if start < last {
            continue;
        }
        let end = start + needle.len();
        out.push_str(&glib::markup_escape_text(&text[last..start]));
        out.push_str("<span bgcolor=\"#f6d32d\" bgalpha=\"45%\" weight=\"bold\">");
        out.push_str(&glib::markup_escape_text(&text[start..end]));
        out.push_str("</span>");
        last = end;
    }
    out.push_str(&glib::markup_escape_text(&text[last..]));
    out
}

fn source_class(source: &str) -> &'static str {
    let base = source.split_whitespace().last().unwrap_or(source);
    match base {
        "stdout" => return "c0",
        "stderr" => return "c3",
        "Manager" | "Bridge" | "Discovery" | "MCP" => return "c5",
        "Tunnel" => return "c2",
        _ => {}
    }
    let hash = source
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    ["c0", "c1", "c2", "c3", "c4"][(hash % 5) as usize]
}

fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn format_ms(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{:.0} s", ms as f64 / 1000.0)
    } else if ms >= 1000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

const SLOW_MS: u64 = 1000;
const SOURCE_WIDTH: i32 = 112;
/// Below this content width the inspectors switch to a stacked layout.
const NARROW_SP: f64 = 560.0;

/// Wraps a page so it can adapt to its own width rather than the window's.
fn adaptive(
    child: &impl IsA<gtk::Widget>,
    on_change: impl Fn(bool) + Clone + 'static,
) -> adw::BreakpointBin {
    let bin = adw::BreakpointBin::builder()
        .width_request(360)
        .height_request(200)
        .child(child)
        .build();
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        NARROW_SP,
        adw::LengthUnit::Sp,
    ));
    {
        let on_change = on_change.clone();
        breakpoint.connect_apply(move |_| on_change(true));
    }
    breakpoint.connect_unapply(move |_| on_change(false));
    bin.add_breakpoint(breakpoint);
    bin
}

/// Re-binds realized rows: all of them, or only the one showing `only`. GTK
/// keeps rows bound when their item is unchanged, so highlight, layout and
/// in-place item changes have to be pushed explicitly.
type Rebind = Rc<dyn Fn(Option<&glib::Object>)>;

fn finish_factory(
    factory: gtk::SignalListItemFactory,
    bound: Rc<RefCell<Vec<glib::WeakRef<gtk::ListItem>>>>,
    bind: Rc<impl Fn(&gtk::ListItem) + 'static>,
) -> (gtk::SignalListItemFactory, Rebind) {
    {
        let bind = bind.clone();
        factory.connect_bind(move |_, item| bind(item.downcast_ref::<gtk::ListItem>().unwrap()));
    }
    let rebind = Rc::new(move |only: Option<&glib::Object>| {
        bound.borrow_mut().retain(|weak| {
            let Some(item) = weak.upgrade() else {
                return false;
            };
            if let Some(obj) = item.item()
                && only.is_none_or(|o| *o == obj)
            {
                bind(&item);
            }
            true
        });
    });
    (factory, rebind)
}

/// Shared scaffolding: a filtered list in a scroller that follows new rows
/// while the user is at the bottom, with a floating "jump to latest" button.
struct StreamList {
    store: gio::ListStore,
    filter: gtk::CustomFilter,
    filtered: gtk::FilterListModel,
    view: gtk::ListView,
    scroll: gtk::ScrolledWindow,
    overlay: gtk::Overlay,
    rebind: Rebind,
}
impl StreamList {
    fn new(
        (factory, rebind): (gtk::SignalListItemFactory, Rebind),
        filter: gtk::CustomFilter,
    ) -> Self {
        let store = gio::ListStore::new::<BoxedAnyObject>();
        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let view = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(filtered.clone())))
            .factory(&factory)
            .css_classes(["stream-list"])
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let overlay = gtk::Overlay::builder().child(&scroll).build();
        let jump = gtk::Button::builder()
            .icon_name("go-bottom-symbolic")
            .tooltip_text("Jump to Latest")
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(18)
            .margin_bottom(18)
            .css_classes(["osd", "circular"])
            .visible(false)
            .build();
        overlay.add_overlay(&jump);
        let list = Self {
            store,
            filter,
            filtered,
            view,
            scroll,
            overlay,
            rebind,
        };
        {
            let view = list.view.clone();
            let filtered = list.filtered.clone();
            jump.connect_clicked(move |_| scroll_to_end(&view, &filtered));
        }
        let adjustment = list.scroll.vadjustment();
        let update = {
            let jump = jump.clone();
            move |a: &gtk::Adjustment| jump.set_visible(!at_bottom(a))
        };
        adjustment.connect_value_changed(update.clone());
        adjustment.connect_upper_notify(update);
        list
    }
    /// Re-runs the filter and rebinds rows so highlights update.
    fn refilter(&self) {
        self.filter.changed(gtk::FilterChange::Different);
        (self.rebind)(None);
    }
    fn following(&self) -> bool {
        at_bottom(&self.scroll.vadjustment())
    }
    /// The page has left the window, so its refresh timer should stop.
    /// (Checking a weak ref is not enough: signal closures keep `self` alive.)
    fn detached(&self) -> bool {
        self.view.root().is_none()
    }
    /// Covered by another page; refreshing can wait until it is shown again.
    fn hidden(&self) -> bool {
        !self.view.is_mapped()
    }
    fn follow(&self) {
        let view = self.view.clone();
        let filtered = self.filtered.clone();
        glib::idle_add_local_once(move || scroll_to_end(&view, &filtered));
    }
    fn visible(&self) -> Vec<BoxedAnyObject> {
        (0..self.filtered.n_items())
            .filter_map(|i| self.filtered.item(i).and_downcast())
            .collect()
    }
}
fn at_bottom(a: &gtk::Adjustment) -> bool {
    a.value() >= a.upper() - a.page_size() - 24.0
}
fn scroll_to_end(view: &gtk::ListView, model: &gtk::FilterListModel) {
    let n = model.n_items();
    if n > 0 {
        view.scroll_to(n - 1, gtk::ListScrollFlags::NONE, None);
    }
}

fn empty_page(icon: &str, title: &str, description: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(description)
        .css_classes(["compact"])
        .build()
}

/// A header bar with a title, the server name and a "more" menu.
fn header(title: &str, subtitle: &str, menu: &gio::Menu) -> adw::HeaderBar {
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(title, subtitle)));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            .menu_model(menu)
            .build(),
    );
    header
}

fn filter_bar(search: &gtk::SearchEntry, extra: &impl IsA<gtk::Widget>) -> adw::Clamp {
    let bar = gtk::Box::builder()
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    bar.append(search);
    bar.append(extra);
    adw::Clamp::builder().maximum_size(900).child(&bar).build()
}

fn level_toggles(names: &[(&str, &str)]) -> (adw::ToggleGroup, Vec<adw::Toggle>) {
    let group = adw::ToggleGroup::builder()
        .halign(gtk::Align::Center)
        .homogeneous(false)
        .build();
    let toggles = names
        .iter()
        .map(|(name, label)| {
            let toggle = adw::Toggle::builder().name(*name).label(*label).build();
            group.add(toggle.clone());
            toggle
        })
        .collect();
    group.set_active_name(Some(names[0].0));
    (group, toggles)
}

/// A dropdown whose first entry means "everything"; entries are only appended
/// so the selection survives new sources arriving.
struct Choices {
    list: gtk::StringList,
    dropdown: gtk::DropDown,
}
impl Choices {
    fn new(all: &str, tooltip: &str) -> Self {
        let list = gtk::StringList::new(&[all]);
        let dropdown = gtk::DropDown::builder()
            .model(&list)
            .tooltip_text(tooltip)
            .build();
        Self { list, dropdown }
    }
    fn ensure(&self, value: &str) {
        let n = self.list.n_items();
        if !(1..n).any(|i| self.list.string(i).as_deref() == Some(value)) {
            self.list.append(value);
        }
    }
    fn selected(&self) -> Option<String> {
        match self.dropdown.selected() {
            0 | gtk::INVALID_LIST_POSITION => None,
            i => self.list.string(i).map(|s| s.to_string()),
        }
    }
}

fn save_text(ui: &Ui, title: &str, name: &str, text: String) {
    let picker = gtk::FileDialog::builder()
        .title(title)
        .modal(true)
        .initial_name(name)
        .build();
    let overlay = ui.overlay.clone();
    picker.save(Some(&ui.window), None::<&gio::Cancellable>, move |result| {
        let toast = match result {
            Ok(file) => match file.path() {
                Some(path) => match std::fs::write(&path, text) {
                    Ok(()) => format!("Saved to {}", path.display()),
                    Err(e) => format!("Could not save: {e}"),
                },
                None => "Choose a local file".into(),
            },
            Err(e)
                if e.matches(gtk::DialogError::Dismissed)
                    || e.matches(gtk::DialogError::Cancelled) =>
            {
                return;
            }
            Err(e) => format!("Could not save: {e}"),
        };
        overlay.add_toast(adw::Toast::new(&toast));
    });
}

fn action(group: &gio::SimpleActionGroup, name: &str, f: impl Fn() + 'static) {
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| f());
    group.add_action(&action);
}

// ───────────────────────────── Logs ─────────────────────────────

struct LogItem {
    entry: Arc<LogEntry>,
    /// Consecutive identical lines are folded into one row.
    count: Cell<u32>,
    last_seq: Cell<u64>,
}

#[derive(Default)]
struct LogFilter {
    needle: String,
    level: String,
    source: Option<String>,
}
impl LogFilter {
    fn matches(&self, e: &LogEntry) -> bool {
        (match self.level.as_str() {
            "errors" => e.level == LogLevel::Error,
            "warnings" => e.level >= LogLevel::Warning,
            _ => true,
        }) && self.source.as_ref().is_none_or(|s| s == &e.source)
            && (self.needle.is_empty()
                || e.message.to_lowercase().contains(&self.needle)
                || e.source.to_lowercase().contains(&self.needle))
    }
}

fn log_factory(
    state: Rc<RefCell<LogFilter>>,
    narrow: Rc<Cell<bool>>,
) -> (gtk::SignalListItemFactory, Rebind) {
    let factory = gtk::SignalListItemFactory::new();
    let bound: Rc<RefCell<Vec<glib::WeakRef<gtk::ListItem>>>> = Rc::default();
    factory.connect_setup({
        let bound = bound.clone();
        move |_, item| {
            bound
                .borrow_mut()
                .push(item.downcast_ref::<gtk::ListItem>().unwrap().downgrade());
            let row = gtk::Box::builder()
                .spacing(10)
                .css_classes(["log-line"])
                .build();
            let meta = gtk::Box::builder()
                .spacing(10)
                .valign(gtk::Align::Start)
                .build();
            let time = label(&["log-time"]);
            time.set_valign(gtk::Align::Center);
            let source_box = gtk::Box::builder()
                .width_request(SOURCE_WIDTH)
                .valign(gtk::Align::Center)
                .build();
            let source = label(&["source-tag"]);
            source.set_halign(gtk::Align::Start);
            source.set_ellipsize(gtk::pango::EllipsizeMode::End);
            source.set_max_width_chars(10);
            source_box.append(&source);
            meta.append(&time);
            meta.append(&source_box);
            let message = gtk::Label::builder()
                .xalign(0.0)
                .hexpand(true)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .valign(gtk::Align::Start)
                .css_classes(["log-message"])
                .build();
            let repeat = label(&["log-repeat"]);
            repeat.set_valign(gtk::Align::Start);
            repeat.set_tooltip_text(Some("Repeated consecutively"));
            row.append(&meta);
            row.append(&message);
            row.append(&repeat);
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(Some(&row));
        }
    });
    let bind = Rc::new(move |item: &gtk::ListItem| {
        let (Some(row), Some(obj)) = (item.child(), item.item().and_downcast::<BoxedAnyObject>())
        else {
            return;
        };
        let data = obj.borrow::<LogItem>();
        let e = &data.entry;
        let parts = kids(&row);
        let [meta, message, repeat] = parts.as_slice() else {
            return;
        };
        let meta_parts = kids(meta);
        let [time, source_box] = meta_parts.as_slice() else {
            return;
        };
        // Narrow pages stack the time and source above the message.
        let narrow = narrow.get();
        let outer = row.downcast_ref::<gtk::Box>().unwrap();
        outer.set_orientation(if narrow {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        outer.set_spacing(if narrow { 2 } else { 10 });
        source_box.set_width_request(if narrow { -1 } else { SOURCE_WIDTH });
        repeat.set_halign(gtk::Align::Start);
        for class in ["error", "warning", "debug"] {
            row.remove_css_class(class);
        }
        match e.level {
            LogLevel::Error => row.add_css_class("error"),
            LogLevel::Warning => row.add_css_class("warning"),
            LogLevel::Debug => row.add_css_class("debug"),
            LogLevel::Info => {}
        }
        let time = time.downcast_ref::<gtk::Label>().unwrap();
        time.set_label(&e.time);
        time.set_tooltip_text((!e.date.is_empty()).then_some(e.date.as_str()));
        let source = source_box
            .first_child()
            .and_downcast::<gtk::Label>()
            .unwrap();
        source.set_label(&e.source);
        source.set_tooltip_text(Some(&e.source));
        for class in ["c0", "c1", "c2", "c3", "c4", "c5"] {
            source.remove_css_class(class);
        }
        source.add_css_class(source_class(&e.source));
        message
            .downcast_ref::<gtk::Label>()
            .unwrap()
            .set_markup(&highlight(&e.message, &state.borrow().needle));
        let repeat = repeat.downcast_ref::<gtk::Label>().unwrap();
        let count = data.count.get();
        repeat.set_visible(count > 1);
        repeat.set_label(&format!("×{count}"));
    });
    finish_factory(factory, bound, bind)
}

impl Ui {
    pub(super) fn logs(self: &Rc<Self>) {
        let Some(id) = self.id() else {
            return;
        };
        ensure_css();
        let server_name = self.manager.server(&id).map(|s| s.name).unwrap_or_default();
        let state = Rc::new(RefCell::new(LogFilter {
            level: "all".into(),
            ..Default::default()
        }));
        let filter = {
            let state = state.clone();
            gtk::CustomFilter::new(move |obj| {
                let obj = obj.downcast_ref::<BoxedAnyObject>().unwrap();
                state.borrow().matches(&obj.borrow::<LogItem>().entry)
            })
        };
        let narrow = Rc::new(Cell::new(false));
        let list = Rc::new(StreamList::new(
            log_factory(state.clone(), narrow.clone()),
            filter,
        ));

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&list.overlay, Some("lines"));
        stack.add_named(
            &empty_page(
                "utilities-terminal-symbolic",
                "No Logs Yet",
                "Output from the server process, Marshal and any custom commands will appear here.",
            ),
            Some("empty"),
        );
        stack.add_named(
            &empty_page(
                "system-search-symbolic",
                "No Matching Lines",
                "Try a different search, level or source.",
            ),
            Some("no-results"),
        );

        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Copy Visible Lines"), Some("logs.copy"));
        section.append(Some("Save to File…"), Some("logs.save"));
        section.append(Some("Open Log File"), Some("logs.open"));
        menu.append_section(None, &section);
        let danger = gio::Menu::new();
        danger.append(Some("Clear Logs…"), Some("logs.clear"));
        menu.append_section(None, &danger);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header("Logs", &server_name, &menu));
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search logs")
            .hexpand(true)
            .build();
        let sources = Choices::new("All Sources", "Source");
        toolbar.add_top_bar(&filter_bar(&search, &sources.dropdown));
        toolbar.set_content(Some(&stack));
        let bottom = gtk::ActionBar::new();
        let (levels, toggles) = level_toggles(&[
            ("all", "All"),
            ("warnings", "Warnings"),
            ("errors", "Errors"),
        ]);
        bottom.set_center_widget(Some(&levels));
        toolbar.add_bottom_bar(&bottom);
        let page = {
            let list = list.clone();
            adaptive(&toolbar, move |is_narrow| {
                narrow.set(is_narrow);
                list.refilter();
            })
        };
        self.navigation
            .push(&adw::NavigationPage::new(&page, "Logs"));

        let actions = gio::SimpleActionGroup::new();
        {
            let list = list.clone();
            let overlay = self.overlay.clone();
            let window = self.window.clone();
            action(&actions, "copy", move || {
                window.clipboard().set_text(&visible_logs(&list));
                overlay.add_toast(adw::Toast::new("Copied visible lines"));
            });
        }
        {
            let list = list.clone();
            let ui = Rc::downgrade(self);
            action(&actions, "save", move || {
                if let Some(ui) = ui.upgrade() {
                    save_text(&ui, "Save Logs", "marshal-logs.log", visible_logs(&list));
                }
            });
        }
        {
            let path = self.manager.log_path(&id);
            let window = self.window.clone();
            let overlay = self.overlay.clone();
            action(&actions, "open", move || {
                if !path.exists() {
                    overlay.add_toast(adw::Toast::new("No log file has been written yet"));
                    return;
                }
                gtk::FileLauncher::new(Some(&gio::File::for_path(&path))).launch(
                    Some(&window),
                    None::<&gio::Cancellable>,
                    |_| {},
                );
            });
        }
        {
            let manager = self.manager.clone();
            let window = self.window.clone();
            let id = id.clone();
            action(&actions, "clear", move || {
                let dialog = adw::AlertDialog::builder()
                    .heading("Clear Logs?")
                    .body("All log history for this server, including the saved log file, will be deleted.")
                    .build();
                dialog.add_responses(&[("cancel", "Cancel"), ("clear", "Clear")]);
                dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
                dialog.set_default_response(Some("cancel"));
                let manager = manager.clone();
                let id = id.clone();
                dialog.connect_response(Some("clear"), move |_, _| manager.clear_logs(&id));
                dialog.present(Some(&window));
            });
        }
        toolbar.insert_action_group("logs", Some(&actions));

        {
            let state = state.clone();
            let list = list.clone();
            search.connect_search_changed(move |s| {
                state.borrow_mut().needle = s.text().to_lowercase();
                list.refilter();
            });
        }
        let sources = Rc::new(sources);
        {
            let state = state.clone();
            let list = list.clone();
            let choices = sources.clone();
            sources.dropdown.connect_selected_notify(move |_| {
                state.borrow_mut().source = choices.selected();
                list.refilter();
            });
        }
        {
            let state = state.clone();
            let list = list.clone();
            levels.connect_active_name_notify(move |g| {
                state.borrow_mut().level = g.active_name().unwrap_or_default().to_string();
                list.refilter();
            });
        }

        let manager = self.manager.clone();
        let mut last_seq = 0u64;
        let mut revision = None;
        let mut first = true;
        // Running totals, adjusted as rows arrive and age out, so a tick does
        // not have to walk the whole store.
        let (mut warnings, mut errors) = (0i64, 0i64);
        let mut tick = move || {
            let show = |list: &StreamList| {
                stack.set_visible_child_name(if list.store.n_items() == 0 {
                    "empty"
                } else if list.filtered.n_items() == 0 {
                    "no-results"
                } else {
                    "lines"
                });
            };
            let mut tally = |level: LogLevel, delta: i64| match level {
                LogLevel::Error => errors += delta,
                LogLevel::Warning => warnings += delta,
                _ => {}
            };
            if !first && list.detached() {
                return glib::ControlFlow::Break;
            }
            if !first && list.hidden() {
                return glib::ControlFlow::Continue;
            }
            let store = &list.store;
            let Some((rev, logs)) = manager.stream(&id, revision, |s| &s.logs) else {
                show(&list);
                return glib::ControlFlow::Continue;
            };
            revision = Some(rev);
            let following = list.following();
            let mut changed = false;
            // Drop rows the manager has already discarded (or cleared).
            let front = logs.front().map_or(u64::MAX, |e| e.seq);
            let mut drop = 0;
            while drop < store.n_items() {
                let obj = store.item(drop).and_downcast::<BoxedAnyObject>().unwrap();
                let item = obj.borrow::<LogItem>();
                if item.last_seq.get() >= front {
                    break;
                }
                tally(item.entry.level, -(item.count.get() as i64));
                drop += 1;
            }
            if drop > 0 {
                store.splice(0, drop, &[] as &[glib::Object]);
                changed = true;
            }
            if logs.is_empty() {
                last_seq = 0;
            }
            let mut fresh: Vec<BoxedAnyObject> = vec![];
            let mut last: Option<BoxedAnyObject> = store
                .n_items()
                .checked_sub(1)
                .and_then(|i| store.item(i))
                .and_downcast();
            let mut bumped = None;
            let seen = last_seq;
            for entry in logs.iter().filter(|e| e.seq > seen) {
                last_seq = entry.seq;
                sources.ensure(&entry.source);
                tally(entry.level, 1);
                if let Some(obj) = &last {
                    let item = obj.borrow::<LogItem>();
                    if item.entry.source == entry.source && item.entry.message == entry.message {
                        item.count.set(item.count.get() + 1);
                        item.last_seq.set(entry.seq);
                        if fresh.is_empty() {
                            bumped = Some(obj.clone());
                        }
                        continue;
                    }
                }
                let obj = BoxedAnyObject::new(LogItem {
                    entry: entry.clone(),
                    count: Cell::new(1),
                    last_seq: Cell::new(entry.seq),
                });
                fresh.push(obj.clone());
                last = Some(obj);
            }
            if let Some(obj) = bumped {
                (list.rebind)(Some(obj.upcast_ref()));
                changed = true;
            }
            if !fresh.is_empty() {
                store.extend_from_slice(&fresh);
                changed = true;
            }
            if changed || first {
                toggles[0].set_label(Some(&format!("All  {}", group_thousands(logs.len()))));
                toggles[1].set_label(Some(&format!(
                    "Warnings  {}",
                    group_thousands(warnings.max(0) as usize)
                )));
                toggles[2].set_label(Some(&format!(
                    "Errors  {}",
                    group_thousands(errors.max(0) as usize)
                )));
                if following || first {
                    list.follow();
                }
                first = false;
            }
            show(&list);
            glib::ControlFlow::Continue
        };
        tick();
        glib::timeout_add_local(std::time::Duration::from_millis(300), tick);
    }
}

fn visible_logs(list: &StreamList) -> String {
    list.visible()
        .into_iter()
        .map(|o| {
            let item = o.borrow::<LogItem>();
            match item.count.get() {
                1 => item.entry.render(),
                n => format!("{}  (×{n})", item.entry.render()),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ──────────────────────────── Traffic ────────────────────────────

#[derive(Default)]
struct TrafficFilter {
    needle: String,
    mode: String,
    client: Option<String>,
}
impl TrafficFilter {
    fn matches(&self, e: &TrafficEntry) -> bool {
        (match self.mode.as_str() {
            "failed" => e.failed(),
            "slow" => e.duration_ms.is_some_and(|d| d >= SLOW_MS),
            _ => true,
        }) && self.client.as_ref().is_none_or(|c| c == &e.client)
            && (self.needle.is_empty()
                || e.method.to_lowercase().contains(&self.needle)
                || e.target
                    .as_ref()
                    .is_some_and(|t| t.to_lowercase().contains(&self.needle))
                || e.failure()
                    .is_some_and(|f| f.to_lowercase().contains(&self.needle)))
    }
}

fn status_icon(e: &TrafficEntry) -> (&'static str, &'static str, &'static str) {
    match e.outcome {
        TrafficOutcome::Ok => ("object-select-symbolic", "ok", "Succeeded"),
        TrafficOutcome::ToolError(_) => (
            "dialog-warning-symbolic",
            "tool-error",
            "Tool reported an error",
        ),
        TrafficOutcome::Error(_) => ("dialog-error-symbolic", "error", "Failed"),
        TrafficOutcome::Notification => ("mail-send-symbolic", "notification", "Notification"),
    }
}

fn traffic_factory(state: Rc<RefCell<TrafficFilter>>) -> (gtk::SignalListItemFactory, Rebind) {
    let factory = gtk::SignalListItemFactory::new();
    let bound: Rc<RefCell<Vec<glib::WeakRef<gtk::ListItem>>>> = Rc::default();
    factory.connect_setup({
        let bound = bound.clone();
        move |_, item| {
            bound
                .borrow_mut()
                .push(item.downcast_ref::<gtk::ListItem>().unwrap().downgrade());
            let row = gtk::Box::builder()
                .spacing(12)
                .css_classes(["traffic-line"])
                .build();
            let icon = gtk::Image::builder()
                .valign(gtk::Align::Center)
                .css_classes(["traffic-status"])
                .build();
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .hexpand(true)
                .valign(gtk::Align::Center)
                .build();
            let title = label(&["heading"]);
            title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            let subtitle = label(&["dim-label", "caption"]);
            subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let error = label(&["traffic-error"]);
            error.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&title);
            text.append(&subtitle);
            text.append(&error);
            let side = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .valign(gtk::Align::Center)
                .build();
            let duration = label(&["traffic-duration", "numeric"]);
            duration.set_xalign(1.0);
            let time = label(&["dim-label", "caption", "numeric"]);
            time.set_xalign(1.0);
            side.append(&duration);
            side.append(&time);
            row.append(&icon);
            row.append(&text);
            row.append(&side);
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(Some(&row));
        }
    });
    let bind = Rc::new(move |item: &gtk::ListItem| {
        let (Some(row), Some(obj)) = (item.child(), item.item().and_downcast::<BoxedAnyObject>())
        else {
            return;
        };
        let e = obj.borrow::<Arc<TrafficEntry>>();
        let parts = kids(&row);
        let [icon, text, side] = parts.as_slice() else {
            return;
        };
        if e.failed() {
            row.add_css_class("failed");
        } else {
            row.remove_css_class("failed");
        }
        let icon = icon.downcast_ref::<gtk::Image>().unwrap();
        let (name, class, tip) = status_icon(&e);
        icon.set_icon_name(Some(name));
        icon.set_tooltip_text(Some(tip));
        for c in ["ok", "error", "tool-error", "notification"] {
            icon.remove_css_class(c);
        }
        icon.add_css_class(class);
        let needle = state.borrow().needle.clone();
        let texts = kids(text);
        let title = texts[0].downcast_ref::<gtk::Label>().unwrap();
        let subtitle = texts[1].downcast_ref::<gtk::Label>().unwrap();
        let error = texts[2].downcast_ref::<gtk::Label>().unwrap();
        let kind = if e.outcome == TrafficOutcome::Notification {
            format!("{} (notification)", e.method)
        } else {
            e.method.clone()
        };
        match &e.target {
            Some(target) => {
                title.set_markup(&highlight(target, &needle));
                subtitle.set_markup(&format!(
                    "{} · {}",
                    highlight(&kind, &needle),
                    glib::markup_escape_text(&e.client)
                ));
            }
            None => {
                title.set_markup(&highlight(&e.method, &needle));
                subtitle.set_markup(&format!(
                    "{}{}",
                    if e.outcome == TrafficOutcome::Notification {
                        "notification · "
                    } else {
                        ""
                    },
                    glib::markup_escape_text(&e.client)
                ));
            }
        }
        error.set_visible(e.failed());
        if let Some(failure) = e.failure() {
            error.set_markup(&highlight(failure, &needle));
            error.set_tooltip_text(Some(failure));
        }
        let sides = kids(side);
        let duration = sides[0].downcast_ref::<gtk::Label>().unwrap();
        let time = sides[1].downcast_ref::<gtk::Label>().unwrap();
        duration.set_label(&e.duration_ms.map(format_ms).unwrap_or_default());
        if e.duration_ms.is_some_and(|d| d >= SLOW_MS) {
            duration.add_css_class("slow");
        } else {
            duration.remove_css_class("slow");
        }
        time.set_label(e.time.split('.').next().unwrap_or_default());
        time.set_tooltip_text(Some(&e.time));
    });
    finish_factory(factory, bound, bind)
}

/// Aggregates for the summary view.
#[derive(Default)]
struct Bucket {
    calls: usize,
    failed: usize,
    durations: Vec<u64>,
}
impl Bucket {
    fn add(&mut self, e: &TrafficEntry) {
        self.calls += 1;
        self.failed += e.failed() as usize;
        if let Some(d) = e.duration_ms {
            self.durations.push(d);
        }
    }
    fn latency(&mut self) -> Option<(u64, u64)> {
        self.durations.sort_unstable();
        (!self.durations.is_empty()).then(|| {
            (
                percentile(&self.durations, 0.5),
                percentile(&self.durations, 0.95),
            )
        })
    }
}

fn stat_card(value: &str, caption: &str, bad: bool) -> gtk::Box {
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .css_classes(["card", "stat-card"])
        .build();
    if bad {
        card.add_css_class("bad");
    }
    let value = gtk::Label::builder()
        .label(value)
        .css_classes(["stat-value", "numeric"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    card.append(&value);
    card.append(
        &gtk::Label::builder()
            .label(caption)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    card
}

impl Ui {
    pub(super) fn traffic(self: &Rc<Self>) {
        let Some(id) = self.id() else {
            return;
        };
        ensure_css();
        let state = Rc::new(RefCell::new(TrafficFilter {
            mode: "all".into(),
            ..Default::default()
        }));
        let filter = {
            let state = state.clone();
            gtk::CustomFilter::new(move |obj| {
                let obj = obj.downcast_ref::<BoxedAnyObject>().unwrap();
                state.borrow().matches(&obj.borrow::<Arc<TrafficEntry>>())
            })
        };
        let list = Rc::new(StreamList::new(traffic_factory(state.clone()), filter));
        list.view.set_single_click_activate(true);
        {
            let ui = Rc::downgrade(self);
            let filtered = list.filtered.clone();
            list.view.connect_activate(move |_, position| {
                if let (Some(ui), Some(obj)) = (
                    ui.upgrade(),
                    filtered.item(position).and_downcast::<BoxedAnyObject>(),
                ) {
                    let entry = obj.borrow::<Arc<TrafficEntry>>().clone();
                    ui.traffic_detail(&entry);
                }
            });
        }

        // Requests view.
        let requests_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        requests_stack.add_named(&list.overlay, Some("lines"));
        requests_stack.add_named(
            &empty_page(
                "system-search-symbolic",
                "No Matching Requests",
                "Try a different search, client or filter.",
            ),
            Some("no-results"),
        );
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search methods, tools, errors")
            .hexpand(true)
            .build();
        let clients = Rc::new(Choices::new("All Clients", "Client"));
        let requests = adw::ToolbarView::new();
        requests.add_top_bar(&filter_bar(&search, &clients.dropdown));
        requests.set_content(Some(&requests_stack));
        let bottom = gtk::ActionBar::new();
        let (modes, toggles) =
            level_toggles(&[("all", "All"), ("failed", "Failed"), ("slow", "Slow")]);
        bottom.set_center_widget(Some(&modes));
        requests.add_bottom_bar(&bottom);

        // Summary view.
        let summary_page = adw::PreferencesPage::new();
        let summary_scroll_holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let cards_group = adw::PreferencesGroup::new();
        cards_group.add(&summary_scroll_holder);
        summary_page.add(&cards_group);
        let methods_group = adw::PreferencesGroup::builder()
            .title("Tools and Methods")
            .description("Select one to see its requests")
            .build();
        let clients_group = adw::PreferencesGroup::builder().title("Clients").build();
        let errors_group = adw::PreferencesGroup::builder()
            .title("Recent Failures")
            .build();
        summary_page.add(&methods_group);
        summary_page.add(&errors_group);
        summary_page.add(&clients_group);

        let views = adw::ViewStack::builder().hhomogeneous(false).build();
        views.add_titled_with_icon(
            &requests,
            Some("requests"),
            "Requests",
            "view-list-symbolic",
        );
        views.add_titled_with_icon(
            &summary_page,
            Some("summary"),
            "Summary",
            "x-office-spreadsheet-symbolic",
        );
        let outer = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        outer.add_named(&views, Some("content"));
        outer.add_named(
            &empty_page(
                "network-transmit-receive-symbolic",
                "No Traffic Yet",
                "Requests from Marshal and from MCP clients connected through Marshal's local bridge will appear here, with their parameters, responses and timings.",
            ),
            Some("empty"),
        );

        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Copy Visible as Text"), Some("traffic.copy"));
        section.append(Some("Export as JSON…"), Some("traffic.export"));
        menu.append_section(None, &section);
        let danger = gio::Menu::new();
        danger.append(Some("Clear Traffic"), Some("traffic.clear"));
        menu.append_section(None, &danger);
        let server_name = self.manager.server(&id).map(|s| s.name).unwrap_or_default();
        let head = header("Traffic", &server_name, &menu);
        let switcher = adw::ToggleGroup::builder().build();
        let view_toggles = [
            ("requests", "Requests", "view-list-symbolic"),
            ("summary", "Summary", "view-grid-symbolic"),
        ]
        .map(|(name, title, icon)| {
            let toggle = adw::Toggle::builder()
                .name(name)
                .label(title)
                .tooltip(title)
                .build();
            switcher.add(toggle.clone());
            (toggle, title, icon)
        });
        switcher.set_active_name(Some("requests"));
        switcher
            .bind_property("active-name", &views, "visible-child-name")
            .bidirectional()
            .sync_create()
            .build();
        head.set_title_widget(Some(&switcher));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&head);
        toolbar.set_content(Some(&outer));
        // Narrow pages only have room for icons in the header.
        let page = adaptive(&toolbar, move |narrow| {
            for (toggle, title, icon) in &view_toggles {
                toggle.set_label(if narrow { None } else { Some(*title) });
                toggle.set_icon_name(if narrow { Some(*icon) } else { None });
            }
        });
        self.navigation
            .push(&adw::NavigationPage::new(&page, "Traffic"));

        let actions = gio::SimpleActionGroup::new();
        {
            let list = list.clone();
            let window = self.window.clone();
            let overlay = self.overlay.clone();
            action(&actions, "copy", move || {
                let text = visible_traffic(&list)
                    .iter()
                    .map(|e| {
                        format!(
                            "{}  {}  {}{}  {}  {}",
                            e.time,
                            e.client,
                            e.method,
                            e.target
                                .as_ref()
                                .map(|t| format!(" {t}"))
                                .unwrap_or_default(),
                            match &e.outcome {
                                TrafficOutcome::Ok => "OK".to_string(),
                                TrafficOutcome::Notification => "notified".into(),
                                TrafficOutcome::ToolError(m) => format!("tool error: {m}"),
                                TrafficOutcome::Error(m) => format!("error: {m}"),
                            },
                            e.duration_ms.map(format_ms).unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                window.clipboard().set_text(&text);
                overlay.add_toast(adw::Toast::new("Copied visible requests"));
            });
        }
        {
            let list = list.clone();
            let ui = Rc::downgrade(self);
            action(&actions, "export", move || {
                let Some(ui) = ui.upgrade() else { return };
                let entries: Vec<Value> = visible_traffic(&list)
                    .iter()
                    .map(|e| {
                        let parse = |s: &Option<String>| {
                            s.as_ref().map(|s| {
                                serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                            })
                        };
                        json!({
                            "time": e.time,
                            "client": e.client,
                            "method": e.method,
                            "target": e.target,
                            "status": match &e.outcome {
                                TrafficOutcome::Ok => "ok",
                                TrafficOutcome::Notification => "notification",
                                TrafficOutcome::ToolError(_) => "tool-error",
                                TrafficOutcome::Error(_) => "error",
                            },
                            "error": e.failure(),
                            "durationMs": e.duration_ms,
                            "params": parse(&e.params),
                            "result": parse(&e.response),
                        })
                    })
                    .collect();
                let text = serde_json::to_string_pretty(&entries).unwrap_or_default();
                save_text(&ui, "Export Traffic", "marshal-traffic.json", text);
            });
        }
        {
            let manager = self.manager.clone();
            let id = id.clone();
            action(&actions, "clear", move || manager.clear_traffic(&id));
        }
        toolbar.insert_action_group("traffic", Some(&actions));

        {
            let state = state.clone();
            let list = list.clone();
            search.connect_search_changed(move |s| {
                state.borrow_mut().needle = s.text().to_lowercase();
                list.refilter();
            });
        }
        {
            let state = state.clone();
            let list = list.clone();
            let choices = clients.clone();
            clients.dropdown.connect_selected_notify(move |_| {
                state.borrow_mut().client = choices.selected();
                list.refilter();
            });
        }
        {
            let state = state.clone();
            let list = list.clone();
            modes.connect_active_name_notify(move |g| {
                state.borrow_mut().mode = g.active_name().unwrap_or_default().to_string();
                list.refilter();
            });
        }

        let manager = self.manager.clone();
        let ui_weak = Rc::downgrade(self);
        let summary_rows: Rc<RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>> = Rc::default();
        // The summary is rebuilt from scratch, so only do it while it is on
        // screen, and at most once a second while traffic keeps arriving.
        let latest: Rc<RefCell<VecDeque<Arc<TrafficEntry>>>> = Rc::default();
        let summary_stale = Rc::new(Cell::new(true));
        let refresh_summary = {
            let ui = Rc::downgrade(self);
            let views = views.downgrade();
            let latest = latest.clone();
            let stale = summary_stale.clone();
            let last_fill: Cell<Option<std::time::Instant>> = Cell::default();
            let switcher = switcher.clone();
            Rc::new(move |force: bool| {
                let (Some(ui), Some(views)) = (ui.upgrade(), views.upgrade()) else {
                    return;
                };
                if !stale.get() || views.visible_child_name().as_deref() != Some("summary") {
                    return;
                }
                if !force
                    && last_fill
                        .get()
                        .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1))
                {
                    return;
                }
                stale.set(false);
                last_fill.set(Some(std::time::Instant::now()));
                ui.fill_summary(
                    &latest.borrow(),
                    &summary_scroll_holder,
                    &methods_group,
                    &errors_group,
                    &clients_group,
                    &summary_rows,
                    &search,
                    &switcher,
                );
            })
        };
        {
            let refresh = refresh_summary.clone();
            views.connect_visible_child_name_notify(move |_| refresh(true));
        }
        let mut last_seq = 0u64;
        let mut revision = None;
        let mut first = true;
        let mut tick = move || {
            if ui_weak.upgrade().is_none() || (!first && list.detached()) {
                return glib::ControlFlow::Break;
            }
            if !first && list.hidden() {
                return glib::ControlFlow::Continue;
            }
            let show = |list: &StreamList| {
                requests_stack.set_visible_child_name(if list.filtered.n_items() == 0 {
                    "no-results"
                } else {
                    "lines"
                });
            };
            let Some((rev, traffic)) = manager.stream(&id, revision, |s| &s.traffic) else {
                refresh_summary(false);
                show(&list);
                return glib::ControlFlow::Continue;
            };
            revision = Some(rev);
            let store = &list.store;
            let following = list.following();
            let mut changed = false;
            let front = traffic.front().map_or(u64::MAX, |e| e.seq);
            let mut drop = 0;
            while drop < store.n_items() {
                let obj = store.item(drop).and_downcast::<BoxedAnyObject>().unwrap();
                if obj.borrow::<Arc<TrafficEntry>>().seq >= front {
                    break;
                }
                drop += 1;
            }
            if drop > 0 {
                store.splice(0, drop, &[] as &[glib::Object]);
                changed = true;
            }
            if traffic.is_empty() {
                last_seq = 0;
            }
            let fresh: Vec<BoxedAnyObject> = traffic
                .iter()
                .filter(|e| e.seq > last_seq)
                .map(|e| {
                    clients.ensure(&e.client);
                    BoxedAnyObject::new(e.clone())
                })
                .collect();
            if let Some(e) = traffic.back() {
                last_seq = last_seq.max(e.seq);
            }
            if !fresh.is_empty() {
                store.extend_from_slice(&fresh);
                changed = true;
            }
            if changed || first {
                let failed = traffic.iter().filter(|e| e.failed()).count();
                let slow = traffic
                    .iter()
                    .filter(|e| e.duration_ms.is_some_and(|d| d >= SLOW_MS))
                    .count();
                toggles[0].set_label(Some(&format!("All  {}", group_thousands(traffic.len()))));
                toggles[1].set_label(Some(&format!("Failed  {}", group_thousands(failed))));
                toggles[2].set_label(Some(&format!("Slow  {}", group_thousands(slow))));
                *latest.borrow_mut() = traffic;
                summary_stale.set(true);
                if following || first {
                    list.follow();
                }
                first = false;
            }
            refresh_summary(false);
            outer.set_visible_child_name(if latest.borrow().is_empty() {
                "empty"
            } else {
                "content"
            });
            show(&list);
            glib::ControlFlow::Continue
        };
        tick();
        glib::timeout_add_local(std::time::Duration::from_millis(300), tick);
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_summary(
        self: &Rc<Self>,
        traffic: &VecDeque<Arc<TrafficEntry>>,
        cards: &gtk::Box,
        methods_group: &adw::PreferencesGroup,
        errors_group: &adw::PreferencesGroup,
        clients_group: &adw::PreferencesGroup,
        rows: &RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>,
        search: &gtk::SearchEntry,
        switcher: &adw::ToggleGroup,
    ) {
        for (group, row) in rows.borrow_mut().drain(..) {
            group.remove(&row);
        }
        while let Some(child) = cards.first_child() {
            cards.remove(&child);
        }
        let requests: Vec<&Arc<TrafficEntry>> = traffic
            .iter()
            .filter(|e| e.outcome != TrafficOutcome::Notification)
            .collect();
        let mut all = Bucket::default();
        let mut methods: BTreeMap<(String, String), Bucket> = BTreeMap::new();
        let mut by_client: BTreeMap<String, Bucket> = BTreeMap::new();
        for e in &requests {
            all.add(e);
            methods
                .entry((e.target.clone().unwrap_or_default(), e.method.clone()))
                .or_default()
                .add(e);
            by_client.entry(e.client.clone()).or_default().add(e);
        }
        // Four across when there is room, two by two on narrow pages.
        let tiles = gtk::FlowBox::builder()
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(4)
            .column_spacing(8)
            .row_spacing(8)
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["stat-tiles"])
            .build();
        let rate = if all.calls == 0 {
            0.0
        } else {
            all.failed as f64 * 100.0 / all.calls as f64
        };
        let latency = all.latency();
        tiles.append(&stat_card(&group_thousands(all.calls), "Requests", false));
        tiles.append(&stat_card(
            &if all.failed == 0 {
                "0".into()
            } else {
                format!("{} · {rate:.0}%", all.failed)
            },
            "Failed",
            all.failed > 0,
        ));
        tiles.append(&stat_card(
            &latency.map(|l| format_ms(l.0)).unwrap_or("–".into()),
            "Median Latency",
            false,
        ));
        tiles.append(&stat_card(
            &latency.map(|l| format_ms(l.1)).unwrap_or("–".into()),
            "95th Percentile",
            latency.is_some_and(|l| l.1 >= SLOW_MS),
        ));
        let mut index = 0;
        while let Some(child) = tiles.child_at_index(index) {
            child.set_focusable(false);
            index += 1;
        }
        cards.append(&tiles);
        let span = traffic.front().zip(traffic.back()).map(|(a, b)| {
            let a = a.time.split('.').next().unwrap_or_default();
            let b = b.time.split('.').next().unwrap_or_default();
            format!("Covers the last {} messages, {a} – {b}", traffic.len())
        });
        if let Some(span) = span {
            cards.append(
                &gtk::Label::builder()
                    .label(span)
                    .margin_top(8)
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
        }

        let mut ranked: Vec<_> = methods.into_iter().collect();
        ranked.sort_by(|a, b| b.1.calls.cmp(&a.1.calls).then(a.0.cmp(&b.0)));
        for ((target, method), mut bucket) in ranked {
            let title = if target.is_empty() {
                method.clone()
            } else {
                target.clone()
            };
            let mut subtitle = format!(
                "{}{} call{}",
                if target.is_empty() {
                    String::new()
                } else {
                    format!("{method} · ")
                },
                bucket.calls,
                if bucket.calls == 1 { "" } else { "s" }
            );
            if bucket.failed > 0 {
                subtitle.push_str(&format!(" · {} failed", bucket.failed));
            }
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&title))
                .subtitle(glib::markup_escape_text(&subtitle))
                .activatable(true)
                .build();
            if bucket.failed > 0 {
                row.add_prefix(
                    &gtk::Image::builder()
                        .icon_name("dialog-warning-symbolic")
                        .tooltip_text("Has failures")
                        .css_classes(["traffic-status", "tool-error"])
                        .build(),
                );
            }
            if let Some((median, p95)) = bucket.latency() {
                let latency = label(&["dim-label", "numeric", "caption"]);
                latency.set_label(&if median == p95 {
                    format_ms(median)
                } else {
                    format!("{} · p95 {}", format_ms(median), format_ms(p95))
                });
                latency.set_valign(gtk::Align::Center);
                row.add_suffix(&latency);
            }
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let search = search.clone();
            let switcher = switcher.clone();
            row.connect_activated(move |_| {
                search.set_text(&title);
                switcher.set_active_name(Some("requests"));
            });
            methods_group.add(&row);
            rows.borrow_mut()
                .push((methods_group.clone(), row.upcast()));
        }
        methods_group.set_visible(!requests.is_empty());

        let failures: Vec<_> = traffic
            .iter()
            .rev()
            .filter(|e| e.failed())
            .take(5)
            .collect();
        errors_group.set_visible(!failures.is_empty());
        for e in failures {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(
                    e.target.as_deref().unwrap_or(&e.method),
                ))
                .subtitle(glib::markup_escape_text(&format!(
                    "{} · {}",
                    e.time.split('.').next().unwrap_or_default(),
                    e.failure().unwrap_or_default()
                )))
                .subtitle_lines(2)
                .activatable(true)
                .build();
            let (icon, class, _) = status_icon(e);
            row.add_prefix(
                &gtk::Image::builder()
                    .icon_name(icon)
                    .css_classes(["traffic-status", class])
                    .build(),
            );
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let ui = Rc::downgrade(self);
            let entry = e.clone();
            row.connect_activated(move |_| {
                if let Some(ui) = ui.upgrade() {
                    ui.traffic_detail(&entry);
                }
            });
            errors_group.add(&row);
            rows.borrow_mut().push((errors_group.clone(), row.upcast()));
        }

        let mut clients: Vec<_> = by_client.into_iter().collect();
        clients.sort_by_key(|c| std::cmp::Reverse(c.1.calls));
        clients_group.set_visible(!clients.is_empty());
        for (client, mut bucket) in clients {
            let mut subtitle = format!(
                "{} request{}",
                bucket.calls,
                if bucket.calls == 1 { "" } else { "s" }
            );
            if bucket.failed > 0 {
                subtitle.push_str(&format!(" · {} failed", bucket.failed));
            }
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&client))
                .subtitle(subtitle)
                .build();
            if let Some((median, _)) = bucket.latency() {
                let latency = label(&["dim-label", "numeric", "caption"]);
                latency.set_label(&format!("median {}", format_ms(median)));
                latency.set_valign(gtk::Align::Center);
                row.add_suffix(&latency);
            }
            clients_group.add(&row);
            rows.borrow_mut()
                .push((clients_group.clone(), row.upcast()));
        }
    }

    pub(super) fn traffic_detail(&self, e: &TrafficEntry) {
        ensure_css();
        let page = adw::PreferencesPage::new();
        let overview = adw::PreferencesGroup::new();
        let property = |title: &str, value: &str| {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(glib::markup_escape_text(value))
                .subtitle_selectable(true)
                .build();
            row.add_css_class("property");
            overview.add(&row);
            row
        };
        let (icon, class, status) = status_icon(e);
        let status_row = property(
            "Status",
            &match e.failure() {
                Some(f) => format!("{status}: {f}"),
                None => status.to_owned(),
            },
        );
        status_row.add_prefix(
            &gtk::Image::builder()
                .icon_name(icon)
                .valign(gtk::Align::Center)
                .css_classes(["traffic-status", class])
                .build(),
        );
        property("Method", &e.method);
        if let Some(target) = &e.target {
            property(
                if e.method.starts_with("resources/") {
                    "Resource"
                } else {
                    "Name"
                },
                target,
            );
        }
        property("Client", &e.client);
        property("Time", &e.time);
        if let Some(ms) = e.duration_ms {
            property("Duration", &format_ms(ms));
        }
        page.add(&overview);
        let json_group = |title: &str, text: &str| {
            let group = adw::PreferencesGroup::builder().title(title).build();
            let copy = gtk::Button::builder()
                .icon_name("edit-copy-symbolic")
                .tooltip_text(format!("Copy {title}"))
                .css_classes(["flat"])
                .build();
            let owned = text.to_owned();
            let overlay = self.overlay.clone();
            copy.connect_clicked(move |button| {
                button.clipboard().set_text(&owned);
                overlay.add_toast(adw::Toast::new("Copied"));
            });
            group.set_header_suffix(Some(&copy));
            let view = gtk::TextView::builder()
                .editable(false)
                .monospace(true)
                .wrap_mode(gtk::WrapMode::WordChar)
                .css_classes(["json-view"])
                .build();
            view.buffer().set_text(text);
            let frame = gtk::Frame::builder()
                .child(&view)
                .css_classes(["card"])
                .build();
            group.add(&frame);
            group
        };
        if let Some(params) = &e.params {
            page.add(&json_group("Request", params));
        }
        if let Some(response) = &e.response {
            page.add(&json_group("Response", response));
        }
        self.push(e.target.as_deref().unwrap_or(&e.method), &page);
    }
}

fn visible_traffic(list: &StreamList) -> Vec<Arc<TrafficEntry>> {
    list.visible()
        .into_iter()
        .map(|o| o.borrow::<Arc<TrafficEntry>>().clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_escapes_and_marks_matches() {
        assert_eq!(highlight("a<b>", ""), "a&lt;b&gt;");
        let marked = highlight("Error: Bad", "bad");
        assert!(marked.starts_with("Error: <span"));
        assert!(marked.ends_with(">Bad</span>"));
    }

    #[test]
    fn levels_are_inferred_from_common_formats() {
        assert_eq!(LogLevel::infer("ERROR something broke"), LogLevel::Error);
        assert_eq!(LogLevel::infer("[warn] slow disk"), LogLevel::Warning);
        assert_eq!(LogLevel::infer("2024-01-01 INFO ready"), LogLevel::Info);
        assert_eq!(LogLevel::infer("terrorists and mirrors"), LogLevel::Info);
        assert_eq!(
            LogLevel::infer("x Traceback (most recent call last):"),
            LogLevel::Error
        );
    }

    #[test]
    fn percentiles_and_thousands() {
        assert_eq!(percentile(&[1, 2, 3, 4, 100], 0.5), 3);
        assert_eq!(percentile(&[1, 2, 3, 4, 100], 0.95), 100);
        assert_eq!(group_thousands(1234567), "1,234,567");
        assert_eq!(format_ms(1500), "1.5 s");
    }
}
