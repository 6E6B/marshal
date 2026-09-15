mod backend;
mod config;
mod host;
mod i18n;
mod mcp;
mod secrets;
mod tray;
mod tunnel;
mod ui;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

fn resolve_server(manager: &backend::Manager, name: &str) -> Option<config::Server> {
    manager
        .servers()
        .into_iter()
        .find(|s| s.id == name || s.name.eq_ignore_ascii_case(name))
}

fn cli_list(manager: &backend::Manager) -> String {
    let mut out = String::new();
    for server in manager.servers() {
        let snap = manager.snapshot(&server.id);
        let connection = match &server.connection {
            config::Connection::Stdio { executable, .. } => executable.clone(),
            config::Connection::Http { url, .. } => url.clone(),
        };
        out.push_str(&format!("{}\t{}\t{}\n", server.name, snap.state, connection));
    }
    if out.is_empty() {
        out.push_str("No servers configured\n");
    }
    out
}

/// Runs one CLI subcommand against the manager. Returns the process exit code.
fn cli_command(
    app: &adw::Application,
    manager: &std::sync::Arc<backend::Manager>,
    args: &[String],
    cmdline: &gio::ApplicationCommandLine,
    ui_holder: &std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<ui::Ui>>>>,
    hold: &std::cell::RefCell<Option<gio::ApplicationHoldGuard>>,
) -> Option<glib::ExitCode> {
    let command = args.first()?.as_str();
    let print = |text: String| cmdline.print_literal(&text);
    let fail = |text: String| {
        cmdline.printerr_literal(&text);
        glib::ExitCode::FAILURE
    };
    let find = |name: &str| -> Result<config::Server, glib::ExitCode> {
        resolve_server(manager, name)
            .ok_or_else(|| fail(format!("No server named “{name}”\n")))
    };
    match command {
        "list" | "status" => {
            print(cli_list(manager));
            Some(glib::ExitCode::SUCCESS)
        }
        "start" | "stop" | "restart" => {
            let Some(name) = args.get(1) else {
                return Some(fail(format!("Usage: marshal {command} <server>\n")));
            };
            let server = match find(name) {
                Ok(s) => s,
                Err(code) => return Some(code),
            };
            // Starting a server keeps Marshal alive to supervise it even when
            // no window was requested.
            if command != "stop" {
                if hold.borrow().is_none() {
                    *hold.borrow_mut() = Some(app.hold());
                }
                if ui_holder.borrow().is_none() {
                    *ui_holder.borrow_mut() = Some(ui::build(app, manager.clone(), false));
                }
            }
            let runtime = manager.runtime.clone();
            let manager = manager.clone();
            let id = server.id.clone();
            let task = async move {
                match command {
                    "start" => manager.start(&id).await,
                    "stop" => manager.stop(&id).await,
                    _ => manager.restart(&id).await,
                }
            };
            match manager_timeout(&runtime, task) {
                Ok(()) => {
                    print(format!("{}: {command}ed\n", server.name));
                    Some(glib::ExitCode::SUCCESS)
                }
                Err(e) => Some(fail(format!("{}: {e:#}\n", server.name))),
            }
        }
        "profile" => {
            let Some(name) = args.get(1) else {
                return Some(fail("Usage: marshal profile <name>\n".into()));
            };
            if hold.borrow().is_none() {
                *hold.borrow_mut() = Some(app.hold());
            }
            if ui_holder.borrow().is_none() {
                *ui_holder.borrow_mut() = Some(ui::build(app, manager.clone(), false));
            }
            let runtime = manager.runtime.clone();
            let manager = manager.clone();
            let name = name.clone();
            let task = {
                let name = name.clone();
                async move { manager.start_profile(&name).await }
            };
            match manager_timeout(&runtime, task) {
                Ok(()) => {
                    print(format!("Profile “{name}” started\n"));
                    Some(glib::ExitCode::SUCCESS)
                }
                Err(e) => Some(fail(format!("{e:#}\n"))),
            }
        }
        _ => None,
    }
}

fn manager_timeout<F>(runtime: &tokio::runtime::Runtime, future: F) -> anyhow::Result<()>
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    runtime.block_on(async move {
        tokio::time::timeout(std::time::Duration::from_secs(30), future)
            .await
            .map_err(|_| anyhow::anyhow!("Operation timed out"))?
    })
}

fn main() -> glib::ExitCode {
    i18n::init();
    gtk::glib::set_application_name("Marshal");
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("Tokio runtime"));
    let manager = backend::Manager::new(runtime);
    let app = adw::Application::builder()
        .application_id("io.github._6E6B.marshal")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.add_main_option(
        "background",
        glib::Char::from(b'b'),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        "Start in the background without showing the main window",
        None,
    );

    let hold = std::rc::Rc::new(std::cell::RefCell::new(None));
    let ui_holder: std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<ui::Ui>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    {
        let ui_holder = ui_holder.clone();
        let hold = hold.clone();
        let manager = manager.clone();
        app.connect_activate(move |app| {
            if let Some(ui) = ui_holder.borrow().as_ref() {
                ui.present();
                return;
            }
            if hold.borrow().is_none() {
                *hold.borrow_mut() = Some(app.hold());
            }
            let present = !manager.settings().run_in_background_on_startup;
            let ui = ui::build(app, manager.clone(), present);
            *ui_holder.borrow_mut() = Some(ui);
        });
    }

    {
        let ui_holder = ui_holder.clone();
        let hold = hold.clone();
        let manager = manager.clone();
        app.connect_command_line(move |app, cmdline| {
            let args = cmdline.arguments();
            let positional: Vec<String> = args
                .iter()
                .skip(1)
                .filter_map(|a| a.to_str())
                .filter(|a| !a.starts_with('-'))
                .map(str::to_owned)
                .collect();
            if let Some(code) = cli_command(app, &manager, &positional, cmdline, &ui_holder, &hold) {
                return code;
            }
            let has_bg = args
                .iter()
                .any(|a| a == "--background" || a == "-b" || a == "--hidden");
            let has_show = args.iter().any(|a| a == "--show" || a == "-s");

            if let Some(ui) = ui_holder.borrow().as_ref() {
                if !has_bg || has_show {
                    ui.present();
                }
                return glib::ExitCode::SUCCESS;
            }

            if hold.borrow().is_none() {
                *hold.borrow_mut() = Some(app.hold());
            }

            let present = if has_bg {
                false
            } else if has_show {
                true
            } else {
                !manager.settings().run_in_background_on_startup
            };

            let ui = ui::build(app, manager.clone(), present);
            *ui_holder.borrow_mut() = Some(ui);

            glib::ExitCode::SUCCESS
        });
    }

    app.run()
}
