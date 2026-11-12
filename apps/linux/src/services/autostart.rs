//! XDG autostart entry (`~/.config/autostart/tidemark.desktop`).

use std::path::PathBuf;

use tidemark_core::config::Config;

fn path() -> Option<PathBuf> {
    Config::dir()
        .parent()
        .map(|config| config.join("autostart/tidemark.desktop"))
}

pub fn is_enabled() -> bool {
    path().is_some_and(|path| path.is_file())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let Some(path) = path() else {
        return Err("Не удалось определить папку автозапуска.".into());
    };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Не удалось отключить автозапуск: {error}")),
        };
    }
    let Some(parent) = path.parent() else {
        return Err("Не удалось определить папку автозапуска.".into());
    };
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Не удалось создать папку автозапуска: {error}"))?;
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "tidemark".into());
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Tidemark\n\
         Comment=Остров для Ubuntu\n\
         Exec=\"{}\"\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        executable.replace('\\', "\\\\").replace('"', "\\\"")
    );
    std::fs::write(&path, entry).map_err(|error| format!("Не удалось записать автозапуск: {error}"))
}
