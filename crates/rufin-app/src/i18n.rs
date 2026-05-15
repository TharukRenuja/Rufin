use gettextrs::{
    LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, setlocale, textdomain,
};

const DOMAIN: &str = "rufin";

pub fn init() {
    let _locale = setlocale(LocaleCategory::LcAll, "");
    let localedir = std::env::var("RUFIN_LOCALEDIR").unwrap_or_else(|_| "po".to_string());
    let _domain_dir = bindtextdomain(DOMAIN, localedir);
    let _codeset = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _domain = textdomain(DOMAIN);
}

pub fn tr(message: &str) -> String {
    gettext(message)
}
