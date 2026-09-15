//! Minimal gettext wiring for translated user-facing strings.
//!
//! `t()`/`f()` mark translatable text. Translations live in `po/` and are
//! installed by the Meson build through its i18n module.

use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, ngettext, setlocale,
    textdomain,
};

const DOMAIN: &str = "marshal";

pub fn init() {
    unsafe { setlocale(LocaleCategory::LcAll, "") };
    if let Some(dir) = locale_dir() {
        let _ = bindtextdomain(DOMAIN, &dir);
    }
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

fn locale_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = option_env!("MARSHAL_LOCALEDIR") {
        return Some(dir.into());
    }
    let mut candidates: Vec<std::path::PathBuf> = vec![
        "/app/share/locale".into(),
        "/usr/local/share/locale".into(),
        "/usr/share/locale".into(),
    ];
    if let Ok(exe) = std::env::current_exe()
        && let Some(prefix) = exe.parent().and_then(|p| p.parent())
    {
        candidates.insert(0, prefix.join("share").join("locale"));
    }
    candidates.into_iter().find(|p| p.is_dir())
}

/// Translate a message.
pub fn t(message: &str) -> String {
    gettext(message)
}

/// Translate a message with a singular/plural form.
#[allow(dead_code)] // Part of the translation API; used as strings migrate.
pub fn tn(singular: &str, plural: &str, n: u64) -> String {
    ngettext(singular, plural, n.min(u32::MAX as u64) as u32)
}
