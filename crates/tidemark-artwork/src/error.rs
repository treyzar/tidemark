use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid artwork url: {0}")]
    Url(String),
    #[error("unsupported artwork url scheme `{0}`")]
    Scheme(String),
    #[error("userinfo in artwork url is not allowed")]
    Userinfo,
    #[error("remote file authority `{0}` is not allowed")]
    FileAuthority(String),
    #[error("file artwork url must be an absolute local path")]
    FilePath,
    #[error("file artwork must be a regular file")]
    NonRegularFile,
    #[error("data uri: {0}")]
    DataUri(&'static str),
    #[error("artwork exceeds the 8 MiB cap")]
    TooLarge,
    #[error("http status {0}")]
    Status(u16),
    #[error("redirect: {0}")]
    Redirect(&'static str),
    #[error("artwork fetch timed out")]
    Timeout,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(#[from] ureq::Error),
    #[error("image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("decoded artwork {0}x{1} exceeds limits")]
    Dimensions(u32, u32),
}
