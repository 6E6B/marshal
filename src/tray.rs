use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use gtk::glib;
use ksni::TrayMethods;

static ICON_16: &[u8] = include_bytes!("../data/icons/tray-16.bin");
static ICON_24: &[u8] = include_bytes!("../data/icons/tray-24.bin");
static ICON_32: &[u8] = include_bytes!("../data/icons/tray-32.bin");
static SYMBOLIC_SVG: &str = include_str!("../data/io.github._6E6B.marshal-symbolic.svg");
static PNG_16: &[u8] = include_bytes!("../data/icons/tray-16.png");
static PNG_24: &[u8] = include_bytes!("../data/icons/tray-24.png");
static PNG_32: &[u8] = include_bytes!("../data/icons/tray-32.png");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub id: String,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayState {
    pub window_visible: bool,
    pub servers: Vec<ServerStatus>,
}

#[derive(Debug, Clone)]
pub enum TrayAction {
    ToggleOrPresent,
    Present,
    Hide,
    Preferences,
    ToggleServer(String),
    Quit,
}

fn dispatch_action(action: TrayAction) {
    glib::idle_add_once(move || {
        crate::ui::dispatch_tray_action(action);
    });
}

pub struct MarshalTray {
    state: Arc<Mutex<TrayState>>,
    icon_theme_path: String,
}

impl ksni::Tray for MarshalTray {
    fn id(&self) -> String {
        "io.github._6E6B.marshal".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::ApplicationStatus
    }

    fn title(&self) -> String {
        let state = self.state.lock().unwrap();
        let active = state.servers.iter().filter(|s| s.active).count();
        if active == 0 {
            "Marshal".into()
        } else {
            format!("Marshal ({active} active)")
        }
    }

    fn icon_name(&self) -> String {
        "io.github._6E6B.marshal-symbolic".into()
    }

    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![
            ksni::Icon {
                width: 16,
                height: 16,
                data: ICON_16.to_vec(),
            },
            ksni::Icon {
                width: 24,
                height: 24,
                data: ICON_24.to_vec(),
            },
            ksni::Icon {
                width: 32,
                height: 32,
                data: ICON_32.to_vec(),
            },
        ]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        dispatch_action(TrayAction::ToggleOrPresent);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let mut items: Vec<ksni::MenuItem<Self>> = Vec::new();
        let state = self.state.lock().unwrap().clone();

