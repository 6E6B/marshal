use crate::{
    backend::Manager,
    config::{Connection, CustomCommand, RemoteConfig, Server},
};
use adw::prelude::*;
use gtk::{gio, glib};
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct CommandRow {
    id: String,
    row: adw::ActionRow,
    run: gtk::Button,
    edit: gtk::Button,
    delete: gtk::Button,
}

pub(crate) struct Ui {
    window: adw::ApplicationWindow,
    manager: Arc<Manager>,
    overlay: adw::ToastOverlay,
    split: adw::NavigationSplitView,
    navigation: adw::NavigationView,
    list: gtk::ListBox,
    list_stack: gtk::Stack,
    content_stack: gtk::Stack,
    selected: RefCell<Option<String>>,
    remote_states: RefCell<std::collections::HashMap<String, String>>,
    overview: adw::NavigationPage,
    status: adw::ActionRow,
    lifecycle: gtk::Button,
    banner: adw::Banner,
    capabilities: Vec<adw::ActionRow>,
    diagnostics: adw::ActionRow,
    commands: adw::PreferencesGroup,
    command_rows: RefCell<Vec<CommandRow>>,
    command_signature: RefCell<String>,
}

pub fn build(app: &adw::Application, manager: Arc<Manager>) -> Rc<Ui> {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Marshal")
        .default_width(1040)
        .default_height(740)
        .width_request(360)
        .height_request(400)
        .hide_on_close(true)
        .build();
    let overlay = adw::ToastOverlay::new();
    let split = adw::NavigationSplitView::builder()
        .min_sidebar_width(230.0)
        .max_sidebar_width(320.0)
        .build();
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        760.0,
        adw::LengthUnit::Sp,
    ));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);
    overlay.set_child(Some(&split));
    window.set_content(Some(&overlay));
    let sidebar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("Marshal", "")));
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Server (Ctrl+N)")
        .action_name("app.add-server")
        .build();
    header.pack_start(&add);
    let menu = gio::Menu::new();
    menu.append(
        Some("Agent Clients & Inventory…"),
        Some("app.agent-clients"),
    );
    menu.append(Some("About Marshal"), Some("app.about"));
    menu.append(Some("Quit"), Some("app.quit"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Main Menu")
            .build(),
    );
    sidebar.add_top_bar(&header);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");
    {
        let manager = manager.clone();
        list.set_sort_func(move |a, b| {
            manager
                .is_active(b.widget_name().as_str())
                .cmp(&manager.is_active(a.widget_name().as_str()))
                .into()
        });
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&list)
        .build();
    let list_stack = gtk::Stack::new();
    list_stack.add_named(&scroll, Some("servers"));
    let empty = adw::StatusPage::builder()
        .icon_name("network-server-symbolic")
        .title("No Servers")
        .description("Add an MCP server to get started.")
        .build();
    let empty_add = gtk::Button::builder()
        .label("Add Server")
        .halign(gtk::Align::Center)
        .action_name("app.add-server")
        .css_classes(["suggested-action", "pill"])
        .build();
    empty.set_child(Some(&empty_add));
    list_stack.add_named(&empty, Some("empty"));
    sidebar.set_content(Some(&list_stack));
    split.set_sidebar(Some(&adw::NavigationPage::new(&sidebar, "Servers")));

    let navigation = adw::NavigationView::new();
    let toolbar = adw::ToolbarView::new();
    let detail_header = adw::HeaderBar::new();
    let detail_menu = gio::Menu::new();
    detail_menu.append(Some("Install to MCP Clients…"), Some("app.install-clients"));
    detail_menu.append(Some("Configuration"), Some("app.configure"));
    detail_menu.append(Some("Restart"), Some("app.restart"));
    detail_menu.append(Some("Refresh Capabilities"), Some("app.refresh"));
    let removal = gio::Menu::new();
    removal.append(Some("Remove Server…"), Some("app.remove"));
    detail_menu.append_section(None, &removal);
    detail_header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Server Actions")
            .menu_model(&detail_menu)
            .build(),
    );
    toolbar.add_top_bar(&detail_header);
    let banner = adw::Banner::builder()
        .button_label("Restart")
        .revealed(false)
        .build();
    toolbar.add_top_bar(&banner);
    let page = adw::PreferencesPage::new();
    let state_group = adw::PreferencesGroup::new();
    let status = adw::ActionRow::builder().title("Stopped").build();
    let lifecycle = gtk::Button::builder()
        .label("Start")
        .valign(gtk::Align::Center)
        .build();
    status.add_suffix(&lifecycle);
    state_group.add(&status);
    page.add(&state_group);
    let inspect = adw::PreferencesGroup::builder().title("Inspect").build();
    let capabilities: Vec<_> = ["Tools", "Resources", "Prompts"]
        .into_iter()
        .map(|title| {
            let row = link_row(title, "");
            row.set_visible(false);
            inspect.add(&row);
            row
        })
        .collect();
    let logs = link_row("Logs", "");
    inspect.add(&logs);
    page.add(&inspect);
    let manage = adw::PreferencesGroup::builder().title("Manage").build();
    let install_clients = link_row(
        "Install to MCP Clients",
        "Configure for Claude Code, VS Code, Codex…",
    );
    let remote = link_row("Remote Access", "");
    let configuration = link_row("Configuration", "");
    let diagnostics = adw::ActionRow::builder()
        .title("Connection")
        .subtitle_selectable(true)
        .build();
    manage.add(&install_clients);
    manage.add(&remote);
    manage.add(&configuration);
    manage.add(&diagnostics);
    page.add(&manage);
    let commands = adw::PreferencesGroup::builder()
        .title("Custom Commands")
        .build();
    let add_command = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Command")
        .valign(gtk::Align::Center)
        .build();
    commands.set_header_suffix(Some(&add_command));
    page.add(&commands);
    toolbar.set_content(Some(&page));
    let overview = adw::NavigationPage::new(&toolbar, "Server");
    navigation.add(&overview);
    let placeholder_toolbar = adw::ToolbarView::new();
    placeholder_toolbar.add_top_bar(&adw::HeaderBar::new());
    placeholder_toolbar.set_content(Some(
        &adw::StatusPage::builder()
            .icon_name("network-server-symbolic")
            .title("Select a Server")
            .build(),
    ));
    let content_stack = gtk::Stack::new();
    content_stack.add_named(&placeholder_toolbar, Some("empty"));
    content_stack.add_named(&navigation, Some("detail"));
    split.set_content(Some(&adw::NavigationPage::new(&content_stack, "Server")));
    let ui = Rc::new(Ui {
        window,
        manager,
        overlay,
        split,
        navigation,
        list,
        list_stack,
        content_stack: content_stack.clone(),
        selected: RefCell::new(None),
        remote_states: RefCell::new(Default::default()),
        overview,
        status,
        lifecycle,
        banner,
        capabilities,
        diagnostics,
        commands,
        command_rows: RefCell::new(vec![]),
        command_signature: RefCell::new(String::new()),
    });
    connect(&add_command, &ui, |ui| {
        if let Some(id) = ui.id() {
            ui.command_editor(id, None);
        }
    });
    let weak = Rc::downgrade(&ui);
    ui.list.connect_row_selected(move |_, row| {
        if let (Some(ui), Some(row)) = (weak.upgrade(), row) {
            *ui.selected.borrow_mut() = Some(row.widget_name().to_string());
            ui.navigation.pop_to_page(&ui.overview);
            content_stack.set_visible_child_name("detail");
            ui.split.set_show_content(true);
            ui.update();
        }
    });
    connect(&ui.lifecycle, &ui, |ui| {
        let Some(id) = ui.id() else {
            return;
        };
        let stop = ui.manager.snapshot(&id).active();
        ui.operation(move |m| async move {
            if stop {
                m.stop(&id).await
            } else {
                m.start(&id).await
            }
        });
    });
    let weak = Rc::downgrade(&ui);
    ui.banner.connect_button_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.restart();
        }
    });
    for (index, row) in ui.capabilities.iter().enumerate() {
        let weak = Rc::downgrade(&ui);
        row.connect_activated(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.inspection(index);
            }
        });
    }
    let weak = Rc::downgrade(&ui);
    logs.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.logs();
        }
    });
    let weak = Rc::downgrade(&ui);
    remote.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.remote();
        }
    });
    let weak = Rc::downgrade(&ui);
    configuration.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.edit_selected();
        }
    });
    let weak = Rc::downgrade(&ui);
    install_clients.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.install_clients();
        }
    });
    for (name, handler) in [
        ("add-server", (|u: &Rc<Ui>| u.editor(None)) as fn(&Rc<Ui>)),
        ("install-clients", |u| u.install_clients()),
        ("agent-clients", |u| u.agent_clients()),
        ("configure", |u| u.edit_selected()),
        ("restart", |u| u.restart()),
        ("refresh", |u| {
            if let Some(id) = u.id() {
                u.operation(move |m| async move { m.discover(&id).await });
            }
        }),
        ("remove", |u| u.remove()),
        ("about", |u| u.about()),
        ("quit", |u| u.quit()),
    ] {
        let action = gio::SimpleAction::new(name, None);
        let weak = Rc::downgrade(&ui);
        action.connect_activate(move |_, _| {
            if let Some(ui) = weak.upgrade() {
                handler(&ui);
            }
        });
        app.add_action(&action);
    }
    app.set_accels_for_action("app.add-server", &["<Control>n"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    let close = gio::SimpleAction::new("close", None);
    let weak = ui.window.downgrade();
    close.connect_activate(move |_, _| {
        if let Some(w) = weak.upgrade() {
            w.close();
        }
    });
    ui.window.add_action(&close);
    app.set_accels_for_action("win.close", &["<Control>w"]);
    ui.refresh_list();
    ui.update();
    if let Some(error) = &ui.manager.load_error {
        ui.error(error);
    }
    let weak = Rc::downgrade(&ui);
    glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
        let Some(ui) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        ui.update();
        glib::ControlFlow::Continue
    });
    for server in ui.manager.servers().into_iter().filter(|s| s.auto_start) {
        ui.operation(move |m| async move { m.start(&server.id).await });
    }
    ui.window.present();
    // Retain the controller with the application; hiding the window leaves supervision alive.
    let retained = ui.clone();
    app.connect_shutdown(move |_| {
        let _ = &retained;
    });
    ui
}

