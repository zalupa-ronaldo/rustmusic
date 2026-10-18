//! Session persistence for the desktop client.
//!
//! On macOS the login keychain stamps every item with the creating process's
//! partition. An ad-hoc signature (`codesign -s -`) has no Apple Team ID, so
//! that partition is the binary's cdhash. A rebuild or a Gatekeeper
//! translocation path that is re-signed produces a different cdhash, and the
//! next launch cannot read the item. "Always Allow" only appends the current
//! cdhash. A self-signed certificate does not change this on macOS 26: the
//! partition stays a cdhash, and a `req:` entry does not auto-grant.
//!
//! The item is therefore created and read by `/usr/bin/security`. That binary
//! is Apple-signed, so its partition is the stable `apple-tool:` identity, and
//! the ACL allows any application. Rebuilding or moving this app does not
//! change who is allowed to read. The secret is passed to `security` on stdin
//! as hex, never as an argument, and each command stays inside the tool's
//! 4096-byte interactive line. The trade-off is that any process of the same
//! user can ask `security` for the secret. It is still encrypted in the login
//! keychain, not stored as a plaintext file.
//!
//! A leftover `session.json` from the previous desktop build is copied into
//! the keychain once and then deleted. Linux and Windows keep using the
//! keyring crate (Secret Service / Credential Manager).

use anyhow::{Context, Result, anyhow};
use rustmusic_core::Session;
use std::{fs, path::PathBuf};

const SERVICE: &str = "beer.rustmusic.app";
const ACCOUNT: &str = "youtube-music-session";
/// `security -i` reads at most this many bytes, including the newline and
/// excluding the terminating NUL of its 4096-byte buffer. A longer line is
/// silently split and corrupts the item.
const SECURITY_LINE_MAX: usize = 4095;
const CHUNK_BYTES: usize = 1024;
const MAX_CHUNKS: usize = 64;
/// Production items are readable by a later build. A pinned ACL is the bug.
const KEYCHAIN_POLICY: AclPolicy = AclPolicy::AnyApplication;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AclPolicy {
    /// Trusted only for the identity that created the item (cdhash pin).
    #[cfg_attr(not(test), allow(dead_code))]
    PinnedToCreator,
    /// Not bound to the creating binary. On macOS this is the `security -A` item.
    AnyApplication,
}

trait SecretStore {
    fn read_secret(&self) -> Result<Option<String>>;
    fn write_secret(&self, secret: &str) -> Result<()>;
    fn delete_secret(&self) -> Result<()>;
}

#[cfg(not(target_os = "macos"))]
struct KeyringStore;

#[cfg(not(target_os = "macos"))]
fn entry() -> Result<keyring::Entry> {
    Ok(keyring::Entry::new(SERVICE, ACCOUNT)?)
}

#[cfg(not(target_os = "macos"))]
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

#[cfg(target_os = "macos")]
struct SecurityCliStore;

#[cfg(target_os = "macos")]
impl SecretStore for SecurityCliStore {
    fn read_secret(&self) -> Result<Option<String>> {
        // Only the items this store created. The older bare account was
        // pinned to one ad-hoc cdhash; asking `security` for it can raise a
        // keychain dialog, so a missing count means signed out.
        let Some(count) = read_count()? else {
            return Ok(None);
        };
        if count == 0 {
            return Ok(None);
        }
        let mut parts = Vec::with_capacity(count);
        for index in 0..count {
            match read_item(&chunk_account(index))? {
                Some(bytes) => parts.push(bytes),
                None => return Ok(None),
            }
        }
        Ok(Some(join_chunks(&parts)?))
    }

    fn write_secret(&self, secret: &str) -> Result<()> {
        let items = plan_items(secret)?;
        let previous = read_count().ok().flatten().unwrap_or(0);
        for item in &items {
            write_item(&item.account, &item.secret)?;
        }
        let written = items.len().saturating_sub(1);
        for index in written..previous {
            let _ = delete_item(&chunk_account(index));
        }
        Ok(())
    }

