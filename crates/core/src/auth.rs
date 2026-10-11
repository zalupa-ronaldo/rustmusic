use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{fmt, io::Write};
use ytmusicapi::BrowserAuth;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub cookie: String,
    pub auth_user: String,
    pub netscape: String,
}
impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("auth_user", &self.auth_user)
            .finish_non_exhaustive()
    }
}
fn allowed_cookie(name: &str) -> bool {
    matches!(
        name,
        "SID"
            | "HSID"
            | "SSID"
            | "APISID"
            | "SAPISID"
            | "LOGIN_INFO"
            | "VISITOR_INFO1_LIVE"
            | "VISITOR_PRIVACY_METADATA"
            | "YSC"
            | "PREF"
            | "CONSENT"
            | "SOCS"
    ) || name.starts_with("__Secure-1P")
        || name.starts_with("__Secure-3P")
}
fn allowed_domain(domain: &str) -> bool {
    matches!(
        domain.trim_start_matches('.'),
        "youtube.com"
            | "music.youtube.com"
            | "www.youtube.com"
            | "google.com"
            | "accounts.google.com"
    )
}
impl Session {
    pub fn import(input: &str, auth_user: &str) -> Result<Self> {
        if input.len() > 256 * 1024 {
            bail!("Слишком большой файл авторизации");
        }
        if auth_user.parse::<u8>().is_err() {
            bail!("Номер аккаунта должен быть от 0 до 255");
        }
        let (cookie, netscape) = if input.trim_start().starts_with('{') {
            let auth = BrowserAuth::from_json(input)
                .map_err(|_| anyhow::anyhow!("Ожидается JSON с полем cookie"))?;
            Self::from_header(&auth.cookie)?
        } else if input.contains('\t') {
            let mut pairs = Vec::new();
            let mut lines = Vec::new();
            for raw in input.lines() {
                let line = raw.strip_prefix("#HttpOnly_").unwrap_or(raw);
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                let fields: Vec<_> = line.split('\t').collect();
                if fields.len() != 7 {
                    bail!("Некорректный Netscape cookies файл");
                }
                if allowed_domain(fields[0])
                    && allowed_cookie(fields[5])
                    && !fields[6].contains(['\r', '\n', ';'])
                {
                    // Google cookies are retained for the resolver but not sent to music.youtube.com.
                    if fields[0].trim_start_matches('.').ends_with("youtube.com") {
                        pairs.push(format!("{}={}", fields[5], fields[6]));
                    }
                    lines.push(raw.to_owned());
                }
            }
            (
                pairs.join("; "),
                format!("# Netscape HTTP Cookie File\n{}\n", lines.join("\n")),
            )
        } else {
            Self::from_header(input.trim())?
        };
        let result = Self {
            cookie,
            auth_user: auth_user.to_owned(),
            netscape,
        };
        result.browser_auth().sapisid().map_err(|_| {
            anyhow::anyhow!(
                "В сессии нет __Secure-3PAPISID. Войдите в YouTube Music и импортируйте заново."
            )
        })?;
        Ok(result)
    }
    fn from_header(header: &str) -> Result<(String, String)> {
        if header.contains(['\r', '\n']) {
            bail!("Cookie должен быть одной строкой");
        }
        let mut pairs = Vec::new();
        let mut lines = Vec::new();
        for pair in header.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if allowed_cookie(name) && !value.contains('\t') {
                pairs.push(format!("{name}={value}"));
                lines.push(format!(".youtube.com\tTRUE\t/\tTRUE\t0\t{name}\t{value}"));
            }
        }
        Ok((
            pairs.join("; "),
            format!("# Netscape HTTP Cookie File\n{}\n", lines.join("\n")),
        ))
    }
    pub fn browser_auth(&self) -> BrowserAuth {
        BrowserAuth {
            cookie: self.cookie.clone(),
            x_goog_authuser: self.auth_user.clone(),
            origin: "https://music.youtube.com".into(),
        }
    }
    pub fn cookie_file(&self) -> Result<tempfile::NamedTempFile> {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(self.netscape.as_bytes())?;
        file.flush()?;
        Ok(file)
    }
    pub async fn from_browser(browser: &str, profile: &str, auth_user: &str) -> Result<Self> {
        if !matches!(
            browser,
            "chrome" | "firefox" | "safari" | "edge" | "brave" | "chromium"
        ) {
            bail!("Браузер не поддерживается");
        }
        if profile.contains(['\n', '\r']) {
            bail!("Некорректный профиль");
        }
        let selector = if profile.trim().is_empty() {
            browser.to_owned()
        } else {
            format!("{browser}:{}", profile.trim())
        };
        let temp = tempfile::NamedTempFile::new()?;
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(90),
            tokio::process::Command::new(crate::resolver::tool_path("yt-dlp"))
                .args([
                    "--ignore-config",
                    "--cookies-from-browser",
                    &selector,
                    "--cookies",
                ])
                .arg(temp.path())
                .args(["--skip-download", "--quiet", "https://music.youtube.com/"])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("Время импорта истекло")??;
        // yt-dlp may report a page-extraction error after successfully exporting cookies.
        let content = std::fs::read_to_string(temp.path())?;
        if content.trim().is_empty() || !content.contains("__Secure-3PAPISID") {
            let _ = output;
            bail!(
                "Не удалось прочитать сессию. Проверьте профиль, разрешение Keychain или используйте ручной импорт."
            );
        }
        Self::import(&content, auth_user)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_domains_and_cookies() {
        let input = ".youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-3PAPISID\tfixture\n.evil.test\tTRUE\t/\tTRUE\t0\tSID\tsecret\n.youtube.com\tTRUE\t/\tTRUE\t0\tother\tprivate\n";
        let session = Session::import(input, "1").unwrap();
        assert_eq!(session.cookie, "__Secure-3PAPISID=fixture");
        assert!(!session.netscape.contains("secret"));
        assert!(!format!("{session:?}").contains("fixture"));
    }
    #[test]
    fn rejects_missing_auth_and_header_injection() {
        assert!(Session::import("SID=x", "0").is_err());
        assert!(Session::import("__Secure-3PAPISID=x\r\nInjected: yes", "0").is_err());
        assert!(Session::import("__Secure-3PAPISID=x", "bad").is_err());
    }
}
