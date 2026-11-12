use crate::{Error, Result};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeIdentity {
    pub id: u32,
    pub serial: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub level: f64,
    pub muted: bool,
    identity: NodeIdentity,
}
impl Volume {
    pub fn identity(self) -> NodeIdentity {
        self.identity
    }
}
pub fn parse_identity(text: &str) -> Result<NodeIdentity> {
    if text.len() > 65536 {
        return Err(Error::Limit);
    }
    let first = text.lines().next().ok_or(Error::InvalidResponse)?.trim();
    let (id, kind) = first
        .strip_prefix("id ")
        .and_then(|s| s.split_once(", type "))
        .ok_or(Error::InvalidResponse)?;
    if kind.split('/').next() != Some("PipeWire:Interface:Node") {
        return Err(Error::InvalidResponse);
    }
    let id = id.parse::<u32>().map_err(|_| Error::InvalidResponse)?;
    let mut serial = None;
    for line in text.lines().skip(1) {
        let line = line.trim().trim_start_matches('*').trim();
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "object.serial" {
                if serial.is_some() {
                    return Err(Error::InvalidResponse);
                }
                serial = Some(
                    value
                        .trim()
                        .trim_matches('"')
                        .parse::<u64>()
                        .map_err(|_| Error::InvalidResponse)?,
                );
            }
        }
    }
    Ok(NodeIdentity {
        id,
        serial: serial.ok_or(Error::InvalidResponse)?,
    })
}
pub fn parse_level(text: &str) -> Result<(f64, bool)> {
    if text.len() > 256 {
        return Err(Error::Limit);
    }
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() < 2
        || fields.len() > 3
        || fields[0] != "Volume:"
        || (fields.len() == 3 && fields[2] != "[MUTED]")
    {
        return Err(Error::InvalidResponse);
    }
    let level = fields[1]
        .parse::<f64>()
        .map_err(|_| Error::InvalidResponse)?;
    if !level.is_finite() || !(0.0..=16.0).contains(&level) {
        return Err(Error::InvalidResponse);
    }
    Ok((level, fields.len() == 3))
}
#[cfg(any(target_os = "linux", test))]
type Runner<'a> = dyn FnMut(&[String]) -> Result<String> + 'a;
#[cfg(any(target_os = "linux", test))]
fn inspect(run: &mut Runner<'_>) -> Result<NodeIdentity> {
    parse_identity(&run(&["inspect".into(), "@DEFAULT_AUDIO_SINK@".into()])?)
}
#[cfg(any(target_os = "linux", test))]
fn read_with(run: &mut Runner<'_>) -> Result<Volume> {
    let identity = inspect(run)?;
    let (level, muted) = parse_level(&run(&["get-volume".into(), identity.id.to_string()])?)?;
    if inspect(run)? != identity {
        return Err(Error::Changed);
    }
    Ok(Volume {
        level,
        muted,
        identity,
    })
}
#[cfg(any(target_os = "linux", test))]
fn write_with(
    run: &mut Runner<'_>,
    target: &Volume,
    level: Option<f64>,
    muted: Option<bool>,
) -> Result<Volume> {
    if level.is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n)) {
        return Err(Error::InvalidInput);
    }
    let before = read_with(run)?;
    if before.identity != target.identity {
        return Err(Error::Changed);
    }
    if let Some(level) = level {
        run(&[
            "set-volume".into(),
            target.identity.id.to_string(),
            format!("{level:.6}"),
        ])?;
    }
    if let Some(muted) = muted {
        run(&[
            "set-mute".into(),
            target.identity.id.to_string(),
            if muted { "1" } else { "0" }.into(),
        ])?;
    }
    let after = read_with(run)?;
    if after.identity != target.identity {
        return Err(Error::Changed);
    }
    if level.is_some_and(|n| (after.level - n).abs() > 0.006)
        || muted.is_some_and(|n| after.muted != n)
    {
        return Err(Error::NotConfirmed);
    }
    Ok(after)
}
#[cfg(target_os = "linux")]
fn command(args: &[String]) -> Result<String> {
    let mut command = std::process::Command::new("/usr/bin/wpctl");
    command.args(args).env("LC_ALL", "C");
    crate::process::run(command, std::time::Duration::from_secs(2), 65536)
}
#[cfg(target_os = "linux")]
pub fn read() -> Result<Volume> {
    read_with(&mut command)
}
#[cfg(target_os = "linux")]
pub fn set_level(target: &Volume, level: f64) -> Result<Volume> {
    write_with(&mut command, target, Some(level), None)
}
#[cfg(target_os = "linux")]
pub fn set_muted(target: &Volume, muted: bool) -> Result<Volume> {
    write_with(&mut command, target, None, Some(muted))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspect_requires_unique_serial_and_node_type() {
        let id=parse_identity("id 42, type PipeWire:Interface:Node/3\n * object.serial = \"901\"\n node.name = \"private speaker\"").unwrap();
        assert_eq!(
            id,
            NodeIdentity {
                id: 42,
                serial: 901
            }
        );
        assert!(
            parse_identity("id 42, type PipeWire:Interface:Device/3\nobject.serial = \"901\"")
                .is_err()
        );
        assert!(parse_identity(
            "id 42, type PipeWire:Interface:Node\nobject.serial = \"901\"\nobject.serial = \"902\""
        )
        .is_err());
        assert!(parse_identity("id 42, type PipeWire:Interface:Node").is_err());
    }
    #[test]
    fn volume_is_explicit_and_does_not_accept_nan_or_localized_noise() {
        assert_eq!(parse_level("Volume: 0.75 [MUTED]\n").unwrap(), (0.75, true));
        assert_eq!(parse_level("Volume: 1.25").unwrap(), (1.25, false));
        for bad in [
            "Volume: NaN",
            "Volume: -0.4",
            "Volume: 0.4 ignored",
            "error",
        ] {
            assert!(parse_level(bad).is_err());
        }
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    fn identity(id: u32, serial: u64) -> String {
        format!("id {id}, type PipeWire:Interface:Node/3\nobject.serial = \"{serial}\"")
    }
    #[test]
    fn changed_default_or_reused_node_id_prevents_any_write() {
        let target = Volume {
            level: 0.5,
            muted: false,
            identity: NodeIdentity { id: 42, serial: 1 },
        };
        for changed in [
            NodeIdentity { id: 43, serial: 1 },
            NodeIdentity { id: 42, serial: 2 },
        ] {
            let mut count = 0;
            let mut run = |args: &[String]| {
                count += 1;
                assert!(!args[0].starts_with("set-"));
                Ok(if args[0] == "inspect" {
                    identity(changed.id, changed.serial)
                } else {
                    "Volume: 0.5".into()
                })
            };
            assert_eq!(
                write_with(&mut run, &target, Some(0.25), None),
                Err(Error::Changed)
            );
            assert_eq!(count, 3);
        }
    }
    #[test]
    fn successful_write_uses_numeric_identity_and_observed_state() {
        let target = Volume {
            level: 0.5,
            muted: false,
            identity: NodeIdentity { id: 42, serial: 1 },
        };
        let mut commands = Vec::new();
        let mut changed = false;
        let mut run = |args: &[String]| {
            commands.push(args.to_vec());
            Ok(match args[0].as_str() {
                "inspect" => identity(42, 1),
                "set-mute" => {
                    assert_eq!(args, &["set-mute", "42", "1"]);
                    changed = true;
                    String::new()
                }
                _ => {
                    if changed {
                        "Volume: 0.5 [MUTED]".into()
                    } else {
                        "Volume: 0.5".into()
                    }
                }
            })
        };
        assert!(
            write_with(&mut run, &target, None, Some(true))
                .unwrap()
                .muted
        );
        assert_eq!(commands.iter().filter(|a| a[0] == "set-mute").count(), 1);
        let mut run = |args: &[String]| {
            Ok(if args[0] == "inspect" {
                identity(42, 1)
            } else if args[0] == "get-volume" {
                "Volume: 0.5".into()
            } else {
                String::new()
            })
        };
        assert_eq!(
            write_with(&mut run, &target, Some(0.25), None),
            Err(Error::NotConfirmed)
        );
    }
}