fn remote_active(state: &str) -> bool {
    ["Starting", "Connected", "Connecting", "Waiting for Tunnel"].contains(&state)
}
fn sidebar_status_dot(snapshot: &crate::backend::Snapshot) -> gtk::Image {
    let dot = gtk::Image::from_icon_name("media-record-symbolic");
    dot.set_widget_name("status-dot");
    dot.set_pixel_size(10);
    dot.set_valign(gtk::Align::Center);
    apply_sidebar_status(&dot, snapshot);
    dot
}
fn apply_sidebar_status(dot: &gtk::Image, snapshot: &crate::backend::Snapshot) {
    for class in ["success", "warning", "error", "dim-label"] {
        dot.remove_css_class(class);
    }
    dot.add_css_class(match snapshot.state.as_str() {
        "Running" => "success",
        "Starting" | "Restarting" => "warning",
        "Failed" => "error",
        _ => "dim-label",
    });
    dot.set_tooltip_text(Some(snapshot.state.as_str()));
}
fn named_descendant(widget: &impl IsA<gtk::Widget>, name: &str) -> Option<gtk::Widget> {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.widget_name() == name {
            return Some(current);
        }
        if let Some(found) = named_descendant(&current, name) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}
fn sidebar_running_out_of_order(list: &gtk::ListBox, manager: &Manager) -> bool {
    let mut seen_inactive = false;
    let mut child = list.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        if manager.is_active(widget.widget_name().as_str()) {
            if seen_inactive {
                return true;
            }
        } else {
            seen_inactive = true;
        }
    }
    false
}

fn link_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().activatable(true).build();
    row.set_use_markup(false);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}
fn connect(button: &gtk::Button, ui: &Rc<Ui>, f: impl Fn(&Rc<Ui>) + 'static) {
    let weak = Rc::downgrade(ui);
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            f(&ui);
        }
    });
}
fn text_view(text: &str, editable: bool) -> gtk::TextView {
    let view = gtk::TextView::builder()
        .editable(editable)
        .cursor_visible(editable)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(12)
        .right_margin(12)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    view.buffer().set_text(text);
    view
}
fn buffer_text(view: &gtk::TextView) -> String {
    let b = view.buffer();
    b.text(&b.start_iter(), &b.end_iter(), false).into()
}
fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

