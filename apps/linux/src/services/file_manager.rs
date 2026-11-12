//! "Show in folder" through the freedesktop FileManager1 interface. There is
//! no fake fallback: if no file manager answers, the UI shows the D-Bus error.

pub fn reveal(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("В записи нет путей к файлам.".into());
    }
    let connection = zbus::blocking::Connection::session()
        .map_err(|error| format!("Нет сеанса D-Bus: {error}"))?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.FileManager1",
        "/org/freedesktop/FileManager1",
        "org.freedesktop.FileManager1",
    )
    .map_err(|error| format!("FileManager1 недоступен: {error}"))?;
    let uris: Vec<String> = paths
        .iter()
        .map(|path| crate::model::file_uri(path))
        .collect();
    let args: (Vec<String>, String) = (uris, String::new());
    let _: () = proxy
        .call("ShowItems", &args)
        .map_err(|error| format!("Не удалось открыть папку: {error}"))?;
    Ok(())
}
