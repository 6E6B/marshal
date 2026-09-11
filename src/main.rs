mod backend;
mod config;
mod host;
mod mcp;
mod secrets;
mod tray;
mod tunnel;
mod ui;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

fn main() -> glib::ExitCode {
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