impl Ui {
    fn command_editor(self: &Rc<Self>, server_id: String, existing: Option<CustomCommand>) {
        let dialog = adw::PreferencesDialog::builder()
            .title(if existing.is_some() {
                "Edit Command"
            } else {
                "Add Command"
            })
            .content_width(560)
            .content_height(430)
            .build();
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .description("Runs with /bin/sh using this server’s environment and working directory. Shell syntax is supported. Commands run until they exit or you stop them.").build();
        let name = adw::EntryRow::builder().title("Button Name").build();
        let command = text_view(
            existing.as_ref().map(|c| c.command.as_str()).unwrap_or(""),
            true,
        );
        command.set_height_request(140);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&command)
            .min_content_height(140)
            .build();
        if let Some(custom) = &existing {
            name.set_text(&custom.name);
        }
        group.add(&name);
        let code = adw::PreferencesGroup::builder()
            .title("Shell Command")
            .build();
        code.add(&scroll);
        let save = gtk::Button::builder()
            .label("Save")
            .halign(gtk::Align::End)
            .css_classes(["suggested-action"])
            .build();
        code.add(&save);
        page.add(&group);
        page.add(&code);
        dialog.add(&page);
        let weak = Rc::downgrade(self);
        let dialog_save = dialog.clone();
        save.connect_clicked(move |button| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let custom = CustomCommand {
                id: existing
                    .as_ref()
                    .map(|c| c.id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                name: name.text().trim().to_owned(),
                command: buffer_text(&command),
            };
            if custom.name.is_empty() || custom.command.trim().is_empty() {
                dialog_save.add_toast(adw::Toast::new("Enter a button name and command"));
                return;
            }
            let id = server_id.clone();
            let manager = ui.manager.clone();
            button.set_sensitive(false);
            dialog_save.set_can_close(false);
            let task = ui.manager.runtime.spawn(async move {
                let mut server = manager.server(&id)?;
                if let Some(old) = server.commands.iter_mut().find(|c| c.id == custom.id) {
                    *old = custom;
                } else {
                    server.commands.push(custom);
                }
                manager.save(server, vec![]).await
            });
            let dialog = dialog_save.clone();
            let button = button.clone();
            glib::spawn_future_local(async move {
                let result = task.await;
                dialog.set_can_close(true);
                match result {
                    Ok(Ok(())) => {
                        dialog.close();
                        ui.update();
                    }
                    Ok(Err(e)) => dialog.add_toast(adw::Toast::new(&format!("{e:#}"))),
                    Err(e) => dialog.add_toast(adw::Toast::new(&e.to_string())),
                }
                button.set_sensitive(true);
            });
        });
        dialog.present(Some(&self.window));
    }

    fn editor(self: &Rc<Self>, existing: Option<Server>) {
        let adding = existing.is_none();
        let server = existing.unwrap_or_else(|| Server {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            connection: Connection::Stdio {
                executable: String::new(),
                arguments: vec![],
                directory: String::new(),
            },
            environment: Default::default(),
            secrets: vec![],
            auto_start: false,
            restart_on_failure: false,
            remote: RemoteConfig::default(),
            commands: vec![],
            bridge_port: None,
        });
        let draft = Rc::new(RefCell::new(server.clone()));
        let secret_changes = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
        let dialog = adw::PreferencesDialog::builder()
            .title(if adding {
                "Add Server"
            } else {
                "Configuration"
            })
            .content_width(560)
            .content_height(650)
            .build();
        let page = adw::PreferencesPage::new();
        let connection = adw::PreferencesGroup::new();
        let name = adw::EntryRow::builder()
            .title("Name")
            .text(&server.name)
            .build();
        let kind = adw::ComboRow::builder()
            .title("Connection")
            .model(&gtk::StringList::new(&["Launch Command", "HTTP Endpoint"]))
            .selected(u32::from(matches!(
                server.connection,
                Connection::Http { .. }
            )))
            .build();
        let command = adw::EntryRow::builder()
            .title(if kind.selected() == 0 {
                "Command"
            } else {
                "Endpoint URL"
            })
            .text(server.command_text())
            .build();
        connection.add(&name);
        connection.add(&kind);
        connection.add(&command);
        page.add(&connection);
        let advanced_group = adw::PreferencesGroup::new();
        let advanced = adw::ExpanderRow::builder()
            .title("Advanced")
            .expanded(!adding)
            .build();
        let directory_path = Rc::new(RefCell::new(match &server.connection {
            Connection::Stdio { directory, .. } => directory.clone(),
            _ => String::new(),
        }));
        let directory = adw::ActionRow::builder()
            .title("Working Directory")
            .activatable(true)
            .subtitle_selectable(true)
            .build();
        directory.set_use_markup(false);
        let initial_directory = directory_path.borrow().clone();
        directory.set_subtitle(if initial_directory.is_empty() {
            "Default working directory"
        } else {
            &initial_directory
        });
        let browse = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Choose Working Directory")
            .valign(gtk::Align::Center)
            .build();
        let clear = gtk::Button::builder()
            .icon_name("edit-clear-symbolic")
            .tooltip_text("Use Default Working Directory")
            .valign(gtk::Align::Center)
            .sensitive(!directory_path.borrow().is_empty())
            .build();
        directory.add_suffix(&clear);
        directory.add_suffix(&browse);
        directory.set_activatable_widget(Some(&browse));
        let path = directory_path.clone();
        let row = directory.clone();
        clear.connect_clicked(move |button| {
            path.borrow_mut().clear();
            row.set_subtitle("Default working directory");
            button.set_sensitive(false);
        });
        let path = directory_path.clone();
        let row = directory.clone();
        let parent = dialog.downgrade();
        let window = self.window.clone();
        browse.connect_clicked(move |_| {
            let picker = gtk::FileDialog::builder()
                .title("Choose Working Directory")
                .modal(true)
                .build();
            if !path.borrow().is_empty() {
                picker.set_initial_folder(Some(&gio::File::for_path(path.borrow().as_str())));
            }
            let path = path.clone();
            let row = row.clone();
            let clear = clear.clone();
            let parent = parent.clone();
            picker.select_folder(Some(&window), None::<&gio::Cancellable>, move |result| {
                let Some(parent) = parent.upgrade() else {
                    return;
                };
                match result {
                    Ok(folder) => {
                        if let Some(selected) =
                            folder.path().and_then(|p| p.to_str().map(str::to_owned))
                        {
                            row.set_subtitle(&selected);
                            *path.borrow_mut() = selected;
                            clear.set_sensitive(true);
                        } else {
                            parent.add_toast(adw::Toast::new(
                                "Choose a local folder with a valid UTF-8 path",
                            ));
                        }
                    }
                    Err(error)
                        if error.matches(gtk::DialogError::Dismissed)
                            || error.matches(gtk::DialogError::Cancelled) => {}
                    Err(error) => parent.add_toast(adw::Toast::new(&format!(
                        "Could not choose folder: {error}"
                    ))),
                }
            });
        });
        let startup = adw::SwitchRow::builder()
            .title("Start Automatically")
            .subtitle("When Marshal starts")
            .active(server.auto_start)
            .build();
        let restart = adw::SwitchRow::builder()
            .title("Restart on Failure")
            .subtitle("Retry up to five times with increasing delays")
            .active(server.restart_on_failure)
            .build();
        let environment = link_row("Environment & Secrets", "");
        advanced.add_row(&directory);
        advanced.add_row(&startup);
        advanced.add_row(&restart);
        advanced.add_row(&environment);
        advanced_group.add(&advanced);
        page.add(&advanced_group);
        directory.set_visible(kind.selected() == 0);
        restart.set_visible(kind.selected() == 0);
        environment.set_visible(kind.selected() == 0);
        let c = command.clone();
        let d = directory.clone();
        let r = restart.clone();
        let e = environment.clone();
        kind.connect_selected_notify(move |k| {
            let stdio = k.selected() == 0;
            c.set_title(if stdio { "Command" } else { "Endpoint URL" });
            d.set_visible(stdio);
            r.set_visible(stdio);
            e.set_visible(stdio);
        });
        let env_draft = draft.clone();
        let env_changes = secret_changes.clone();
        let parent = dialog.clone();
        let manager = self.manager.clone();
        environment.connect_activated(move |_| {
            environment_editor(
                &parent,
                env_draft.clone(),
                env_changes.clone(),
                manager.clone(),
            )
        });
        let action_group = adw::PreferencesGroup::new();
        let save = gtk::Button::builder()
            .label(if adding { "Add and Start" } else { "Save" })
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        action_group.add(&save);
        page.add(&action_group);
        dialog.add(&page);
        let ui = self.clone();
        let dialog_save = dialog.clone();
        let command_focus = command.clone();
        save.connect_clicked(move |button| {
            let mut server = draft.borrow().clone();
            server.name = name.text().trim().into();
            server.auto_start = startup.is_active();
            server.restart_on_failure = restart.is_active() && kind.selected() == 0;
            let connection = if kind.selected() == 0 {
                Server::command(command.text().as_str()).map(|mut c| {
                    if let Connection::Stdio { directory: dir, .. } = &mut c {
                        *dir = directory_path.borrow().clone();
                    }
                    c
                })
            } else {
                Ok(Connection::Http {
                    url: command.text().trim().into(),
                })
            };
            match connection {
                Ok(c) => server.connection = c,
                Err(e) => {
                    dialog_save.add_toast(adw::Toast::new(&e.to_string()));
                    command.grab_focus();
                    return;
                }
            }
            if server.name.is_empty() && adding {
                server.name = match &server.connection {
                    Connection::Stdio { executable, .. } => std::path::Path::new(executable)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    Connection::Http { url } => reqwest::Url::parse(url)
                        .ok()
                        .and_then(|u| u.host_str().map(str::to_owned))
                        .unwrap_or_default(),
                };
            }
            if let Err(e) = server.validate() {
                dialog_save.add_toast(adw::Toast::new(&e.to_string()));
                return;
            }
            button.set_sensitive(false);
            dialog_save.set_can_close(false);
            let manager = ui.manager.clone();
            let changes = secret_changes.borrow().clone();
            let id = server.id.clone();
            let task = manager.runtime.spawn({
                let m = manager.clone();
                async move {
                    m.save(server, changes).await?;
                    if adding {
                        m.start(&id).await?;
                    }
                    Ok::<_, anyhow::Error>(id)
                }
            });
            let ui = ui.clone();
            let dialog = dialog_save.clone();
            let button = button.clone();
            let changes = secret_changes.clone();
            glib::spawn_future_local(async move {
                let response = task.await;
                button.set_sensitive(true);
                dialog.set_can_close(true);
                match response {
                    Ok(Ok(id)) => {
                        changes.borrow_mut().clear();
                        *ui.selected.borrow_mut() = Some(id);
                        ui.refresh_list();
                        ui.update();
                        dialog.close();
                        ui.overlay.add_toast(adw::Toast::new(if adding {
                            "Server added"
                        } else {
                            "Configuration saved"
                        }));
                    }
                    Ok(Err(e)) => dialog.add_toast(adw::Toast::new(&format!("{e:#}"))),
                    Err(e) => dialog.add_toast(adw::Toast::new(&e.to_string())),
                }
            });
        });
        dialog.present(Some(&self.window));
        command_focus.grab_focus();
    }

    fn inspection(self: &Rc<Self>, index: usize) {
        let Some(id) = self.id() else {
            return;
        };
        let snapshot = self.manager.snapshot(&id);
        let items = match index {
            0 => snapshot.tools,
            1 => snapshot.resources,
            _ => snapshot.prompts,
        };
        let title = ["Tools", "Resources", "Prompts"][index];
        if items.is_empty() && (index != 1 || snapshot.templates.is_empty()) {
            self.push(
                title,
                &adw::StatusPage::builder()
                    .icon_name("system-search-symbolic")
                    .title(format!("No {title}"))
                    .description("The server returned an empty list.")
                    .build(),
            );
            return;
        }
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::new();
        for item in items {
            let row = link_row(string(&item, "name"), string(&item, "description"));
            row.set_subtitle_lines(2);
            let weak = Rc::downgrade(self);
            let id = id.clone();
            row.connect_activated(move |_| {
                if let Some(ui) = weak.upgrade() {
                    match index {
                        0 => ui.tool(&id, &item),
                        1 => ui.resource(&id, &item),
                        _ => ui.prompt(&id, &item),
                    }
                }
            });
            group.add(&row);
        }
        page.add(&group);
        if index == 1 && !snapshot.templates.is_empty() {
            let group = adw::PreferencesGroup::builder()
                .title("Resource Templates")
                .build();
            for template in snapshot.templates {
                let row = link_row(string(&template, "name"), string(&template, "uriTemplate"));
                let weak = Rc::downgrade(self);
                let id = id.clone();
                row.connect_activated(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        ui.resource(&id, &template);
                    }
                });
                group.add(&row);
            }
            page.add(&group);
        }
        self.push(title, &page);
    }
    fn tool(self: &Rc<Self>, id: &str, tool: &Value) {
        let page = adw::PreferencesPage::new();
        let description = adw::PreferencesGroup::builder()
            .description(string(tool, "description"))
            .build();
        let input_schema = &tool["inputSchema"];
        let fields = schema_fields(input_schema, &description);
        page.add(&description);
        let raw_group = adw::PreferencesGroup::new();
        let raw = adw::ExpanderRow::builder()
            .title("Structured Input and Schema")
            .build();
        let raw_input = text_view("{}", true);
        let raw_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(130)
            .max_content_height(220)
            .child(&raw_input)
            .build();
        raw.add_row(&raw_scroll);
        let use_raw = adw::SwitchRow::builder()
            .title("Use Structured Input")
            .active(
                fields.is_empty()
                    && input_schema["properties"]
                        .as_object()
                        .is_some_and(|p| !p.is_empty()),
            )
            .build();
        raw.add_row(&use_raw);
        let schema_view = text_view(
            &serde_json::to_string_pretty(input_schema).unwrap_or_default(),
            false,
        );
        raw.add_row(
            &gtk::ScrolledWindow::builder()
                .min_content_height(150)
                .max_content_height(250)
                .child(&schema_view)
                .build(),
        );
        raw_group.add(&raw);
        page.add(&raw_group);
        let actions = adw::PreferencesGroup::new();
        let call = gtk::Button::builder()
            .label("Run Tool")
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        actions.add(&call);
        page.add(&actions);
        let result_group = ResultPane::new("Result");
        page.add(&result_group.group);
        let name = string(tool, "name").to_owned();
        let id = id.to_owned();
        let ui = self.clone();
        call.connect_clicked(move |button| {
            let input = if use_raw.is_active() {
                serde_json::from_str::<Value>(&buffer_text(&raw_input))
                    .map_err(|e| anyhow::anyhow!(e))
            } else {
                collect_fields(&fields)
            };
            let input = match input {
                Ok(v) if v.is_object() => v,
                Ok(_) => {
                    ui.error("Tool input must be a JSON object");
                    return;
                }
                Err(e) => {
                    ui.error(&e.to_string());
                    return;
                }
            };
            ui.invoke(
                button,
                &result_group,
                &id,
                "tools/call",
                json!({"name": name, "arguments": input}),
            );
        });
        self.push(string(tool, "name"), &page);
    }
    fn resource(self: &Rc<Self>, id: &str, resource: &Value) {
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .description(string(resource, "description"))
            .build();
        let uri = adw::EntryRow::builder()
            .title("Resource URI")
            .text(if resource.get("uri").is_some() {
                string(resource, "uri")
            } else {
                string(resource, "uriTemplate")
            })
            .build();
        group.add(&uri);
        page.add(&group);
        let action = gtk::Button::builder()
            .label("Read Resource")
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        let actions = adw::PreferencesGroup::new();
        actions.add(&action);
        page.add(&actions);
        let result = ResultPane::new("Contents");
        page.add(&result.group);
        let ui = self.clone();
        let id = id.to_owned();
        action.connect_clicked(move |button| {
            ui.invoke(
                button,
                &result,
                &id,
                "resources/read",
                json!({"uri": uri.text().to_string()}),
            );
        });
        self.push(string(resource, "name"), &page);
    }
    fn prompt(self: &Rc<Self>, id: &str, prompt: &Value) {
        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .description(string(prompt, "description"))
            .build();
        let mut fields = vec![];
        if let Some(arguments) = prompt["arguments"].as_array() {
            for argument in arguments {
                let name = string(argument, "name").to_owned();
                let required = argument["required"].as_bool().unwrap_or(false);
                let row = adw::EntryRow::builder()
                    .title(format!(
                        "{}{}",
                        name,
                        if required { " (Required)" } else { "" }
                    ))
                    .build();
                group.add(&row);
                fields.push((name, required, row));
            }
        }
        page.add(&group);
        let actions = adw::PreferencesGroup::new();
        let action = gtk::Button::builder()
            .label("Get Prompt")
            .halign(gtk::Align::Center)
            .css_classes(["suggested-action", "pill"])
            .build();
        actions.add(&action);
        page.add(&actions);
        let result = ResultPane::new("Messages");
        page.add(&result.group);
        let ui = self.clone();
        let id = id.to_owned();
        let name = string(prompt, "name").to_owned();
        action.connect_clicked(move |button| {
            let mut arguments = serde_json::Map::new();
            for (key, required, row) in &fields {
                let text = row.text().to_string();
                if *required && text.is_empty() {
                    ui.error(&format!("Enter {key}"));
                    row.grab_focus();
                    return;
                }
                if !text.is_empty() {
                    arguments.insert(key.clone(), json!(text));
                }
            }
            ui.invoke(
                button,
                &result,
                &id,
                "prompts/get",
                json!({"name": name, "arguments": arguments}),
            );
        });
        self.push(string(prompt, "name"), &page);
    }
    fn invoke(
        self: &Rc<Self>,
        button: &gtk::Button,
        result: &ResultPane,
        id: &str,
        method: &str,
        params: Value,
    ) {
        button.set_sensitive(false);
        let original_label = button.label().unwrap_or_default();
        button.set_label("Working…");
        let manager = self.manager.clone();
        let id = id.to_owned();
        let method = method.to_owned();
        let task = manager.runtime.spawn({
            let m = manager.clone();
            async move { m.request(&id, &method, params).await }
        });
        let button = button.clone();
        let result = result.clone();
        let ui = self.clone();
        glib::spawn_future_local(async move {
            let response = task.await;
            button.set_sensitive(true);
            button.set_label(&original_label);
            match response {
                Ok(Ok(value)) => {
                    result.group.set_visible(true);
                    render_result(&result, &value);
                }
                Ok(Err(e)) => ui.error(&format!("{e:#}")),
                Err(e) => ui.error(&e.to_string()),
            }
        });
    }
    fn logs(&self) {
        let Some(id) = self.id() else {
            return;
        };
        let view = text_view("", false);
        view.set_vexpand(true);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .build();
        let toolbar = self.push("Logs", &scroll);
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy Logs")
            .build();
        let buffer = view.buffer();
        copy.connect_clicked(move |button| {
            button.clipboard().set_text(&buffer.text(
                &buffer.start_iter(),
                &buffer.end_iter(),
                false,
            ));
        });
        // A bottom toolbar supplies the log action without duplicating navigation headers.
        let bar = gtk::ActionBar::new();
        bar.pack_end(&copy);
        toolbar.add_bottom_bar(&bar);
        let manager = self.manager.clone();
        let weak = view.downgrade();
        let scroll = scroll.downgrade();
        let mut previous = String::new();
        glib::timeout_add_local(std::time::Duration::from_millis(350), move || {
            let Some(view) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let Some(scroll) = scroll.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let text = manager
                .snapshot(&id)
                .logs
                .into_iter()
                .collect::<Vec<_>>()
                .join("\n");
            if text != previous {
                let adjustment = scroll.vadjustment();
                let at_bottom =
                    adjustment.value() >= adjustment.upper() - adjustment.page_size() - 12.0;
                view.buffer().set_text(&text);
                if at_bottom {
                    let buffer = view.buffer();
                    let mark = buffer.create_mark(None, &buffer.end_iter(), false);
                    view.scroll_mark_onscreen(&mark);
                    buffer.delete_mark(&mark);
                }
                previous = text;
            }
            glib::ControlFlow::Continue
        });
    }
    fn remote(self: &Rc<Self>) {
        let Some(server) = self.id().and_then(|id| self.manager.server(&id).ok()) else {
            return;
        };
        let page = adw::PreferencesPage::new();
        let state_group = adw::PreferencesGroup::new();
        let state = adw::ActionRow::builder()
            .title("Remote Access")
            .subtitle("Off")
            .build();
        let toggle = gtk::Button::builder()
            .label("Enable Remote Access")
            .valign(gtk::Align::Center)
            .build();
        state.add_suffix(&toggle);
        state_group.add(&state);
        let endpoint = adw::ActionRow::builder()
            .title("Connection")
            .subtitle_selectable(true)
            .visible(false)
            .build();
        endpoint.set_use_markup(false);
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy Connection")
            .valign(gtk::Align::Center)
            .build();
        endpoint.add_suffix(&copy);
        state_group.add(&endpoint);
        page.add(&state_group);
        let group = adw::PreferencesGroup::builder().title("Tunnel").build();
        let provider = adw::ComboRow::builder()
            .title("Provider")
            .model(&gtk::StringList::new(&[
                "Choose a provider…",
                "OpenAI Secure MCP Tunnel",
                "ngrok",
            ]))
            .selected(match server.remote.provider.as_str() {
                "openai" => 1,
                "ngrok" => 2,
                _ => 0,
            })
            .build();
        let configure = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Edit Provider Settings")
            .valign(gtk::Align::Center)
            .build();
        provider.add_suffix(&configure);
        group.add(&provider);
        let start_with_server = adw::SwitchRow::builder()
            .title("Start with Server")
            .subtitle("Enable remote access whenever this MCP server starts")
            .active(server.remote.start_with_server)
            .build();
        group.add(&start_with_server);
        page.add(&group);
        let syncing = Rc::new(std::cell::Cell::new(false));
        let sync = syncing.clone();
        let ui = self.clone();
        let id = server.id.clone();
        provider.connect_selected_notify(move |row| {
            if sync.get() {
                return;
            }
            let selected = match row.selected() {
                1 => "openai",
                2 => "ngrok",
                _ => {
                    let saved = ui
                        .manager
                        .server(&id)
                        .map(|s| match s.remote.provider.as_str() {
                            "openai" => 1,
                            "ngrok" => 2,
                            _ => 0,
                        })
                        .unwrap_or(0);
                    sync.set(true);
                    row.set_selected(saved);
                    sync.set(false);
                    return;
                }
            };
            ui.tunnel_setup(&id, selected, row, sync.clone());
        });
        let ui = self.clone();
        let id = server.id.clone();
        let row = provider.clone();
        configure.connect_clicked(move |_| {
            let selected = match row.selected() {
                1 => "openai",
                2 => "ngrok",
                _ => return,
            };
            ui.tunnel_setup(&id, selected, &row, syncing.clone());
        });
        let saving_start_option = Rc::new(std::cell::Cell::new(false));
        let saving = saving_start_option.clone();
        let ui = self.clone();
        let id = server.id.clone();
        start_with_server.connect_active_notify(move |row| {
            if saving.replace(true) {
                return;
            }
            let Ok(mut server) = ui.manager.server(&id) else {
                saving.set(false);
                return;
            };
            let previous = server.remote.start_with_server;
            server.remote.start_with_server = row.is_active();
            row.set_sensitive(false);
            let manager = ui.manager.clone();
            let task = manager.runtime.spawn({
                let manager = manager.clone();
                async move { manager.save(server, vec![]).await }
            });
            let row = row.clone();
            let saving = saving.clone();
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                let result = task.await;
                saving.set(false);
                row.set_sensitive(true);
                match result {
                    Ok(Ok(())) => ui
                        .overlay
                        .add_toast(adw::Toast::new("Remote access startup preference saved")),
                    Ok(Err(e)) => {
                        saving.set(true);
                        row.set_active(previous);
                        saving.set(false);
                        ui.error(&format!("{e:#}"));
                    }
                    Err(e) => {
                        saving.set(true);
                        row.set_active(previous);
                        saving.set(false);
                        ui.error(&e.to_string());
                    }
                }
            });
        });
        let busy = Rc::new(std::cell::Cell::new(false));
        let busy_action = busy.clone();
        let id = server.id.clone();
        let ui = self.clone();
        toggle.connect_clicked(move |button| {
            if busy_action.replace(true) {
                return;
            }
            button.set_sensitive(false);
            let stop = remote_active(&ui.manager.snapshot(&id).remote_state);
            let m = ui.manager.clone();
            let id = id.clone();
            let task = m.runtime.spawn({
                let m = m.clone();
                async move {
                    if stop {
                        m.stop_tunnel(&id).await
                    } else {
                        m.start_tunnel(&id).await
                    }
                }
            });
            let busy = busy_action.clone();
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                let result = task.await;
                busy.set(false);
                match result {
                    Ok(Ok(())) => ui.update(),
                    Ok(Err(e)) => ui.error(&format!("{e:#}")),
                    Err(e) => ui.error(&e.to_string()),
                }
            });
        });
        let endpoint_copy = endpoint.clone();
        let overlay = self.overlay.clone();
        copy.connect_clicked(move |button| {
            button
                .clipboard()
                .set_text(endpoint_copy.subtitle().as_deref().unwrap_or(""));
            overlay.add_toast(adw::Toast::new("Connection copied"));
        });
        let weak = state.downgrade();
        let m = self.manager.clone();
        let id = server.id.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            let Some(state) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let s = m.snapshot(&id);
            let active = remote_active(&s.remote_state);
            state.set_use_markup(false);
            toggle.set_label(if active {
                "Disable Remote Access"
            } else {
                "Enable Remote Access"
            });
            let configured = m
                .server(&id)
                .is_ok_and(|server| matches!(server.remote.provider.as_str(), "openai" | "ngrok"));
            toggle.set_sensitive(!busy.get() && (active || (s.state == "Running" && configured)));
            provider.set_sensitive(!busy.get() && !active);
            configure.set_sensitive(configured && !busy.get() && !active);
            start_with_server.set_sensitive(configured && !saving_start_option.get());
            state.set_subtitle(if s.state != "Running" && !active {
                "Start the server to enable remote access"
            } else {
                match s.remote_state.as_str() {
                    "" | "Local Only" => "Off",
                    state => state,
                }
            });
            endpoint.set_visible(s.endpoint.is_some());
            endpoint.set_subtitle(s.endpoint.as_deref().unwrap_or(""));
            glib::ControlFlow::Continue
        });
        self.push("Remote Access", &page);
    }
    fn tunnel_setup(
        self: &Rc<Self>,
        id: &str,
        provider: &str,
        row: &adw::ComboRow,
        syncing: Rc<std::cell::Cell<bool>>,
    ) {
        let Ok(server) = self.manager.server(id) else {
            return;
        };
        let openai = provider == "openai";
        let same_provider = server.remote.provider == provider;
        let dialog = adw::PreferencesDialog::builder()
            .title(if openai {
                "Set Up OpenAI Tunnel"
            } else {
                "Set Up ngrok"
            })
            .content_width(480)
            .content_height(360)
            .build();
        let page = adw::PreferencesPage::new();
        let fields = adw::PreferencesGroup::builder()
            .description(if openai {
                "Enter your tunnel ID and runtime API key from OpenAI Platform."
            } else {
                "Enter your ngrok authtoken. Anyone with the public endpoint can use this server’s tools unless you configure access restrictions in ngrok."
            })
            .build();
        let detail = adw::EntryRow::builder()
            .title(if openai {
                "Tunnel ID"
            } else {
                "Public Domain (Optional)"
            })
            .text(if openai {
                &server.remote.tunnel_id
            } else {
                &server.remote.domain
            })
            .build();
        let credential = adw::PasswordEntryRow::builder()
            .title(if openai { "API Key" } else { "Authtoken" })
            .build();
        fields.add(&detail);
        fields.add(&credential);
        page.add(&fields);
        let actions = adw::PreferencesGroup::new();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::Center);
        let cancel = gtk::Button::with_label("Cancel");
        let save = gtk::Button::builder()
            .label("Save")
            .css_classes(["suggested-action"])
            .build();
        buttons.append(&cancel);
        buttons.append(&save);
        actions.add(&buttons);
        page.add(&actions);
        dialog.add(&page);
        let weak = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = weak.upgrade() {
                dialog.close();
            }
        });
        // Only committed settings are shown after saving or dismissing setup.
        let row = row.downgrade();
        let manager = self.manager.clone();
        let server_id = id.to_owned();
        dialog.connect_closed(move |_| {
            if let Some(row) = row.upgrade() {
                let selected = manager
                    .server(&server_id)
                    .map(|s| match s.remote.provider.as_str() {
                        "openai" => 1,
                        "ngrok" => 2,
                        _ => 0,
                    })
                    .unwrap_or(0);
                syncing.set(true);
                row.set_selected(selected);
                syncing.set(false);
            }
        });
        credential.set_sensitive(false);
        save.set_sensitive(false);
        save.set_label("Loading…");
        let server_id = id.to_owned();
        let credential_name = RemoteConfig::credential_name(provider).unwrap();
        let task = self.manager.runtime.spawn(async move {
            if crate::secrets::SecretStore::contains(&server_id, credential_name).await? {
                crate::secrets::SecretStore::get(&server_id, credential_name)
                    .await
                    .map(Some)
            } else if same_provider
                && crate::secrets::SecretStore::contains(&server_id, "remote-token").await?
            {
                // Read the former shared entry once so saving this dialog migrates it.
                crate::secrets::SecretStore::get(&server_id, "remote-token")
                    .await
                    .map(Some)
            } else {
                Ok(None)
            }
        });
        let weak = dialog.downgrade();
        let weak_credential = credential.downgrade();
        let weak_save = save.downgrade();
        glib::spawn_future_local(async move {
            let result = task.await;
            let (Some(dialog), Some(credential), Some(save)) = (
                weak.upgrade(),
                weak_credential.upgrade(),
                weak_save.upgrade(),
            ) else {
                return;
            };
            if !dialog.is_visible() {
                return;
            }
            credential.set_sensitive(true);
            save.set_sensitive(true);
            save.set_label("Save");
            let error = match result {
                Ok(Ok(value)) => {
                    credential.set_text(value.as_deref().unwrap_or(""));
                    return;
                }
                Ok(Err(e)) => format!("{e:#}"),
                Err(e) => e.to_string(),
            };
            let alert = adw::AlertDialog::builder()
                .heading("Could Not Load Saved Credential")
                .body(format!(
                    "{error}\n\nYou can enter a replacement or cancel and try again."
                ))
                .build();
            alert.add_response("close", "Close");
            alert.present(Some(&dialog));
        });
        let ui = self.clone();
        let id = id.to_owned();
        let provider = provider.to_owned();
        if openai {
            detail.grab_focus();
        } else {
            credential.grab_focus();
        }
        let weak = dialog.downgrade();
        save.connect_clicked(move |button| {
            let Some(dialog) = weak.upgrade() else {
                return;
            };
            let detail_value = detail.text().trim().to_owned();
            if openai && RemoteConfig::validate_openai_tunnel_id(&detail_value).is_err() {
                dialog.add_toast(adw::Toast::new("Enter a valid OpenAI tunnel ID"));
                detail.grab_focus();
                return;
            }
            let value = credential.text().trim().to_owned();
            if value.is_empty() {
                dialog.add_toast(adw::Toast::new(if openai {
                    "Enter an API key"
                } else {
                    "Enter an authtoken"
                }));
                credential.grab_focus();
                return;
            }
            let Ok(mut server) = ui.manager.server(&id) else {
                return;
            };
            server.remote.provider = provider.clone();
            if openai {
                server.remote.tunnel_id = detail_value;
            } else {
                server.remote.domain = detail_value;
            }
            button.set_sensitive(false);
            button.set_label("Saving…");
            fields.set_sensitive(false);
            cancel.set_sensitive(false);
            dialog.set_can_close(false);
            let m = ui.manager.clone();
            let credential_name = RemoteConfig::credential_name(&provider).unwrap().to_owned();
            let task = m.runtime.spawn({
                let m = m.clone();
                async move {
                    let changes = vec![(credential_name, value)];
                    m.save(server, changes).await
                }
            });
            let button = button.clone();
            let fields = fields.clone();
            let cancel = cancel.clone();
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                let result = task.await;
                dialog.set_can_close(true);
                button.set_sensitive(true);
                button.set_label("Save");
                fields.set_sensitive(true);
                cancel.set_sensitive(true);
                let error = match result {
                    Ok(Ok(())) => {
                        dialog.close();
                        ui.overlay.add_toast(adw::Toast::new(if openai {
                            "OpenAI tunnel configured"
                        } else {
                            "ngrok configured"
                        }));
                        return;
                    }
                    Ok(Err(e)) => format!("{e:#}"),
                    Err(e) => e.to_string(),
                };
                let alert = adw::AlertDialog::builder()
                    .heading("Could Not Save Provider Settings")
                    .body(error)
                    .build();
                alert.add_response("close", "Close");
                alert.present(Some(&dialog));
            });
        });
        dialog.present(Some(&self.window));
    }
    fn id(&self) -> Option<String> {
        self.selected.borrow().clone()
    }
    fn error(&self, message: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading("Could Not Complete the Action")
            .body(message)
            .build();
        dialog.add_response("close", "Close");
        dialog.present(Some(&self.window));
    }
    fn operation<F, Fut>(self: &Rc<Self>, task: F)
    where
        F: FnOnce(Arc<Manager>) -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let handle = self.manager.runtime.spawn(task(self.manager.clone()));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = handle.await;
            if let Some(ui) = weak.upgrade() {
                match result {
                    Ok(Ok(())) => ui.update(),
                    Ok(Err(e)) => ui.error(&format!("{e:#}")),
                    Err(e) => ui.error(&e.to_string()),
                }
            }
        });
    }
    fn refresh_list(&self) {
        let selected = self.id();
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let mut servers = self.manager.servers();
        servers.sort_by_key(|s| !self.manager.is_active(&s.id));
        self.list_stack
            .set_visible_child_name(if servers.is_empty() {
                "empty"
            } else {
                "servers"
            });
        for server in servers {
            let snap = self.manager.snapshot(&server.id);
            let row = adw::ActionRow::builder()
                .title(&server.name)
                .subtitle(&snap.state)
                .build();
            row.set_use_markup(false);
            row.set_widget_name(&server.id);
            row.set_selectable(true);
            row.set_activatable(true);
            row.add_suffix(&sidebar_status_dot(&snap));
            self.list.append(&row);
            if selected.as_deref() == Some(&server.id) {
                self.list.select_row(Some(&row));
            }
        }
    }
    fn update(self: &Rc<Self>) {
        for server in self.manager.servers() {
            let state = self.manager.snapshot(&server.id).remote_state;
            let previous = self
                .remote_states
                .borrow_mut()
                .insert(server.id.clone(), state.clone());
            if previous.as_deref() != Some(&state) {
                if let Some(error) = state.strip_prefix("Failed: ") {
                    self.error(&format!(
                        "Remote access for {} failed.\n\n{}",
                        server.name, error
                    ));
                } else if state == "Connected"
                    || (state == "Local Only" && previous.as_deref().is_some_and(remote_active))
                {
                    self.overlay.add_toast(adw::Toast::new(&format!(
                        "{}: {}",
                        server.name,
                        if state == "Connected" {
                            "Remote access connected"
                        } else {
                            "Remote access stopped"
                        }
                    )));
                }
            }
        }

        if let Some(app) = self.window.application() {
            for name in ["configure", "restart", "refresh", "remove"] {
                if let Some(action) = app.lookup_action(name).and_downcast::<gio::SimpleAction>() {
                    action.set_enabled(self.id().is_some());
                }
            }
        }
        let mut row = self.list.first_child();
        while let Some(widget) = row {
            row = widget.next_sibling();
            if let Ok(r) = widget.downcast::<adw::ActionRow>() {
                let snap = self.manager.snapshot(r.widget_name().as_str());
                r.set_subtitle(&snap.state);
                if let Some(dot) = named_descendant(&r, "status-dot")
                    .and_then(|widget| widget.downcast::<gtk::Image>().ok())
                {
                    apply_sidebar_status(&dot, &snap);
                }
            }
        }
        if sidebar_running_out_of_order(&self.list, &self.manager) {
            self.list.invalidate_sort();
        }
        let Some(id) = self.id() else {
            return;
        };
        let Ok(server) = self.manager.server(&id) else {
            return;
        };
        let s = self.manager.snapshot(&id);
        let signature = format!(
            "{}:{}",
            id,
            serde_json::to_string(&server.commands).unwrap()
        );
        if *self.command_signature.borrow() != signature {
            for command in self.command_rows.borrow_mut().drain(..) {
                self.commands.remove(&command.row);
            }
            for command in &server.commands {
                let row = adw::ActionRow::builder().title(&command.name).build();
                row.set_use_markup(false);
                let run = gtk::Button::builder()
                    .label(&command.name)
                    .valign(gtk::Align::Center)
                    .build();
                let edit = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .tooltip_text("Edit Command")
                    .valign(gtk::Align::Center)
                    .build();
                let delete = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .tooltip_text("Remove Command")
                    .valign(gtk::Align::Center)
                    .build();
                let server_id = id.clone();
                let command_id = command.id.clone();
                connect(&run, self, move |ui| {
                    let id = server_id.clone();
                    let cid = command_id.clone();
                    let running = ui
                        .manager
                        .snapshot(&id)
                        .commands
                        .get(&cid)
                        .is_some_and(|state| state == "Running");
                    ui.operation(move |m| async move {
                        if running {
                            m.stop_command(&id, &cid).await
                        } else {
                            m.run_command(&id, &cid).await
                        }
                    });
                });
                let server_id = id.clone();
                let custom = command.clone();
                connect(&edit, self, move |ui| {
                    ui.command_editor(server_id.clone(), Some(custom.clone()))
                });
                let server_id = id.clone();
                let command_id = command.id.clone();
                connect(&delete, self, move |ui| {
                    let id = server_id.clone();
                    let cid = command_id.clone();
                    ui.operation(move |m| async move {
                        let mut server = m.server(&id)?;
                        server.commands.retain(|c| c.id != cid);
                        m.save(server, vec![]).await
                    });
                });
                row.add_suffix(&run);
                row.add_suffix(&edit);
                row.add_suffix(&delete);
                self.commands.add(&row);
                self.command_rows.borrow_mut().push(CommandRow {
                    id: command.id.clone(),
                    row,
                    run,
                    edit,
                    delete,
                });
            }
            *self.command_signature.borrow_mut() = signature;
        }
        for CommandRow {
            id: cid,
            row,
            run,
            edit,
            delete,
        } in self.command_rows.borrow().iter()
        {
            let state = s.commands.get(cid).map(String::as_str).unwrap_or("Ready");
            let running = state == "Running";
            row.set_subtitle(state);
            let name = server
                .commands
                .iter()
                .find(|c| &c.id == cid)
                .map(|c| c.name.as_str())
                .unwrap_or("Run");
            run.set_label(if running { "Stop" } else { name });
            edit.set_sensitive(!running);
            delete.set_sensitive(!running);
        }
        self.overview.set_title(&server.name);
        self.status.set_title(&s.state);
        self.status.set_subtitle(match &server.connection {
            Connection::Stdio { executable, .. } => executable,
            Connection::Http { url } => url,
        });
        self.status.set_use_markup(false);
        self.lifecycle
            .set_label(if matches!(server.connection, Connection::Http { .. }) {
                if s.active() { "Disconnect" } else { "Connect" }
            } else if s.active() {
                "Stop"
            } else {
                "Start"
            });
        self.banner
            .set_revealed(s.needs_restart || s.error.is_some());
        self.banner.set_title(if s.needs_restart {
            "Restart to apply configuration changes"
        } else {
            s.error.as_deref().unwrap_or("")
        });
        self.banner.set_button_label(if s.needs_restart {
            Some("Restart")
        } else {
            None
        });
        for (i, row) in self.capabilities.iter().enumerate() {
            let (key, count) = match i {
                0 => ("tools", s.tools.len()),
                1 => ("resources", s.resources.len() + s.templates.len()),
                _ => ("prompts", s.prompts.len()),
            };
            row.set_visible(
                s.info
                    .as_ref()
                    .is_some_and(|v| v["capabilities"].get(key).is_some()),
            );
            row.set_subtitle(&format!("{count} available"));
            row.set_sensitive(s.state == "Running");
        }
        let mut diagnostics = vec![];
        if let Some(pid) = s.pid {
            diagnostics.push(format!("PID {pid}"));
        }
        if let Some(since) = s.since {
            let elapsed = since.elapsed().as_secs();
            diagnostics.push(format!("Up {}m {}s", elapsed / 60, elapsed % 60));
        }
        if let Some(code) = s.exit_code {
            diagnostics.push(format!("Exit code {code}"));
        }
        if let Some(info) = &s.info {
            diagnostics.push(format!(
                "{} {} · MCP {}",
                string(&info["serverInfo"], "name"),
                string(&info["serverInfo"], "version"),
                string(info, "protocolVersion")
            ));
        }
        if let Some(bridge) = &s.local_endpoint {
            diagnostics.push(format!("Bridge: {bridge}"));
        }
        self.diagnostics.set_visible(!diagnostics.is_empty());
        self.diagnostics.set_use_markup(false);
        self.diagnostics.set_subtitle(&diagnostics.join("\n"));
    }
    fn restart(self: &Rc<Self>) {
        if let Some(id) = self.id() {
            self.operation(move |m| async move { m.restart(&id).await });
        }
    }
    fn edit_selected(self: &Rc<Self>) {
        if let Some(server) = self.id().and_then(|id| self.manager.server(&id).ok()) {
            self.editor(Some(server));
        }
    }
    fn install_clients(self: &Rc<Self>) {
        if let Some(server) = self.id().and_then(|id| self.manager.server(&id).ok()) {
            crate::mcp::ui::install_to_clients_dialog(
                &self.window,
                &self.overlay,
                self.manager.clone(),
                &server,
            );
        }
    }
    fn agent_clients(self: &Rc<Self>) {
        let ui = self.clone();
        crate::mcp::ui::clients_inventory_dialog(
            &self.window,
            &self.overlay,
            self.manager.clone(),
            move || {
                ui.refresh_list();
                ui.update();
            },
        );
    }
    fn push(&self, title: &str, child: &impl IsA<gtk::Widget>) -> adw::ToolbarView {
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(child));
        self.navigation
            .push(&adw::NavigationPage::new(&toolbar, title));
        toolbar
    }
    fn about(&self) {
        adw::AboutDialog::builder()
            .application_name("Marshal")
            .application_icon("io.github._6E6B.marshal")
            .developer_name("Marshal Contributors")
            .version(env!("CARGO_PKG_VERSION"))
            .website("https://github.com/6E6B/marshal")
            .issue_url("https://github.com/6E6B/marshal/issues")
            .license_type(gtk::License::Gpl30)
            .comments("Manage MCP servers on your desktop.")
            .build()
            .present(Some(&self.window));
    }
    fn quit(self: &Rc<Self>) {
        let active = self
            .manager
            .servers()
            .iter()
            .any(|s| self.manager.snapshot(&s.id).active());
        let finish = {
            let ui = self.clone();
            move || {
                let task = ui.manager.runtime.spawn({
                    let m = ui.manager.clone();
                    async move { m.shutdown().await }
                });
                let app = ui.window.application().unwrap();
                glib::spawn_future_local(async move {
                    let _ = task.await;
                    app.quit();
                });
            }
        };
        if active {
            let dialog = adw::AlertDialog::builder().heading("Stop Servers and Quit?").body("Closing the window keeps your servers running. Quitting stops managed servers and remote access.").build();
            dialog.add_responses(&[("cancel", "Cancel"), ("quit", "Stop and Quit")]);
            dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            dialog.connect_response(None, move |_, response| {
                if response == "quit" {
                    finish();
                }
            });
            dialog.present(Some(&self.window));
        } else {
            finish();
        }
    }
    fn remove(self: &Rc<Self>) {
        let Some(server) = self.id().and_then(|id| self.manager.server(&id).ok()) else {
            return;
        };
        let dialog = adw::AlertDialog::builder().heading(format!("Remove “{}”?", server.name)).body("The server will be stopped and its saved configuration removed. Installed packages and server files will be kept.").build();
        dialog.add_responses(&[("cancel", "Cancel"), ("remove", "Remove")]);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let ui = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response != "remove" {
                return;
            }
            let m = ui.manager.clone();
            let id = server.id.clone();
            let handle = m.runtime.spawn({
                let m = m.clone();
                async move { m.remove(&id).await }
            });
            let ui = ui.clone();
            glib::spawn_future_local(async move {
                match handle.await {
                    Ok(Ok(())) => {
                        *ui.selected.borrow_mut() = None;
                        ui.navigation.pop_to_page(&ui.overview);
                        ui.content_stack.set_visible_child_name("empty");
                        ui.refresh_list();
                        ui.update();
                        ui.split.set_show_content(false);
                        ui.overlay.add_toast(adw::Toast::new("Server removed"));
                    }
                    Ok(Err(e)) => ui.error(&e.to_string()),
                    Err(e) => ui.error(&e.to_string()),
                }
            });
        });
        dialog.present(Some(&self.window));
    }
}