    fn delete_secret(&self) -> Result<()> {
        let count = match read_count() {
            Ok(Some(count)) => count,
            Ok(None) => 0,
            Err(_) => MAX_CHUNKS,
        };
        let mut failed = false;
        for index in 0..count {
            if delete_item(&chunk_account(index)).is_err() {
                failed = true;
            }
        }
        if delete_item(&count_account()).is_err() {
            failed = true;
        }
        // The pre-chunk item belongs to whatever binary created it. Deleting
        // it is best-effort: a cdhash ACL can refuse `security` without a
        // dialog, and sign-out must still remove the items this store wrote.
        let _ = delete_item(ACCOUNT);
        if failed {
            Err(anyhow!("Не удалось удалить сессию из Keychain"))
        } else {
            Ok(())
        }
    }
}

pub fn load() -> Result<Option<Session>> {
    let path = legacy_session_path()?;
    #[cfg(target_os = "macos")]
    {
        load_session(&path, &SecurityCliStore)
    }
    #[cfg(not(target_os = "macos"))]
    {
        load_session(&path, &KeyringStore)
    }
}

pub fn save(session: &Session) -> Result<()> {
    let path = legacy_session_path()?;
    #[cfg(target_os = "macos")]
    {
        save_session(&path, &SecurityCliStore, session)
    }
    #[cfg(not(target_os = "macos"))]
    {
        save_session(&path, &KeyringStore, session)
    }
}

pub fn clear() -> Result<()> {
    let path = legacy_session_path()?;
    #[cfg(target_os = "macos")]
    {
        clear_session(&path, &SecurityCliStore)
    }
    #[cfg(not(target_os = "macos"))]
    {
        clear_session(&path, &KeyringStore)
    }
}

fn legacy_session_path() -> Result<PathBuf> {
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
    Ok(platform_config_dir(&home))
}

#[cfg(target_os = "macos")]
fn platform_config_dir(home: &std::path::Path) -> PathBuf {
    home.join("Library/Application Support/rustmusic")
}

#[cfg(target_os = "windows")]
fn platform_config_dir(home: &std::path::Path) -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Roaming"));
    base.join("rustmusic")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir(home: &std::path::Path) -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    base.join("rustmusic")
}

fn encode(session: &Session) -> Result<String> {
    Ok(serde_json::to_string(session)?)
}

fn decode(secret: &str) -> Result<Session> {
    serde_json::from_str(secret)
        .map_err(|_| anyhow!("Сохранённая сессия повреждена. Подключите аккаунт ещё раз."))
}

enum LegacyFile {
    Missing,
    Session(Session),
    Damaged,
}

fn read_legacy_file(path: &std::path::Path) -> Result<LegacyFile> {
    match fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LegacyFile::Missing),
        Err(error) => Err(error.into()),
        Ok(text) if text.trim().is_empty() => Ok(LegacyFile::Missing),
        Ok(text) => match decode(&text) {
            Ok(session) => Ok(LegacyFile::Session(session)),
            Err(_) => Ok(LegacyFile::Damaged),
        },
    }
}

