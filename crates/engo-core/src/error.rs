use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid config: {0}")]
    Config(String),

    #[error("toml parse error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("xml error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("xml attribute error: {0}")]
    XmlAttr(#[from] quick_xml::events::attributes::AttrError),

    #[error("utf-8 decode error: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("format error: {0}")]
    Format(String),
}