#[derive(Clone)]
enum Input {
    Text(adw::EntryRow, String),
    Boolean(adw::SwitchRow),
    Choice(adw::ComboRow, Vec<Value>),
}
#[derive(Clone)]
struct Field {
    name: String,
    required: bool,
    enabled: Option<adw::SwitchRow>,
    input: Input,
}
fn schema_fields(schema: &Value, group: &adw::PreferencesGroup) -> Vec<Field> {
    let mut fields = vec![];
    let Some(properties) = schema["properties"].as_object() else {
        return fields;
    };
    for (name, property) in properties {
        let required = schema["required"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(name)));
        let title = format!(
            "{}{}",
            property["title"].as_str().unwrap_or(name),
            if required { " (Required)" } else { "" }
        );
        let enabled = if required {
            None
        } else {
            let row = adw::SwitchRow::builder()
                .title(format!("Include {name}"))
                .build();
            group.add(&row);
            Some(row)
        };
        let input = if let Some(choices) = property["enum"].as_array() {
            let strings: Vec<String> = choices
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            let strings: Vec<&str> = strings.iter().map(String::as_str).collect();
            let row = adw::ComboRow::builder()
                .title(&title)
                .subtitle(string(property, "description"))
                .model(&gtk::StringList::new(&strings))
                .build();
            row.set_use_markup(false);
            group.add(&row);
            Input::Choice(row, choices.clone())
        } else if property["type"] == "boolean" {
            let row = adw::SwitchRow::builder()
                .title(&title)
                .subtitle(string(property, "description"))
                .active(property["default"].as_bool().unwrap_or(false))
                .build();
            row.set_use_markup(false);
            group.add(&row);
            Input::Boolean(row)
        } else {
            let kind = property["type"].as_str().unwrap_or("json");
            let row = adw::EntryRow::builder()
                .title(if ["object", "array", "json"].contains(&kind) {
                    format!("{title} (JSON)")
                } else {
                    title
                })
                .build();
            row.set_use_markup(false);
            if let Some(default) = property.get("default") {
                row.set_text(default.as_str().unwrap_or(&default.to_string()));
            }
            if !string(property, "description").is_empty() {
                row.set_tooltip_text(Some(string(property, "description")));
            }
            group.add(&row);
            Input::Text(row, kind.into())
        };
        if let Some(enabled) = &enabled {
            let widget: gtk::Widget = match &input {
                Input::Text(row, _) => row.clone().upcast(),
                Input::Boolean(row) => row.clone().upcast(),
                Input::Choice(row, _) => row.clone().upcast(),
            };
            enabled
                .bind_property("active", &widget, "sensitive")
                .sync_create()
                .build();
        }
        fields.push(Field {
            name: name.clone(),
            required,
            enabled,
            input,
        });
    }
    fields
}
fn collect_fields(fields: &[Field]) -> anyhow::Result<Value> {
    let mut object = serde_json::Map::new();
    for field in fields {
        if field.enabled.as_ref().is_some_and(|r| !r.is_active()) {
            continue;
        }
        let value =
            match &field.input {
                Input::Boolean(row) => json!(row.is_active()),
                Input::Choice(row, values) => values
                    .get(row.selected() as usize)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("Choose {}", field.name))?,
                Input::Text(row, kind) => {
                    let text = row.text();
                    if field.required && text.is_empty() && kind != "string" {
                        anyhow::bail!("Enter {}", field.name);
                    }
                    match kind.as_str() {
                        "string" => json!(text.as_str()),
                        "integer" => json!(text.parse::<i64>().map_err(|_| anyhow::anyhow!(
                            "{} must be a whole number",
                            field.name
                        ))?),
                        "number" => {
                            let n = text
                                .parse::<f64>()
                                .map_err(|_| anyhow::anyhow!("{} must be a number", field.name))?;
                            if !n.is_finite() {
                                anyhow::bail!("{} must be finite", field.name);
                            }
                            json!(n)
                        }
                        _ => serde_json::from_str(&text)
                            .map_err(|e| anyhow::anyhow!("{}: {e}", field.name))?,
                    }
                }
            };
        object.insert(field.name.clone(), value);
    }
    Ok(Value::Object(object))
}