fn remove_file_if_present(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn load_session(path: &std::path::Path, keychain: &dyn SecretStore) -> Result<Option<Session>> {
    match read_legacy_file(path)? {
        LegacyFile::Session(session) => {
            let secret = encode(&session)?;
            if keychain.write_secret(&secret).is_ok() {
                remove_file_if_present(path)?;
            }
            return Ok(Some(session));
        }
        LegacyFile::Damaged => remove_file_if_present(path)?,
        LegacyFile::Missing => {
            if path.exists() {
                remove_file_if_present(path)?;
            }
        }
    }
    match keychain.read_secret() {
        Ok(Some(secret)) => match decode(&secret) {
            Ok(session) => Ok(Some(session)),
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
    keychain.write_secret(&secret)?;
    remove_file_if_present(path)?;
    Ok(())
}

fn clear_session(path: &std::path::Path, keychain: &dyn SecretStore) -> Result<()> {
    keychain.delete_secret()?;
    remove_file_if_present(path)?;
    Ok(())
}

fn chunk_account(index: usize) -> String {
    format!("{ACCOUNT}.{index}")
}

fn count_account() -> String {
    format!("{ACCOUNT}.count")
}

struct PlannedItem {
    account: String,
    secret: Vec<u8>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plan_items(secret: &str) -> Result<Vec<PlannedItem>> {
    let bytes = secret.as_bytes();
    if bytes.is_empty() {
        return Err(anyhow!("Не удалось сохранить сессию в Keychain"));
    }
    let chunks: Vec<&[u8]> = bytes.chunks(CHUNK_BYTES).collect();
    if chunks.is_empty() || chunks.len() > MAX_CHUNKS {
        return Err(anyhow!("Сессия слишком большая для записи в Keychain"));
    }
    let mut items = Vec::with_capacity(chunks.len() + 1);
    for (index, chunk) in chunks.iter().enumerate() {
        items.push(PlannedItem {
            account: chunk_account(index),
            secret: chunk.to_vec(),
        });
    }
    items.push(PlannedItem {
        account: count_account(),
        secret: chunks.len().to_string().into_bytes(),
    });
    for item in &items {
        if add_script(KEYCHAIN_POLICY, &item.account, &item.secret).len() > SECURITY_LINE_MAX {
            return Err(anyhow!("Не удалось сохранить сессию в Keychain"));
        }
    }
    Ok(items)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn add_script(policy: AclPolicy, account: &str, secret: &[u8]) -> String {
    let allow_any = match policy {
        AclPolicy::AnyApplication => "-A ",
        AclPolicy::PinnedToCreator => "",
    };
    format!(
        "add-generic-password {allow_any}-U -a {account} -s {SERVICE} -X {}\n",
        hex_encode(secret)
    )
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(any(test, target_os = "macos"))]
fn join_chunks(parts: &[Vec<u8>]) -> Result<String> {
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(part);
    }
    String::from_utf8(bytes)
        .map_err(|_| anyhow!("Сохранённая сессия повреждена. Подключите аккаунт ещё раз."))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn password_bytes(stdout: &[u8]) -> Vec<u8> {
    let mut bytes = stdout.to_vec();
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    bytes
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn output_means_missing(code: Option<i32>, stderr: &str) -> bool {
    code == Some(44) || stderr.to_ascii_lowercase().contains("could not be found")
}

#[cfg(target_os = "macos")]
fn read_count() -> Result<Option<usize>> {
    match read_item(&count_account())? {
        None => Ok(None),
        Some(bytes) => {
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                anyhow!("Сохранённая сессия повреждена. Подключите аккаунт ещё раз.")
            })?;
            let count: usize = text.trim().parse().map_err(|_| {
                anyhow!("Сохранённая сессия повреждена. Подключите аккаунт ещё раз.")
            })?;
            if count > MAX_CHUNKS {
                return Err(anyhow!(
                    "Сохранённая сессия повреждена. Подключите аккаунт ещё раз."
                ));
            }
            Ok(Some(count))
        }
    }
}

#[cfg(target_os = "macos")]
fn read_item(account: &str) -> Result<Option<Vec<u8>>> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-a", account, "-s", SERVICE, "-w"])
        .output()
        .context("Не удалось прочитать сессию из Keychain")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output_means_missing(output.status.code(), &stderr) {
        return Ok(None);
    }
    if !output.status.success() {
        return Err(anyhow!("Не удалось прочитать сессию из Keychain"));
    }
    Ok(Some(password_bytes(&output.stdout)))
}

#[cfg(target_os = "macos")]
fn write_item(account: &str, secret: &[u8]) -> Result<()> {
    let script = add_script(KEYCHAIN_POLICY, account, secret);
    if script.len() > SECURITY_LINE_MAX {
        return Err(anyhow!("Не удалось сохранить сессию в Keychain"));
    }
    let mut child = std::process::Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Не удалось сохранить сессию в Keychain")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(script.as_bytes())
            .context("Не удалось сохранить сессию в Keychain")?;
    }
    let status = child
        .wait()
        .context("Не удалось сохранить сессию в Keychain")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("Не удалось сохранить сессию в Keychain"))
    }
}

