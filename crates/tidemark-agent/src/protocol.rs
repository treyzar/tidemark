use crate::Message;
use std::io::{self, Read};
use std::time::Duration;
pub const MAX_REQUEST: usize = 4 << 20;
pub const MAX_REPLY: usize = 4096;
pub const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
pub(crate) fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid agent protocol")
}
pub fn frame<R: Read>(reader: &mut R, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut byte = [0];
    loop {
        match reader.read(&mut byte)? {
            0 => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete agent frame",
                ))
            }
            _ if byte[0] == b'\n' => return Ok(bytes),
            _ if bytes.len() == limit => return Err(invalid()),
            _ => bytes.push(byte[0]),
        }
    }
}
pub fn encode(msg: &Message) -> io::Result<Vec<u8>> {
    if !matches!(msg.kind.as_str(), "ask" | "notice") {
        return Err(invalid());
    }
    let mut bytes = serde_json::to_vec(msg).map_err(|_| invalid())?;
    if bytes.len() > MAX_REQUEST {
        return Err(invalid());
    }
    bytes.push(b'\n');
    Ok(bytes)
}
pub fn validate_choice(choice: Option<usize>, count: usize) -> Option<usize> {
    choice.filter(|&i| i < count)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_is_bounded_and_requires_newline() {
        assert_eq!(frame(&mut &b"123\n"[..], 3).unwrap(), b"123");
        assert!(frame(&mut &b"1234\n"[..], 3).is_err());
        assert!(frame(&mut &b"123"[..], 3).is_err());
    }
    #[test]
    fn indexes_cannot_escape_options() {
        assert_eq!(validate_choice(Some(2), 2), None);
        assert_eq!(validate_choice(Some(1), 2), Some(1));
    }
}
