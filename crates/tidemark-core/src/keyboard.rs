//! Russian ЙЦУКЕН <-> US QWERTY key mapping, used by search (a query typed
//! on the wrong layout) and the Text tab's "layout fix".

const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`";
const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбюё";

/// Retypes text as if the other layout (RU <-> EN) had been on.
pub fn swap_layout(s: &str) -> String {
    let en: Vec<char> = EN.chars().collect();
    let ru: Vec<char> = RU.chars().collect();
    s.chars()
        .map(|c| {
            let lc = c.to_lowercase().next().unwrap_or(c);
            if let Some(i) = en.iter().position(|&e| e == lc) {
                ru[i]
            } else if let Some(i) = ru.iter().position(|&r| r == lc) {
                en[i]
            } else {
                lc
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_swap() {
        assert_eq!(swap_layout("ghbdtn"), "привет");
        assert_eq!(swap_layout("руддщ"), "hello");
        assert_eq!(swap_layout("Ntktuhfv"), "телеграм");
        assert_eq!(swap_layout("123"), "123");
    }
}
