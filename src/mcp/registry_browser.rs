//! Browse the public MCP registry and prefill a server editor from an entry.
use crate::config::{Connection, Server};
use adw::prelude::*;
use anyhow::{Context, Result, bail};
use gtk::glib;
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
};

const REGISTRY: &str = "https://registry.modelcontextprotocol.io";
const PAGE_SIZE: usize = 40;

struct Entry {
    description: String,
    server: Server,
}

/// Converts a registry `server` object into a managed server definition.
fn to_managed(name: &str, body: &Value) -> Result<Server> {
    let title = body["title"].as_str().unwrap_or("");
    let display = if title.is_empty() {
        name.rsplit('/').next().unwrap_or(name).to_string()
    } else {
        title.to_string()
    };

    let mut environment = BTreeMap::new();
    let mut secrets = Vec::new();
    let mut apply_env = |vars: &Value| {
        for var in vars.as_array().into_iter().flatten() {
            let Some(key) = var["name"].as_str() else {
                continue;
            };
            if var["isSecret"].as_bool().unwrap_or(false)
                || var["isRequired"].as_bool().unwrap_or(false)
            {
                secrets.push(key.to_string());
            } else if let Some(value) = var["value"].as_str() {
                environment.insert(key.to_string(), value.to_string());
            }
        }
    };

    // Prefer a hosted remote over a local package.
    for remote in body["remotes"].as_array().into_iter().flatten() {
        let url = remote["url"].as_str().unwrap_or("");
        if url.is_empty() {
            continue;
        }
        let mut headers = BTreeMap::new();
        let mut secret_headers = Vec::new();
        for header in remote["headers"].as_array().into_iter().flatten() {
            let Some(key) = header["name"].as_str() else {
                continue;
            };
            if header["isSecret"].as_bool().unwrap_or(false)
                || header["value"].as_str().is_none()
            {
                secret_headers.push(key.to_string());
            } else {
                headers.insert(
                    key.to_string(),
                    header["value"].as_str().unwrap_or_default().to_string(),
                );
            }
        }
        apply_env(&remote["variables"]);
        return Ok(Server {
            id: uuid::Uuid::new_v4().to_string(),
            name: display,
            connection: Connection::Http {
                url: url.to_string(),
                headers,
                secret_headers,
            },
            environment,
            secrets,
            auto_start: false,
            restart_on_failure: false,
            remote: crate::config::RemoteConfig::default(),
            commands: vec![],
            bridge_port: None,
        });
    }

    for package in body["packages"].as_array().into_iter().flatten() {
        let registry_type = package["registryType"].as_str().unwrap_or("");
        let identifier = package["identifier"].as_str().unwrap_or("");
        if identifier.is_empty() {
            continue;
        }
        let runtime = package["runtimeHint"].as_str().unwrap_or("");
        let version = package["version"].as_str().unwrap_or("");
        let spec = if !version.is_empty() && version != "latest" {
            format!("{identifier}@{version}")
        } else {
            identifier.to_string()
        };
        let extra: Vec<String> = package["packageArguments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                a["value"]
                    .as_str()
                    .or(a["default"].as_str())
                    .map(str::to_string)
            })
            .collect();
        let runtime_args: Vec<String> = package["runtimeArguments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                a["value"]
                    .as_str()
                    .or(a["default"].as_str())
                    .map(str::to_string)
            })
            .collect();

        let (executable, arguments) = match registry_type {
            "npm" => {
                let exe = if runtime.is_empty() { "npx" } else { runtime };
                let mut args = vec!["-y".to_string(), spec];
                args.extend(runtime_args);
                args.extend(extra);
                (exe.to_string(), args)
            }
            "pypi" => {
                let exe = if runtime.is_empty() { "uvx" } else { runtime };
                let mut args = vec![spec];
                args.extend(runtime_args);
                args.extend(extra);
                (exe.to_string(), args)
            }
            _ => continue,
        };
        apply_env(&package["environmentVariables"]);
        return Ok(Server {
            id: uuid::Uuid::new_v4().to_string(),
            name: display,
            connection: Connection::Stdio {
                executable: executable.to_string(),
                arguments,
                directory: String::new(),
            },
            environment,
            secrets,
            auto_start: false,
            restart_on_failure: false,
            remote: crate::config::RemoteConfig::default(),
            commands: vec![],
            bridge_port: None,
        });
    }
    bail!("This entry has no supported remote or package")
}

