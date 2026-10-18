//! Session persistence for the desktop client.
//!
//! `keyring` 3.6.3 on macOS stores the secret with the legacy
//! `SecKeychainAddGenericPassword` API. That API trusts only the creating
//! binary. RustMusic is ad-hoc signed, so the trusted-application list and the
//! keychain partition list are bound to that build's code signature. A later
//! launch — a rebuilt binary, a copy in `/Applications`, or a Gatekeeper
//! translocation path — is a different identity. `get_password` then returns
//! "not found" or an interaction error, and the client looks logged out even
//! though sign-in succeeded.
//!
//! The session file under the platform config directory is the copy that
//! survives that identity change. It is owner-readable only. The system
//! keychain is still written so a stable signature can find the same secret,
//! and an older keychain-only install is migrated into the file on the next
//! successful read.
use anyhow::{Context, Result, anyhow};
use rustmusic_core::Session;
use std::{fs, path::PathBuf};

const SERVICE: &str = "beer.rustmusic.app";
const ACCOUNT: &str = "youtube-music-session";

fn entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, ACCOUNT)?)
}

trait SecretStore {
    fn read_secret(&self) -> Result<Option<String>>;
    fn write_secret(&self, secret: &str) -> Result<()>;
    fn delete_secret(&self) -> Result<()>;
}

struct KeyringStore;