#[derive(Clone)]
struct ResultPane {
    group: adw::PreferencesGroup,
    children: Rc<RefCell<Vec<gtk::Widget>>>,
}
impl ResultPane {
    fn new(title: &str) -> Self {
        Self {
            group: adw::PreferencesGroup::builder()
                .title(title)
                .visible(false)
                .build(),
            children: Default::default(),
        }
    }
    fn add(&self, widget: &impl IsA<gtk::Widget>) {
        self.group.add(widget);
        self.children.borrow_mut().push(widget.clone().upcast());
    }
}
fn render_result(group: &ResultPane, value: &Value) {
    for child in group.children.borrow_mut().drain(..) {
        group.group.remove(&child);
    }
    let mut text = Vec::new();
    for key in ["content", "contents", "messages"] {
        if let Some(items) = value[key].as_array() {
            for item in items {
                if let Some(t) = item["text"]
                    .as_str()
                    .or_else(|| item["content"]["text"].as_str())
                {
                    text.push(t.to_owned());
                } else {
                    text.push(format!(
                        "{} content — available in Structured Result",
                        item["type"].as_str().unwrap_or("Binary")
                    ));
                }
            }
        }
    }
    if text.is_empty() {
        text.push("The request completed without text content.".into());
    }
    let output = text_view(&text.join("\n\n"), false);
    output.set_monospace(false);
    group.add(
        &gtk::ScrolledWindow::builder()
            .min_content_height(100)
            .max_content_height(320)
            .propagate_natural_height(true)
            .child(&output)
            .build(),
    );
    let raw = adw::ExpanderRow::builder()
        .title(if value["isError"].as_bool() == Some(true) {
            "Tool Reported an Error · Structured Result"
        } else {
            "Structured Result"
        })
        .build();
    let view = text_view(
        &serde_json::to_string_pretty(value).unwrap_or_default(),
        false,
    );
    raw.add_row(
        &gtk::ScrolledWindow::builder()
            .min_content_height(160)
            .max_content_height(320)
            .child(&view)
            .build(),
    );
    group.add(&raw);
}