/// Fetches one page of the registry, newest first.
async fn fetch_page(
    client: &reqwest::Client,
    cursor: Option<&str>,
) -> Result<(Vec<Entry>, Option<String>)> {
    let mut url = format!("{REGISTRY}/v0/servers?version=latest&limit={PAGE_SIZE}");
    if let Some(cursor) = cursor {
        url.push_str(&format!("&cursor={}", urlencoding(cursor)));
    }
    let data: Value = client
        .get(&url)
        .send()
        .await
        .context("Could not reach the MCP registry")?
        .error_for_status()
        .context("The MCP registry returned an error")?
        .json()
        .await?;
    let mut entries = Vec::new();
    for wrapper in data["servers"].as_array().into_iter().flatten() {
        let body = &wrapper["server"];
        let name = body["name"].as_str().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let Ok(server) = to_managed(&name, body) else {
            continue;
        };
        entries.push(Entry {
            description: body["description"].as_str().unwrap_or("").to_string(),
            server,
        });
    }
    let cursor = data["metadata"]["nextCursor"].as_str().map(str::to_string);
    Ok((entries, cursor))
}

fn urlencoding(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-._~".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}

/// Shows the registry browser; `on_pick` opens a prefilled server editor.
pub fn browse_dialog(
    parent: &adw::ApplicationWindow,
    runtime: Arc<tokio::runtime::Runtime>,
    on_pick: impl Fn(Server) + 'static,
) {
    let dialog = adw::Dialog::builder()
        .title("MCP Registry")
        .content_width(640)
        .content_height(560)
        .build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new("MCP Registry", "")));
    toolbar.add_top_bar(&header);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    list.set_margin_start(9);
    list.set_margin_end(9);
    list.set_margin_top(6);
    list.set_margin_bottom(9);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .build();
    let status = adw::StatusPage::builder()
        .icon_name("globe-symbolic")
        .title("Loading")
        .build();
    let spinner = gtk::Spinner::builder().spinning(true).build();
    status.set_child(Some(&spinner));
    let stack = gtk::Stack::new();
    stack.add_named(&scroll, Some("list"));
    stack.add_named(&status, Some("status"));
    stack.set_visible_child_name("status");
    toolbar.set_content(Some(&stack));
    dialog.set_child(Some(&toolbar));

    let entries: Rc<RefCell<Vec<Entry>>> = Rc::new(RefCell::new(vec![]));
    let cursor: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let loading = Rc::new(Cell::new(false));
    let on_pick = Rc::new(on_pick);

    let append_rows = {
        let list = list.clone();
        let entries = entries.clone();
        let dialog = dialog.clone();
        let on_pick = on_pick.clone();
        move |start: usize| {
            let borrowed = entries.borrow();
            for index in start..borrowed.len() {
                let entry = &borrowed[index];
                let kind = match &entry.server.connection {
                    Connection::Http { .. } => "remote",
                    Connection::Stdio { .. } => "package",
                };
                let row = adw::ActionRow::builder()
                    .title(&entry.server.name)
                    .subtitle(format!(
                        "{kind} · {}",
                        entry.description.chars().take(140).collect::<String>()
                    ))
                    .activatable(true)
                    .build();
                row.set_use_markup(false);
                let dialog = dialog.clone();
                let on_pick = on_pick.clone();
                let entries = entries.clone();
                row.connect_activated(move |_| {
                    if let Some(entry) = entries.borrow().get(index) {
                        on_pick(entry.server.clone());
                        dialog.close();
                    }
                });
                list.append(&row);
            }
        }
    };

    let load = Rc::new({
        let entries = entries.clone();
        let cursor = cursor.clone();
        let stack = stack.clone();
        let status = status.clone();
        let list = list.clone();
        let loading = loading.clone();
        let append_rows = append_rows.clone();
        move |append: bool| {
            if loading.replace(true) {
                return;
            }
            let cursor_value = if append {
                cursor.borrow().clone()
            } else {
                None
            };
            if !append {
                entries.borrow_mut().clear();
                while let Some(child) = list.first_child() {
                    list.remove(&child);
                }
            }
            let spinner_row = if entries.borrow().is_empty() {
                status.set_title("Loading");
                status.set_description(Some("Querying the MCP registry…"));
                stack.set_visible_child_name("status");
                None
            } else {
                let row = gtk::ListBoxRow::builder()
                    .child(
                        &gtk::Spinner::builder()
                            .spinning(true)
                            .halign(gtk::Align::Center)
                            .margin_top(9)
                            .margin_bottom(9)
                            .build(),
                    )
                    .build();
                list.append(&row);
                Some(row)
            };
            let http = reqwest::Client::builder()
                .user_agent(concat!("marshal/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_default();
            let task = runtime.spawn(async move { fetch_page(&http, cursor_value.as_deref()).await });
            let entries = entries.clone();
            let cursor = cursor.clone();
            let stack = stack.clone();
            let status = status.clone();
            let list = list.clone();
            let loading = loading.clone();
            let append_rows = append_rows.clone();
            glib::spawn_future_local(async move {
                if let Some(row) = &spinner_row {
                    list.remove(row);
                }
                loading.set(false);
                match task.await {
                    Ok(Ok((mut page, next))) => {
                        let start = entries.borrow().len();
                        entries.borrow_mut().append(&mut page);
                        *cursor.borrow_mut() = next;
                        append_rows(start);
                        stack.set_visible_child_name("list");
                    }
                    Ok(Err(e)) => {
                        if entries.borrow().is_empty() {
                            status.set_title("Registry Unavailable");
                            status.set_description(Some(&format!("{e:#}")));
                            stack.set_visible_child_name("status");
                        }
                    }
                    Err(e) => {
                        if entries.borrow().is_empty() {
                            status.set_title("Registry Unavailable");
                            status.set_description(Some(&e.to_string()));
                            stack.set_visible_child_name("status");
                        }
                    }
                }
            });
        }
    });

    {
        let load = load.clone();
        let cursor = cursor.clone();
        let loading = loading.clone();
        let near_end = move |adj: &gtk::Adjustment| {
            if !loading.get()
                && cursor.borrow().is_some()
                && adj.upper() - adj.value() - adj.page_size() < 300.0
            {
                load(true);
            }
        };
        let adjustment = scroll.vadjustment();
        adjustment.connect_value_changed(near_end.clone());
        adjustment.connect_changed(near_end);
    }
    dialog.present(Some(parent));
    load(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_remote_and_package_entries() {
        let http: Value = serde_json::json!({
            "name": "io.github.acme/weather",
            "title": "Weather",
            "remotes": [{
                "type": "streamable-http",
                "url": "https://mcp.acme.test/weather",
                "headers": [{"name": "Authorization", "isSecret": true}]
            }]
        });
        let server = to_managed("io.github.acme/weather", &http).unwrap();
        let Connection::Http {
            url, secret_headers, ..
        } = &server.connection
        else {
            panic!()
        };
        assert_eq!(url, "https://mcp.acme.test/weather");
        assert_eq!(secret_headers, &["Authorization".to_string()]);
        assert_eq!(server.name, "Weather");

        let npm: Value = serde_json::json!({
            "name": "io.github.acme/files",
            "packages": [{
                "registryType": "npm",
                "identifier": "@acme/mcp-files",
                "version": "1.2.3",
                "runtimeHint": "npx",
                "runtimeArguments": [{"value": "-y"}],
                "packageArguments": [{"value": "--root"}, {"value": "/srv"}],
                "environmentVariables": [{"name": "API_KEY", "isSecret": true}]
            }]
        });
        let server = to_managed("io.github.acme/files", &npm).unwrap();
        let Connection::Stdio {
            executable,
            arguments,
            ..
        } = &server.connection
        else {
            panic!()
        };
        assert_eq!(executable, "npx");
        assert_eq!(arguments[0], "-y");
        assert_eq!(arguments[1], "@acme/mcp-files@1.2.3");
        assert!(arguments.contains(&"--root".to_string()));
        assert_eq!(server.secrets, vec!["API_KEY".to_string()]);

        assert!(to_managed("x", &serde_json::json!({"name": "x"})).is_err());
    }
}
