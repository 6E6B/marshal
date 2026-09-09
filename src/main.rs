mod backend;
mod config;
mod host;
mod mcp;
mod secrets;
mod tunnel;
mod ui;

use adw::prelude::*;

fn main() -> gtk::glib::ExitCode {
    gtk::glib::set_application_name("Marshal");
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new().expect("Tokio runtime"));
    let manager = backend::Manager::new(runtime);
    let app = adw::Application::builder()
        .application_id("io.github._6E6B.marshal")
        .build();
    let hold = std::rc::Rc::new(std::cell::RefCell::new(None));
    app.connect_activate(move |app| {
        if let Some(window) = app.windows().first() {
            window.present();
            return;
        }
        if hold.borrow().is_none() {
            *hold.borrow_mut() = Some(app.hold());
        }
        ui::build(app, manager.clone());
    });
    app.run()
}
