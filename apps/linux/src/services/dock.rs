//! Desktop-entry identities used by the Linux dock; never shell commands.
pub const MAX_APPS: usize = 16;
pub fn normalize(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut result = Vec::new();
    for id in ids {
        if id.len() <= 255
            && id.ends_with(".desktop")
            && !id.starts_with('.')
            && !id.chars().any(|c| c.is_control() || c == '/' || c == '\\')
            && !result.contains(&id)
        {
            result.push(id);
            if result.len() == MAX_APPS {
                break;
            }
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_identity_order_is_stable_and_paths_are_not_commands() {
        assert_eq!(
            normalize(
                [
                    "a.desktop",
                    "a.desktop",
                    "../b.desktop",
                    "C:\\b.desktop",
                    "b.desktop",
                    "sh -c echo"
                ]
                .map(String::from)
            ),
            ["a.desktop", "b.desktop"]
        );
        assert_eq!(
            normalize((0..100).map(|n| format!("{n}.desktop"))).len(),
            MAX_APPS
        );
    }
}
