//! Loader messages are written in English; the app installs its translator at startup so they
//! reach the user in the interface language.

use std::sync::OnceLock;

static TRANSLATOR: OnceLock<fn(&str) -> String> = OnceLock::new();

/// Sets the function that translates message templates (once, at startup).
pub fn set_translator(translate: fn(&str) -> String) {
    let _ = TRANSLATOR.set(translate);
}

/// Translates `template`, then fills `{name}` placeholders.
pub fn trf(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = TRANSLATOR.get().map_or_else(|| template.to_string(), |t| t(template));
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}
