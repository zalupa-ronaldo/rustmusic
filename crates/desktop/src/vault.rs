use anyhow::Result;
use rustmusic_core::Session;
fn entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(
        "beer.rustmusic.app",
        "youtube-music-session",
    )?)
}
pub fn load() -> Result<Option<Session>> {
    match entry()?.get_password() {
        Ok(value) => Ok(Some(serde_json::from_str(&value)?)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn save(session: &Session) -> Result<()> {
    entry()?.set_password(&serde_json::to_string(session)?)?;
    Ok(())
}
pub fn clear() -> Result<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