fn environment_editor(
    parent: &adw::PreferencesDialog,
    draft: Rc<RefCell<Server>>,
    changes: Rc<RefCell<Vec<(String, String)>>>,
    manager: Arc<Manager>,
) {
    let dialog = adw::PreferencesDialog::builder()
        .title("Environment & Secrets")
        .content_width(520)
        .build();
    let page = adw::PreferencesPage::new();
    let ordinary = adw::PreferencesGroup::builder()
        .title("Environment Variables")
        .build();
    let secret = adw::PreferencesGroup::builder()
        .title("Secrets")
        .description("Values are stored in the system keyring.")
        .build();
    for (name, value) in draft.borrow().environment.clone() {
        let row = adw::EntryRow::builder().title(&name).text(&value).build();
        row.set_use_markup(false);
        let d = draft.clone();
        let key = name.clone();
        row.connect_changed(move |row| {
            d.borrow_mut()
                .environment
                .insert(key.clone(), row.text().into());
        });
        let remove = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .tooltip_text("Remove Variable")
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&remove);
        let d = draft.clone();
        let group = ordinary.clone();
        let row_clone = row.clone();
        remove.connect_clicked(move |_| {
            d.borrow_mut().environment.remove(&name);
            group.remove(&row_clone);
        });
        ordinary.add(&row);
    }
    for name in draft.borrow().secrets.clone() {
        let row = adw::PasswordEntryRow::builder()
            .title(&name)
            .sensitive(false)
            .build();
        row.set_use_markup(false);
        let c = changes.clone();
        let key = name.clone();
        let loading = Rc::new(std::cell::Cell::new(true));
        let loading_changed = loading.clone();
        row.connect_changed(move |row| {
            if loading_changed.get() {
                return;
            }
            let mut c = c.borrow_mut();
            c.retain(|(k, _)| k != &key);
            c.push((key.clone(), row.text().into()));
        });
        let remove = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .tooltip_text("Remove Secret from Server")
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&remove);
        let d = draft.clone();
        let c = changes.clone();
        let group = secret.clone();
        let row_clone = row.clone();
        let remove_name = name.clone();
        remove.connect_clicked(move |_| {
            d.borrow_mut().secrets.retain(|k| k != &remove_name);
            c.borrow_mut().retain(|(k, _)| k != &remove_name);
            group.remove(&row_clone);
        });
        secret.add(&row);
        let server_id = draft.borrow().id.clone();
        let task = manager.runtime.spawn({
            let name = name.clone();
            async move { crate::secrets::SecretStore::get(&server_id, &name).await }
        });
        let weak_dialog = dialog.downgrade();
        glib::spawn_future_local(async move {
            let result = task.await;
            row.set_sensitive(true);
            match result {
                Ok(Ok(value)) => row.set_text(&value),
                Ok(Err(error)) => {
                    if let Some(dialog) = weak_dialog.upgrade() {
                        dialog.add_toast(adw::Toast::new(&format!(
                            "Could not load {name} from the keyring: {error}"
                        )));
                    }
                }
                Err(error) => {
                    if let Some(dialog) = weak_dialog.upgrade() {
                        dialog.add_toast(adw::Toast::new(&format!(
                            "Could not load {name} from the keyring: {error}"
                        )));
                    }
                }
            }
            loading.set(false);
        });
    }
    page.add(&ordinary);
    page.add(&secret);
    let add_group = adw::PreferencesGroup::builder()
        .title("Add Variable")
        .build();
    let name = adw::EntryRow::builder().title("Variable Name").build();
    let sensitive = adw::SwitchRow::builder()
        .title("Store in Keyring")
        .active(true)
        .build();
    let value = adw::PasswordEntryRow::builder().title("Value").build();
    let add = gtk::Button::builder()
        .label("Add Variable")
        .halign(gtk::Align::Center)
        .build();
    add_group.add(&name);
    add_group.add(&value);
    add_group.add(&sensitive);
    add_group.add(&add);
    page.add(&add_group);
    let d = draft.clone();
    let c = changes.clone();
    let dialog_clone = dialog.clone();
    add.connect_clicked(move |_| {
        let key = name.text().trim().to_owned();
        if key.is_empty() {
            dialog_clone.add_toast(adw::Toast::new("Enter an environment variable name"));
            name.grab_focus();
            return;
        }
        if key.contains(['=', '\0']) {
            dialog_clone.add_toast(adw::Toast::new(
                "Enter only the variable name; do not include = or the value",
            ));
            name.grab_focus();
            return;
        }
        if RemoteConfig::is_credential_name(&key) {
            dialog_clone.add_toast(adw::Toast::new("This variable name is reserved by Marshal"));
            name.grab_focus();
            return;
        }
        if d.borrow().environment.contains_key(&key) || d.borrow().secrets.contains(&key) {
            dialog_clone.add_toast(adw::Toast::new("This variable already exists"));
            return;
        }
        let text = value.text().to_string();
        if sensitive.is_active() {
            d.borrow_mut().secrets.push(key.clone());
            c.borrow_mut().push((key.clone(), text.clone()));
            let row = adw::PasswordEntryRow::builder()
                .title(&key)
                .text(&text)
                .build();
            row.set_use_markup(false);
            let d = d.clone();
            let changed_changes = c.clone();
            let row_key = key.clone();
            row.connect_changed(move |row| {
                let mut c = changed_changes.borrow_mut();
                c.retain(|(name, _)| name != &row_key);
                c.push((row_key.clone(), row.text().into()));
            });
            let remove = gtk::Button::builder()
                .icon_name("edit-delete-symbolic")
                .tooltip_text("Remove Secret from Server")
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&remove);
            let c = c.clone();
            let group = secret.clone();
            let row_clone = row.clone();
            remove.connect_clicked(move |_| {
                d.borrow_mut().secrets.retain(|name| name != &key);
                c.borrow_mut().retain(|(name, _)| name != &key);
                group.remove(&row_clone);
            });
            secret.add(&row);
        } else {
            d.borrow_mut().environment.insert(key.clone(), text.clone());
            let row = adw::EntryRow::builder().title(&key).text(&text).build();
            row.set_use_markup(false);
            let changed_draft = d.clone();
            let row_key = key.clone();
            row.connect_changed(move |row| {
                changed_draft
                    .borrow_mut()
                    .environment
                    .insert(row_key.clone(), row.text().into());
            });
            let remove = gtk::Button::builder()
                .icon_name("edit-delete-symbolic")
                .tooltip_text("Remove Variable")
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&remove);
            let remove_draft = d.clone();
            let group = ordinary.clone();
            let row_clone = row.clone();
            remove.connect_clicked(move |_| {
                remove_draft.borrow_mut().environment.remove(&key);
                group.remove(&row_clone);
            });
            ordinary.add(&row);
        }
        name.set_text("");
        value.set_text("");
        name.grab_focus();
    });
    dialog.add(&page);
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settle() {
        let loop_ = glib::MainLoop::new(None, false);
        let done = loop_.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(350), move || done.quit());
        loop_.run();
    }
    fn descendants(widget: &impl IsA<gtk::Widget>) -> Vec<gtk::Widget> {
        let mut widgets = vec![widget.clone().upcast()];
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            widgets.extend(descendants(&current));
        }
        widgets
    }
    fn snapshot(ui: &Ui, name: &str) {
        let paintable = gtk::WidgetPaintable::new(Some(&ui.window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(
            &snapshot,
            ui.window.width() as f64,
            ui.window.height() as f64,
        );
        let node = snapshot.to_node().expect("Rendered window");
        let texture = ui.window.renderer().unwrap().render_texture(&node, None);
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ui-smoke");
        std::fs::create_dir_all(&folder).unwrap();
        texture
            .save_to_png(folder.join(format!("{name}.png")))
            .unwrap();
    }
    #[test]
    #[ignore = "Requires a graphical desktop; opens a temporary test window"]
    fn native_window_adapts_and_dialogs_render() {
        adw::init().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let manager = Manager::with_registry(
            Arc::new(tokio::runtime::Runtime::new().unwrap()),
            crate::config::ServerRegistry::at(dir.path().join("servers.json")),
        );
        let app = adw::Application::builder()
            .application_id("io.github._6E6B.marshal.UITest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(None::<&gio::Cancellable>).unwrap();
        let ui = build(&app, manager);
        settle();
        assert!(!ui.split.is_collapsed());
        assert_eq!(ui.list_stack.visible_child_name().as_deref(), Some("empty"));
        snapshot(&ui, "empty-wide");
        ui.editor(None);
        settle();
        snapshot(&ui, "add-server");
        let widgets = descendants(&ui.window.visible_dialog().unwrap());
        assert!(widgets.iter().any(|w| {
            w.clone()
                .downcast::<gtk::Button>()
                .is_ok_and(|b| b.tooltip_text().as_deref() == Some("Choose Working Directory"))
        }));
        assert!(!widgets.iter().any(|w| {
            w.clone()
                .downcast::<adw::EntryRow>()
                .is_ok_and(|r| r.title() == "Working Directory")
        }));
        ui.window.visible_dialog().unwrap().close();
        settle();
        ui.window.set_default_size(390, 700);
        settle();
        assert!(ui.split.is_collapsed());
        snapshot(&ui, "empty-narrow");
        let server: Server = serde_json::from_value(json!({
            "id": "ui-fixture", "name": "Example Server",
            "commands": [{"id": "quick", "name": "Say Hello", "command": "printf hello"}],
            "connection": {
                "transport": "stdio", "executable": "python3", "arguments": [format!("{}/tests/fixtures/mcp_server.py", env!("CARGO_MANIFEST_DIR"))], "directory": ""
            }
        })).unwrap();
        ui.manager
            .runtime
            .block_on(ui.manager.save(server, vec![]))
            .unwrap();
        ui.manager
            .runtime
            .block_on(ui.manager.start("ui-fixture"))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while ui.manager.snapshot("ui-fixture").tools.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "Fixture discovery timed out"
            );
            settle();
        }
        *ui.selected.borrow_mut() = Some("ui-fixture".into());
        ui.refresh_list();
        settle();
        assert!(ui.split.shows_content());
        snapshot(&ui, "server-narrow");
        assert_eq!(ui.command_rows.borrow().len(), 1);
        let run = ui.command_rows.borrow()[0].run.clone();
        run.emit_clicked();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ui
            .manager
            .snapshot("ui-fixture")
            .commands
            .get("quick")
            .map(String::as_str)
            != Some("Completed")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "Custom button should complete"
            );
            settle();
        }
        ui.update();
        assert_eq!(
            ui.command_rows.borrow()[0].row.subtitle().as_deref(),
            Some("Completed")
        );
        ui.command_editor("ui-fixture".into(), None);
        settle();
        snapshot(&ui, "add-command-narrow");
        assert!(
            descendants(&ui.window.visible_dialog().unwrap())
                .iter()
                .any(|widget| {
                    widget
                        .clone()
                        .downcast::<adw::EntryRow>()
                        .is_ok_and(|row| row.title() == "Button Name")
                })
        );
        ui.window.visible_dialog().unwrap().close();
        settle();
        ui.window.set_default_size(1040, 740);
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceLight);
        settle();
        snapshot(&ui, "server-light");
        let tool = ui.manager.snapshot("ui-fixture").tools[0].clone();
        ui.tool("ui-fixture", &tool);
        settle();
        snapshot(&ui, "tool-light");
        ui.navigation.pop_to_page(&ui.overview);
        ui.remote();
        settle();
        snapshot(&ui, "remote-light");
        let widgets = descendants(&ui.navigation.visible_page().unwrap());
        let remote_access = widgets
            .iter()
            .find_map(|w| {
                w.clone()
                    .downcast::<adw::ActionRow>()
                    .ok()
                    .filter(|r| r.title() == "Remote Access")
            })
            .unwrap();
        assert_eq!(remote_access.subtitle().as_deref(), Some("Off"));
        assert!(widgets.iter().any(|w| {
            w.clone()
                .downcast::<gtk::Button>()
                .is_ok_and(|b| b.label().as_deref() == Some("Enable Remote Access"))
        }));
        let provider = widgets
            .iter()
            .find_map(|w| w.clone().downcast::<adw::ComboRow>().ok())
            .unwrap();
        assert_eq!(
            provider.selected(),
            0,
            "An unset provider must not display OpenAI"
        );
        assert!(!widgets.iter().any(|w| w.is::<adw::PasswordEntryRow>()));
        provider.set_selected(1);
        settle();
        let setup = ui.window.visible_dialog().unwrap();
        assert_eq!(setup.title(), "Set Up OpenAI Tunnel");
        snapshot(&ui, "openai-setup");
        let fields = descendants(&setup);
        let credential = fields
            .iter()
            .find_map(|w| w.clone().downcast::<adw::PasswordEntryRow>().ok())
            .unwrap();
        assert_eq!(credential.title(), "API Key");
        let detail = fields
            .iter()
            .find_map(|w| w.clone().downcast::<adw::EntryRow>().ok())
            .unwrap();
        assert_eq!(detail.title(), "Tunnel ID");
        let save = fields
            .iter()
            .find_map(|w| {
                w.clone()
                    .downcast::<gtk::Button>()
                    .ok()
                    .filter(|b| b.label().as_deref() == Some("Save"))
            })
            .unwrap();
        save.emit_clicked();
        assert!(
            ui.manager
                .server("ui-fixture")
                .unwrap()
                .remote
                .provider
                .is_empty()
        );
        detail.set_text("tunnel_0123456789abcdef0123456789abcdef");
        save.emit_clicked();
        assert!(
            ui.manager
                .server("ui-fixture")
                .unwrap()
                .remote
                .provider
                .is_empty()
        );
        credential.set_text("unsaved-test-key");
        setup.close();
        settle();
        assert_eq!(
            provider.selected(),
            0,
            "Cancel must restore the saved provider"
        );
        provider.set_selected(2);
        settle();
        let setup = ui.window.visible_dialog().unwrap();
        assert_eq!(setup.title(), "Set Up ngrok");
        snapshot(&ui, "ngrok-setup");
        let fields = descendants(&setup);
        let credential = fields
            .iter()
            .find_map(|w| w.clone().downcast::<adw::PasswordEntryRow>().ok())
            .unwrap();
        assert_eq!(credential.title(), "Authtoken");
        assert!(
            credential.text().is_empty(),
            "Draft keys must not carry across providers"
        );
        setup.close();
        settle();
        assert_eq!(provider.selected(), 0);
        ui.manager.update("ui-fixture", |s| {
            s.remote_state = "Failed: Test tunnel failure".into()
        });
        ui.update();
        settle();
        let error = ui
            .window
            .visible_dialog()
            .unwrap()
            .downcast::<adw::AlertDialog>()
            .unwrap();
        assert!(error.body().contains("Test tunnel failure"));
        error.close();
        settle();
        ui.update();
        assert!(
            ui.window.visible_dialog().is_none(),
            "The same failure must only be reported once"
        );
        ui.manager.runtime.block_on(ui.manager.shutdown()).unwrap();
        ui.window.destroy();
        app.quit();
    }
}