        // 1. Open or Hide
        if state.window_visible {
            items.push(
                StandardItem {
                    label: "Hide Marshal".into(),
                    activate: Box::new(move |_| {
                        dispatch_action(TrayAction::Hide);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "Open Marshal".into(),
                    activate: Box::new(move |_| {
                        dispatch_action(TrayAction::Present);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        // 2. Server status / servers submenu
        if state.servers.is_empty() {
            items.push(
                StandardItem {
                    label: "No MCP Servers".into(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            let active_count = state.servers.iter().filter(|s| s.active).count();
            let summary = match active_count {
                0 => "Servers: All stopped".to_string(),
                1 => "Servers: 1 running".to_string(),
                n => format!("Servers: {n} running"),
            };

            let mut server_items: Vec<ksni::MenuItem<Self>> = Vec::new();
            for server in &state.servers {
                let id = server.id.clone();
                let bullet = if server.active { "● " } else { "○ " };
                let action_hint = if server.active { " (Stop)" } else { " (Start)" };
                let label = format!("{}{}{}", bullet, server.name, action_hint);
                server_items.push(
                    StandardItem {
                        label,
                        activate: Box::new(move |_| {
                            dispatch_action(TrayAction::ToggleServer(id.clone()));
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
            }

            items.push(
                SubMenu {
                    label: summary,
                    submenu: server_items,
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        // 3. Preferences
        {
            items.push(
                StandardItem {
                    label: "Preferences".into(),
                    activate: Box::new(move |_| {
                        dispatch_action(TrayAction::Preferences);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        // 4. Exit Marshal
        {
            items.push(
                StandardItem {
                    label: "Exit Marshal".into(),
                    icon_name: "application-exit".into(),
                    activate: Box::new(move |_| {
                        dispatch_action(TrayAction::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }

        items
    }
}

pub struct TrayService {
    handle: ksni::Handle<MarshalTray>,
    state: Arc<Mutex<TrayState>>,
    runtime: Arc<tokio::runtime::Runtime>,
    is_shutdown: Arc<AtomicBool>,
}

impl TrayService {
    pub fn update(&self, new_state: TrayState) {
        if self.is_shutdown.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut s = self.state.lock().unwrap();
            if *s == new_state {
                return;
            }
            *s = new_state;
        }
        let handle = self.handle.clone();
        self.runtime.spawn(async move {
            let _ = handle.update(|_| {}).await;
        });
    }

    pub fn shutdown(&self) {
        if !self.is_shutdown.swap(true, Ordering::Relaxed) {
            let awaiter = self.handle.shutdown();
            self.runtime.spawn(async move {
                awaiter.await;
            });
        }
    }
}

impl Drop for TrayService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn prepare_icon_dir() -> PathBuf {
    let icon_dir = glib::user_cache_dir().join("io.github._6E6B.marshal").join("icons");
    let scalable_dir = icon_dir.join("hicolor").join("scalable").join("apps");
    let p16_dir = icon_dir.join("hicolor").join("16x16").join("apps");
    let p24_dir = icon_dir.join("hicolor").join("24x24").join("apps");
    let p32_dir = icon_dir.join("hicolor").join("32x32").join("apps");

    let _ = std::fs::create_dir_all(&scalable_dir);
    let _ = std::fs::create_dir_all(&p16_dir);
    let _ = std::fs::create_dir_all(&p24_dir);
    let _ = std::fs::create_dir_all(&p32_dir);

    let svg_name = "io.github._6E6B.marshal-symbolic.svg";
    let png_name = "io.github._6E6B.marshal-symbolic.png";

    let _ = std::fs::write(scalable_dir.join(svg_name), SYMBOLIC_SVG);
    let _ = std::fs::write(icon_dir.join(svg_name), SYMBOLIC_SVG);

    let _ = std::fs::write(p16_dir.join(png_name), PNG_16);
    let _ = std::fs::write(p24_dir.join(png_name), PNG_24);
    let _ = std::fs::write(p32_dir.join(png_name), PNG_32);
    let _ = std::fs::write(icon_dir.join(png_name), PNG_32);

    icon_dir
}

pub fn spawn_tray(
    initial_state: TrayState,
    runtime: Arc<tokio::runtime::Runtime>,
) -> tokio::sync::oneshot::Receiver<Result<TrayService, String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let icon_theme_path = prepare_icon_dir().to_string_lossy().to_string();
    let state = Arc::new(Mutex::new(initial_state));
    let tray = MarshalTray {
        state: state.clone(),
        icon_theme_path,
    };

    let rt = runtime.clone();
    runtime.spawn(async move {
        let res = tray
            .disable_dbus_name(crate::host::in_flatpak())
            .assume_sni_available(true)
            .spawn()
            .await;
        match res {
            Ok(handle) => {
                let service = TrayService {
                    handle,
                    state,
                    runtime: rt,
                    is_shutdown: Arc::new(AtomicBool::new(false)),
                };
                let _ = tx.send(Ok(service));
            }
            Err(e) => {
                let _ = tx.send(Err(format!("Tray spawn error: {e:?}")));
            }
        }
    });

    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::Tray;

    #[test]
    fn test_embedded_pixmaps_are_valid_and_white() {
        for (name, data, size) in [
            ("16x16", ICON_16, 16usize),
            ("24x24", ICON_24, 24usize),
            ("32x32", ICON_32, 32usize),
        ] {
            assert_eq!(data.len(), size * size * 4, "Pixmap {name} has wrong byte length");
            let mut non_zero = 0;
            for chunk in data.chunks_exact(4) {
                let (a, r, g, b) = (chunk[0], chunk[1], chunk[2], chunk[3]);
                if a > 0 {
                    non_zero += 1;
                    // In ARGB network byte order, premultiplied white has R==A, G==A, B==A
                    assert_eq!(r, a, "Red channel is not white for {name}");
                    assert_eq!(g, a, "Green channel is not white for {name}");
                    assert_eq!(b, a, "Blue channel is not white for {name}");
                }
            }
            assert!(non_zero > 0, "Pixmap {name} should have visible pixels");
        }
    }

    #[test]
    fn test_symbolic_svg_has_white_color() {
        assert!(SYMBOLIC_SVG.contains("color: #ffffff"));
        assert!(SYMBOLIC_SVG.contains("io.github._6E6B.marshal.svg"));
    }

    #[test]
    fn test_prepare_icon_dir_creates_files() {
        let dir = prepare_icon_dir();
        assert!(dir.exists());
        assert!(dir.join("io.github._6E6B.marshal-symbolic.svg").exists());
        assert!(dir.join("io.github._6E6B.marshal-symbolic.png").exists());
        assert!(dir.join("hicolor/scalable/apps/io.github._6E6B.marshal-symbolic.svg").exists());
        assert!(dir.join("hicolor/16x16/apps/io.github._6E6B.marshal-symbolic.png").exists());
        assert!(dir.join("hicolor/24x24/apps/io.github._6E6B.marshal-symbolic.png").exists());
        assert!(dir.join("hicolor/32x32/apps/io.github._6E6B.marshal-symbolic.png").exists());
    }

    #[test]
    fn test_marshal_tray_properties_and_title() {
        let state = Arc::new(Mutex::new(TrayState {
            window_visible: false,
            servers: vec![],
        }));
        let tray = MarshalTray {
            state: state.clone(),
            icon_theme_path: "/tmp".into(),
        };

        assert_eq!(tray.id(), "io.github._6E6B.marshal");
        assert_eq!(tray.icon_name(), "io.github._6E6B.marshal-symbolic");
        assert_eq!(tray.title(), "Marshal");

        // When active servers exist
        state.lock().unwrap().servers.push(ServerStatus {
            id: "s1".into(),
            name: "Test Server".into(),
            active: true,
        });
        assert_eq!(tray.title(), "Marshal (1 active)");

        state.lock().unwrap().servers.push(ServerStatus {
            id: "s2".into(),
            name: "Server 2".into(),
            active: true,
        });
        assert_eq!(tray.title(), "Marshal (2 active)");
    }

    #[test]
    fn test_marshal_tray_pixmaps() {
        let state = Arc::new(Mutex::new(TrayState {
            window_visible: false,
            servers: vec![],
        }));
        let tray = MarshalTray {
            state,
            icon_theme_path: "/tmp".into(),
        };
        let pixmaps = tray.icon_pixmap();
        assert_eq!(pixmaps.len(), 3);
        assert_eq!(pixmaps[0].width, 16);
        assert_eq!(pixmaps[0].height, 16);
        assert_eq!(pixmaps[1].width, 24);
        assert_eq!(pixmaps[1].height, 24);
        assert_eq!(pixmaps[2].width, 32);
        assert_eq!(pixmaps[2].height, 32);
    }

    #[test]
    fn test_marshal_tray_menu_structure() {
        let state = Arc::new(Mutex::new(TrayState {
            window_visible: false,
            servers: vec![],
        }));
        let tray = MarshalTray {
            state: state.clone(),
            icon_theme_path: "/tmp".into(),
        };

        // When window is hidden
        let menu = tray.menu();
        assert!(!menu.is_empty());
        let labels: Vec<String> = menu.iter().filter_map(|item| match item {
            ksni::MenuItem::Standard(s) => Some(s.label.clone()),
            ksni::MenuItem::SubMenu(s) => Some(s.label.clone()),
            _ => None,
        }).collect();

        assert!(labels.iter().any(|l| l == "Open Marshal"));
        assert!(labels.iter().any(|l| l == "No MCP Servers"));
        assert!(labels.iter().any(|l| l == "Preferences"));
        assert!(labels.iter().any(|l| l == "Exit Marshal"));

        // When window is visible and server is running
        state.lock().unwrap().window_visible = true;
        state.lock().unwrap().servers.push(ServerStatus {
            id: "s1".into(),
            name: "My Server".into(),
            active: true,
        });

        let menu = tray.menu();
        let labels: Vec<String> = menu.iter().filter_map(|item| match item {
            ksni::MenuItem::Standard(s) => Some(s.label.clone()),
            ksni::MenuItem::SubMenu(s) => Some(s.label.clone()),
            _ => None,
        }).collect();

        assert!(labels.iter().any(|l| l == "Hide Marshal"));
        assert!(labels.iter().any(|l| l == "Servers: 1 running"));
    }

    #[test]
    fn test_app_settings_can_run_in_background() {
        let mut settings = crate::config::AppSettings::default();
        assert!(settings.run_in_background_on_close);
        assert!(settings.can_run_in_background());

        settings.run_in_background_on_close = false;
        settings.run_in_background_on_startup = false;
        assert!(!settings.can_run_in_background());

        settings.run_in_background_on_startup = true;
        assert!(settings.can_run_in_background());
    }
}