impl SecretStore for KeyringStore {
    fn read_secret(&self) -> Result<Option<String>> {
        match entry()?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn write_secret(&self, secret: &str) -> Result<()> {
        entry()?.set_password(secret)?;
        Ok(())
    }

    fn delete_secret(&self) -> Result<()> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

pub fn load() -> Result<Option<Session>> {
    load_session(&session_path()?, &KeyringStore)
}

pub fn save(session: &Session) -> Result<()> {
    save_session(&session_path()?, &KeyringStore, session)
}

pub fn clear() -> Result<()> {
    clear_session(&session_path()?, &KeyringStore)
}

fn session_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RUSTMUSIC_SESSION_FILE")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    Ok(config_dir()?.join("session.json"))
}

fn config_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .context("Не найден домашний каталог для сохранения сессии")?;
    #[cfg(target_os = "macos")]
    {
        return Ok(home.join("Library/Application Support/rustmusic"));
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Roaming"));
        return Ok(base.join("rustmusic"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Ok(base.join("rustmusic"))
    }
}

fn encode(session: &Session) -> Result<String> {
    Ok(serde_json::to_string(session)?)
}

fn decode(secret: &str) -> Result<Session> {
    serde_json::from_str(secret)
        .map_err(|_| anyhow!("Сохранённая сессия повреждена. Подключите аккаунт ещё раз."))
}

fn read_session_file(path: &std::path::Path) -> Result<Option<Session>> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(None),
        Ok(text) => decode(&text).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_session_file(path: &std::path::Path, secret: &str) -> Result<()> {
    let directory = path.parent().context("Не удалось сохранить сессию")?;
    fs::create_dir_all(directory).context("Не удалось сохранить сессию")?;
    restrict_directory(directory)?;
    let temporary = directory.join(format!(".session-{}.tmp", std::process::id()));
    let written = (|| -> Result<()> {
        fs::write(&temporary, secret)?;
        restrict_file(&temporary)?;
        replace_file(&temporary, path)?;
        restrict_file(path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.context("Не удалось сохранить сессию")
}

fn restrict_directory(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn restrict_file(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn replace_file(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) => {
            if to.exists() {
                fs::remove_file(to)?;
                fs::rename(from, to)
            } else {
                Err(error)
            }
        }
    }
}

fn load_session(path: &std::path::Path, keychain: &dyn SecretStore) -> Result<Option<Session>> {
    match read_session_file(path) {
        Ok(Some(session)) => return Ok(Some(session)),
        Ok(None) => {}
        Err(_) => {
            // A damaged file must not block startup, and its contents must not
            // be copied into the error shown in the window.
            let _ = fs::remove_file(path);
        }
    }
    match keychain.read_secret() {
        Ok(Some(secret)) => match decode(&secret) {
            Ok(session) => {
                if let Ok(encoded) = encode(&session) {
                    let _ = write_session_file(path, &encoded);
                }
                Ok(Some(session))
            }
            Err(_) => Ok(None),
        },
        Ok(None) | Err(_) => Ok(None),
    }
}

fn save_session(
    path: &std::path::Path,
    keychain: &dyn SecretStore,
    session: &Session,
) -> Result<()> {
    let secret = encode(session)?;
    write_session_file(path, &secret)?;
    // Keychain failure must not undo the file or fail sign-in. An ad-hoc
    // macOS build often cannot update the legacy item it just created.
    let _ = keychain.write_secret(&secret);
    Ok(())
}

fn clear_session(path: &std::path::Path, keychain: &dyn SecretStore) -> Result<()> {
    let keychain_result = keychain.delete_secret();
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // The next launch reads the file. A keychain item the ad-hoc signature can
    // no longer delete must not keep the user signed in.
    let _ = keychain_result;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MemoryKeychain {
        value: Mutex<Option<String>>,
    }

    impl MemoryKeychain {
        fn new(value: Option<String>) -> Self {
            Self {
                value: Mutex::new(value),
            }
        }

        fn forget(&self) {
            *self.value.lock().expect("keychain lock") = None;
        }
    }

    impl SecretStore for MemoryKeychain {
        fn read_secret(&self) -> Result<Option<String>> {
            Ok(self.value.lock().expect("keychain lock").clone())
        }

        fn write_secret(&self, secret: &str) -> Result<()> {
            *self.value.lock().expect("keychain lock") = Some(secret.to_owned());
            Ok(())
        }

        fn delete_secret(&self) -> Result<()> {
            self.forget();
            Ok(())
        }
    }

    struct DeniedKeychain;

    impl SecretStore for DeniedKeychain {
        fn read_secret(&self) -> Result<Option<String>> {
            Err(anyhow!("keychain interaction is not allowed"))
        }

        fn write_secret(&self, _secret: &str) -> Result<()> {
            Err(anyhow!("keychain interaction is not allowed"))
        }

        fn delete_secret(&self) -> Result<()> {
            Err(anyhow!("keychain interaction is not allowed"))
        }
    }

    fn fixture() -> Session {
        Session {
            cookie: "__Secure-3PAPISID=fixture".into(),
            auth_user: "0".into(),
            netscape: "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-3PAPISID\tfixture\n".into(),
        }
    }

    #[test]
    fn session_file_restores_after_keychain_forgets_the_entry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let keychain = MemoryKeychain::new(None);
        let session = fixture();
        save_session(&path, &keychain, &session).unwrap();
        // A new process whose code signature no longer matches the keychain
        // item observes NoEntry. The file must still restore the session.
        keychain.forget();
        let loaded = load_session(&path, &keychain).unwrap().unwrap();
        assert_eq!(loaded.cookie, session.cookie);
        assert_eq!(loaded.auth_user, session.auth_user);
        assert_eq!(loaded.netscape, session.netscape);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let directory_mode =
                fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(directory_mode, 0o700);
        }
    }

    #[test]
    fn keychain_session_migrates_into_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let session = fixture();
        let keychain = MemoryKeychain::new(Some(encode(&session).unwrap()));
        assert!(!path.exists());
        let loaded = load_session(&path, &keychain).unwrap().unwrap();
        assert_eq!(loaded.cookie, session.cookie);
        keychain.forget();
        let restored = load_session(&path, &keychain).unwrap().unwrap();
        assert_eq!(restored.auth_user, "0");
        assert_eq!(restored.netscape, session.netscape);
    }

    #[test]
    fn keychain_denial_does_not_drop_a_saved_session() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let session = fixture();
        save_session(&path, &DeniedKeychain, &session).unwrap();
        let loaded = load_session(&path, &DeniedKeychain).unwrap().unwrap();
        assert_eq!(loaded.cookie, session.cookie);
    }

    #[test]
    fn logout_removes_the_file_when_keychain_delete_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        save_session(&path, &DeniedKeychain, &fixture()).unwrap();
        clear_session(&path, &DeniedKeychain).unwrap();
        assert!(!path.exists());
        assert!(load_session(&path, &DeniedKeychain).unwrap().is_none());
    }

    #[test]
    fn damaged_session_file_is_removed_without_echoing_the_secret() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        fs::write(&path, b"__Secure-3PAPISID=fixture-secret {").unwrap();
        let loaded = load_session(&path, &MemoryKeychain::new(None)).unwrap();
        assert!(loaded.is_none());
        assert!(!path.exists());
        let error = decode("__Secure-3PAPISID=fixture-secret").unwrap_err();
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("fixture-secret"));
        assert!(!rendered.contains("__Secure-3PAPISID"));
    }

    #[test]
    fn session_directory_follows_the_platform_config() {
        let directory = config_dir().unwrap();
        #[cfg(target_os = "macos")]
        assert!(directory.ends_with("Library/Application Support/rustmusic"));
        #[cfg(not(target_os = "macos"))]
        assert!(directory.ends_with("rustmusic"));
    }
}
