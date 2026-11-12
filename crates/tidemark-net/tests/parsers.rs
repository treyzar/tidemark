use std::time::Duration;
use tidemark_net::{lyrics::*, translate::*, weather::*};

#[test]
fn remote_lrc_is_sorted_and_rejects_overflow_or_unicode_timestamps() {
    let lines = parse_lrc("[ar:Synthetic]\n[00:02.50][00:01] chorus\n[18446744073709551615:59] overflow\n[00:01.é] bad\n[00:60] bad\n[00:03]");
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].at, Duration::from_secs(1));
    assert_eq!(lines[1].at, Duration::from_millis(2500));
    assert_eq!(line_at(&lines, Duration::from_millis(2500)), Some(1));
    assert_eq!(
        until_next(&lines, Duration::from_secs(2)),
        Some(Duration::from_millis(500))
    );
    assert!(lines[2].text.is_empty());
}

#[test]
fn malformed_remote_conditions_are_not_displayed() {
    assert!(parse_geocoding(r#"{"results":[{"latitude":91,"longitude":0}]}"#).is_err());
    assert!(
        parse_forecast(r#"{"current":{"temperature_2m":12,"weather_code":0,"is_day":2}}"#).is_err()
    );
    let w = parse_forecast(r#"{"current":{"temperature_2m":-4.6,"weather_code":71,"is_day":0}}"#)
        .unwrap();
    assert_eq!(w.temperature_label(), "−5°");
    assert_eq!(w.icon, Icon::Snow);
    assert!(!w.is_day);
}

#[test]
fn translations_require_success_and_join_segments() {
    assert_eq!(
        parse_google(r#"[[["one ","x"],["two","y"]]]"#).unwrap(),
        "one two"
    );
    assert!(parse_mymemory(
        r#"{"responseStatus":429,"responseData":{"translatedText":"QUOTA EXCEEDED"}}"#
    )
    .is_err());
    assert_eq!(target_for("Пример текста"), TargetLanguage::English);
}

#[test]
fn duration_match_and_synced_preference_prevent_wrong_track() {
    let json = r#"[
      {"duration":190,"syncedLyrics":"[00:01]wrong"},
      {"duration":100,"plainLyrics":"plain"},
      {"duration":104,"syncedLyrics":"[00:01]right"}
    ]"#;
    let lyrics = parse_search(json, Duration::from_secs(100))
        .unwrap()
        .unwrap();
    assert_eq!(lyrics.synced[0].text, "right");
    assert!(parse_search(json, Duration::from_secs(120))
        .unwrap()
        .is_none());
}

#[test]
fn weather_fallback_handles_midnight_and_sunset_boundary() {
    let json = r#"{"current_condition":[{"temp_C":"-3","weatherCode":"116"}],"weather":[{"astronomy":[{"sunrise":"12:05 AM","sunset":"04:30 PM"}]}]}"#;
    assert!(!parse_wttr(json, 4).unwrap().is_day);
    assert!(parse_wttr(json, 5).unwrap().is_day);
    assert!(!parse_wttr(json, 990).unwrap().is_day);
    assert!(parse_wttr(json, 1440).is_err());
}

#[test]
fn disabled_and_oversized_inputs_do_not_need_network() {
    let client = tidemark_net::Client::new();
    assert_eq!(client.weather(" \t").unwrap(), None);
    assert_eq!(
        client.translate(" \n", TargetLanguage::English).unwrap(),
        ""
    );
    assert_eq!(
        client.translate(&"é".repeat(2501), TargetLanguage::Russian),
        Err(tidemark_net::Error::TooLarge)
    );
}

#[test]
fn extreme_remote_temperature_cannot_panic_label_formatting() {
    let weather = Weather {
        temperature: -1e30,
        icon: Icon::Clear,
        is_day: true,
    };
    assert!(weather.temperature_label().starts_with('−'));
}