#[cfg(target_os = "macos")]
fn delete_item(account: &str) -> Result<()> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["delete-generic-password", "-a", account, "-s", SERVICE])
        .output()
        .context("Не удалось удалить сессию из Keychain")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() || output_means_missing(output.status.code(), &stderr) {
        Ok(())
    } else {
        Err(anyhow!("Не удалось удалить сессию из Keychain"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct PolicyKeychain {
        policy: AclPolicy,
        identity: Mutex<&'static str>,
        creator: Mutex<Option<&'static str>>,
        value: Mutex<Option<String>>,
    }

    impl PolicyKeychain {
        fn new(policy: AclPolicy) -> Self {
            Self {
                policy,
                identity: Mutex::new("build"),
                creator: Mutex::new(None),
                value: Mutex::new(None),
            }
        }

        fn set_identity(&self, identity: &'static str) {
            *self.identity.lock().expect("identity") = identity;
        }
    }

    impl SecretStore for PolicyKeychain {
        fn read_secret(&self) -> Result<Option<String>> {
            let identity = *self.identity.lock().expect("identity");
            let creator = *self.creator.lock().expect("creator");
            if self.policy == AclPolicy::PinnedToCreator
                && creator.is_some()
                && creator != Some(identity)
            {
                return Ok(None);
            }
            Ok(self.value.lock().expect("value").clone())
        }

        fn write_secret(&self, secret: &str) -> Result<()> {
            let identity = *self.identity.lock().expect("identity");
            *self.creator.lock().expect("creator") = Some(identity);
            *self.value.lock().expect("value") = Some(secret.to_owned());
            Ok(())
        }

        fn delete_secret(&self) -> Result<()> {
            let identity = *self.identity.lock().expect("identity");
            let creator = *self.creator.lock().expect("creator");
            if self.policy == AclPolicy::PinnedToCreator
                && creator.is_some()
                && creator != Some(identity)
            {
                return Err(anyhow!("keychain access denied"));
            }
            *self.value.lock().expect("value") = None;
            *self.creator.lock().expect("creator") = None;
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
    fn pinned_identity_cannot_read_the_item_after_rebuild() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let keychain = PolicyKeychain::new(AclPolicy::PinnedToCreator);
        save_session(&path, &keychain, &fixture()).unwrap();
        assert!(!path.exists());
        keychain.set_identity("rebuilt");
        assert!(load_session(&path, &keychain).unwrap().is_none());
    }

    #[test]
    fn any_application_policy_is_readable_after_rebuild() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let keychain = PolicyKeychain::new(KEYCHAIN_POLICY);
        let session = fixture();
        save_session(&path, &keychain, &session).unwrap();
        assert!(!path.exists());
        keychain.set_identity("rebuilt");
        let loaded = load_session(&path, &keychain).unwrap().unwrap();
        assert_eq!(loaded.cookie, session.cookie);
        assert_eq!(loaded.auth_user, session.auth_user);
        assert_eq!(loaded.netscape, session.netscape);
    }

    #[test]
    fn legacy_session_file_is_copied_into_the_keychain_and_deleted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let session = fixture();
        fs::write(&path, encode(&session).unwrap()).unwrap();
        let keychain = PolicyKeychain::new(KEYCHAIN_POLICY);
        let loaded = load_session(&path, &keychain).unwrap().unwrap();
        assert_eq!(loaded.cookie, session.cookie);
        assert!(!path.exists());
        keychain.set_identity("rebuilt");
        let restored = load_session(&path, &keychain).unwrap().unwrap();
        assert_eq!(restored.auth_user, "0");
        assert_eq!(restored.netscape, session.netscape);
    }

    #[test]
    fn legacy_file_remains_when_keychain_write_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        fs::write(&path, encode(&fixture()).unwrap()).unwrap();
        let loaded = load_session(&path, &DeniedKeychain).unwrap().unwrap();
        assert_eq!(loaded.auth_user, "0");
        assert!(path.exists());
    }

    #[test]
    fn logout_removes_the_keychain_item_and_a_leftover_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let keychain = PolicyKeychain::new(KEYCHAIN_POLICY);
        save_session(&path, &keychain, &fixture()).unwrap();
        fs::write(&path, b"__Secure-3PAPISID=leftover").unwrap();
        clear_session(&path, &keychain).unwrap();
        assert!(!path.exists());
        assert!(load_session(&path, &keychain).unwrap().is_none());
    }

    #[test]
    fn logout_keeps_the_file_when_keychain_delete_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        fs::write(&path, encode(&fixture()).unwrap()).unwrap();
        let error = clear_session(&path, &DeniedKeychain).unwrap_err();
        assert!(path.exists());
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("fixture"));
        assert!(!rendered.contains("__Secure-3PAPISID"));
    }

    #[test]
    fn damaged_session_file_is_removed_without_echoing_the_secret() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        fs::write(&path, b"__Secure-3PAPISID=fixture-secret {").unwrap();
        let loaded = load_session(&path, &PolicyKeychain::new(KEYCHAIN_POLICY)).unwrap();
        assert!(loaded.is_none());
        assert!(!path.exists());
        let error = decode("__Secure-3PAPISID=fixture-secret").unwrap_err();
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("fixture-secret"));
        assert!(!rendered.contains("__Secure-3PAPISID"));
    }

    #[test]
    fn keychain_write_script_fits_the_security_line_and_hides_the_secret() {
        let mut session = fixture();
        session.cookie = format!("{} {}", session.cookie, "x".repeat(8000));
        session.netscape.push('\n');
        session.netscape.push('"');
        let encoded = encode(&session).unwrap();
        assert!(encoded.len() > SECURITY_LINE_MAX);
        assert!(encoded.contains("__Secure-3PAPISID"));
        let items = plan_items(&encoded).unwrap();
        assert!(items.len() > 2);
        let mut raw = Vec::new();
        for item in &items {
            let script = add_script(KEYCHAIN_POLICY, &item.account, &item.secret);
            assert!(script.len() <= SECURITY_LINE_MAX);
            assert!(script.contains(" -A "));
            assert!(!script.contains("__Secure-3PAPISID"));
            assert!(!script.contains("fixture"));
            if !item.account.ends_with(".count") {
                raw.push(item.secret.clone());
            }
        }
        assert_eq!(join_chunks(&raw).unwrap(), encoded);
        let pinned = add_script(
            AclPolicy::PinnedToCreator,
            "youtube-music-session.0",
            b"abc",
        );
        assert!(!pinned.contains(" -A "));
        assert_eq!(KEYCHAIN_POLICY, AclPolicy::AnyApplication);
    }

    #[test]
    fn oversized_session_is_rejected_before_a_truncated_keychain_write() {
        let secret = "a".repeat((MAX_CHUNKS * CHUNK_BYTES) + 1);
        assert!(plan_items(&secret).is_err());
    }

    #[test]
    fn find_output_strips_one_trailing_newline_and_recognizes_item_not_found() {
        assert_eq!(password_bytes(b"abc\n"), b"abc");
        assert_eq!(password_bytes(b"ab\nc\n"), b"ab\nc");
        assert_eq!(password_bytes(b"abc\r\n"), b"abc");
        assert!(output_means_missing(Some(44), ""));
        assert!(output_means_missing(
            Some(1),
            "The specified item could not be found in the keychain."
        ));
        assert!(!output_means_missing(
            Some(36),
            "interaction is not allowed"
        ));
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
