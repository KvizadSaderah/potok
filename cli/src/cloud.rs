//! A copy of the ledger in Google Drive, encrypted before it leaves.
//!
//! The daily backup stays where it is; this sends the newest one over the
//! network as well, so losing the laptop is not losing the ledger. Nothing is
//! installed for it: Google's own API, spoken with `curl`, which the rates
//! already use.
//!
//! * **Encrypted here.** XChaCha20-Poly1305 under a key Argon2id derives from a
//!   passphrase the person chooses. Drive only ever holds ciphertext; a stolen
//!   Google account shows file names and sizes, not money. The passphrase is
//!   the only way back in: forgotten, the copies in the cloud are noise.
//! * **Narrow access.** The `drive.file` scope: potok sees the files it made
//!   and nothing else in that Drive.
//! * **Secrets outside the ledger** (it is what gets backed up) and never on
//!   a command line, where any process could read them: on macOS the
//!   Keychain through `security`'s stdin, elsewhere a file only the owner
//!   can read (`secrets.rs`); `curl` is driven through `--config -`.
//!
//! Two ways to be signed in. With gcloud already on the Mac, its own
//! sign-in: `gcloud auth application-default login` with the `drive.file`
//! scope, and a project of the person's whose quota the calls count against
//! — no client to register. Otherwise an OAuth client of the person's own,
//! made once in Google Cloud Console (a «Desktop app»); README says how.
//! potok ships none — a shared one would be somebody else's quota and
//! somebody else's consent screen.
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use potok_core::tr;

const KEY_CLIENT: &str = "google-client";
const KEY_REFRESH: &str = "google-refresh-token";
const KEY_PASSPHRASE: &str = "cloud-passphrase";
/// The Drive folder; `POTOK_CLOUD_FOLDER` points a trial somewhere else, so
/// it cannot touch the real copies.
fn folder_name() -> String {
    // A quote would end the name inside Drive's own query, so it becomes a
    // dash. Dropping the setting instead would send a trial into the real
    // folder — the one place a trial must never reach.
    match std::env::var("POTOK_CLOUD_FOLDER").ok().filter(|f| !f.is_empty()) {
        Some(trial) => trial.replace('\'', "-"),
        None => "potok-backups".into(),
    }
}
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
/// Copies kept in the cloud — the same month the local folder keeps.
const KEEP: usize = potok_core::backup::KEEP;
/// What a sealed copy starts with. The version names the cipher and the
/// Argon2 parameters, so a later change can still open older copies.
const MAGIC: &[u8; 8] = b"POTOKBK1";
const SUFFIX: &str = ".potok";

// ── Encryption ──────────────────────────────────────────────────────────────

fn key_for(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    // Explicit parameters matching POTOKBK1 (m=19456 KiB, t=2, p=1) so a
    // crate-default change never locks out existing cloud copies.
    let params = argon2::Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("argon2 params: {e}"))?;
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    Ok(key)
}

fn random<const N: usize>() -> [u8; N] {
    use password_hash::rand_core::RngCore;
    let mut out = [0u8; N];
    password_hash::rand_core::OsRng.fill_bytes(&mut out);
    out
}

/// `MAGIC ‖ salt(16) ‖ nonce(24) ‖ ciphertext+tag`.
pub fn seal(plain: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    let salt: [u8; 16] = random();
    let nonce: [u8; 24] = random();
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&key_for(passphrase, &salt)?.into());
    let sealed = cipher
        .encrypt(&nonce.into(), Payload { msg: plain, aad: MAGIC })
        .map_err(|_| anyhow::anyhow!(tr!("encryption failed", "не зашифровалось")))?;
    let mut out = Vec::with_capacity(MAGIC.len() + 40 + sealed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub fn unseal(sealed: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    if sealed.len() < MAGIC.len() + 40 || &sealed[..MAGIC.len()] != MAGIC {
        bail!(tr!("not a potok cloud copy", "это не облачная копия potok"));
    }
    let salt = &sealed[8..24];
    let nonce: [u8; 24] = sealed[24..48].try_into().expect("24 bytes");
    let cipher = chacha20poly1305::XChaCha20Poly1305::new(&key_for(passphrase, salt)?.into());
    cipher
        .decrypt(&nonce.into(), Payload { msg: &sealed[48..], aad: MAGIC })
        .map_err(|_| anyhow::anyhow!(tr!("wrong passphrase, or the copy is damaged", "не тот пароль, или копия повреждена")))
}

// ── curl, with secrets on stdin ─────────────────────────────────────────────

/// A value inside double quotes in a curl config file.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Run curl with options given as a config on stdin — tokens and secrets
/// never appear in a process list. The body comes back; a failure carries
/// Google's own answer, which says what is wrong.
fn curl(options: &[(&str, &str)]) -> Result<Vec<u8>> {
    // A newline inside a quoted value ends the line and the rest becomes
    // another curl option: `output`, `upload-file`. Values carry names and
    // ids that come from Drive, so this is checked, not assumed.
    if let Some((key, _)) = options.iter().find(|(_, v)| v.chars().any(|c| c.is_control())) {
        bail!(tr!("a control character in {key} — not sent", "управляющий символ в {key} — не отправлено"));
    }
    let mut config = String::from("silent\nshow-error\nfail-with-body\nconnect-timeout = 5\nmax-time = 15\n");
    for (key, value) in options {
        config.push_str(&format!("{key} = {}\n", quoted(value)));
    }
    let mut child = Command::new("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!(tr!("curl did not start: {e}", "curl не запустился: {e}")))?;
    child.stdin.take().context("stdin")?.write_all(config.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        // Google says what is wrong in one line inside a page of JSON.
        let body = String::from_utf8_lossy(&out.stdout);
        let said = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| {
            v["error"]["message"].as_str().or(v["error_description"].as_str()).or(v["error"].as_str()).map(str::to_string)
        });
        bail!("{}", said.unwrap_or_else(|| format!("{} {}", String::from_utf8_lossy(&out.stderr).trim(), body.trim())));
    }
    Ok(out.stdout)
}

fn json(bytes: &[u8]) -> Result<serde_json::Value> {
    serde_json::from_slice(bytes).context(tr!("Google answered something that is not JSON", "Google ответил не JSON"))
}

/// `application/x-www-form-urlencoded` and URL query values.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn base64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn from_base64url(text: &str) -> Result<Vec<u8>> {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    // Whitespace is dropped rather than refused: a code travels through a
    // terminal, a chat and a paste, and comes back wrapped.
    let mut bits = 0u32;
    let mut have = 0;
    let mut out = Vec::new();
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        let six = ABC.iter().position(|a| *a as char == c).context(tr!("that is not a potok pairing code", "это не код потока для привязки"))?;
        bits = bits << 6 | six as u32;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have & 0xff) as u8);
        }
    }
    Ok(out)
}

/// PKCE (RFC 7636): the verifier stays here, its SHA-256 goes to Google.
fn challenge(verifier: &str) -> String {
    use sha2::Digest;
    base64url(&sha2::Sha256::digest(verifier.as_bytes()))
}

// ── Google sign-in ──────────────────────────────────────────────────────────

struct Client {
    id: String,
    secret: String,
}

fn client() -> Result<Client> {
    let stored = crate::secrets::get(KEY_CLIENT)?.context(tr!("Google Drive is not set up — potok cloud login", "Google Drive не настроен — potok cloud login"))?;
    let (id, secret) = stored.split_once('\n').context(tr!("the stored client is damaged — potok cloud login", "сохранённый клиент повреждён — potok cloud login"))?;
    Ok(Client { id: id.into(), secret: secret.into() })
}

/// Pull `name=value` out of a request line's query.
fn query_value(request_line: &str, name: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| decode(value))
    })
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Sign in in the browser and get a refresh token: a loopback redirect with
/// PKCE, the way Google wants a desktop app to do it.
fn sign_in(client: &Client, email: Option<&str>) -> Result<String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("127.0.0.1")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let verifier = base64url(&random::<32>());
    let state = base64url(&random::<16>());
    let mut url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        encode(&client.id),
        encode(&redirect),
        encode(SCOPE),
        challenge(&verifier),
        state
    );
    // The account to sign in with, picked already on Google's page.
    if let Some(email) = email {
        url.push_str(&format!("&login_hint={}", encode(email)));
    }
    println!("{}", tr!("Opening the browser to sign in to Google. If it does not open:\n{url}\n", "Открываю браузер для входа в Google. Если не открылся:\n{url}\n"));
    // The browser: `open` on a Mac, `xdg-open` elsewhere. If it did not open,
    // the URL is printed above, and signing in is the same click on a link.
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = Command::new(opener).arg(&url).status();

    let (stream, _) = listener.accept().context(tr!("no answer from the browser", "браузер не ответил"))?;
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let page = |text: &str| format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<!doctype html><meta charset=utf-8><body style=\"font:16px system-ui;margin:3em\">{text}</body>");
    let mut answer = &stream;
    if let Some(error) = query_value(&request_line, "error") {
        let _ = answer.write_all(page(&tr!("potok: Google said «{error}». You can close this tab.", "potok: Google ответил «{error}». Вкладку можно закрыть.")).as_bytes());
        bail!(tr!("Google refused: {error}", "Google отказал: {error}"));
    }
    if query_value(&request_line, "state").as_deref() != Some(state.as_str()) {
        let _ = answer.write_all(page("potok: wrong state.").as_bytes());
        bail!(tr!("the answer is not to this sign-in — try again", "ответ не на этот вход — попробуй ещё раз"));
    }
    let code = query_value(&request_line, "code").context(tr!("no code in Google's answer", "в ответе Google нет кода"))?;
    let _ = answer.write_all(page(trs_signed_in()).as_bytes());

    let body = format!(
        "code={}&client_id={}&client_secret={}&redirect_uri={}&grant_type=authorization_code&code_verifier={}",
        encode(&code),
        encode(&client.id),
        encode(&client.secret),
        encode(&redirect),
        encode(&verifier)
    );
    let token = json(&curl(&[("url", "https://oauth2.googleapis.com/token"), ("data", &body)])?)?;
    token["refresh_token"]
        .as_str()
        .map(str::to_string)
        .context(tr!("Google gave no refresh token — remove potok's access in your Google account and log in again", "Google не выдал refresh token — убери доступ potok в аккаунте Google и войди снова"))
}

fn trs_signed_in() -> &'static str {
    potok_core::trs!("potok: signed in to Google Drive. You can close this tab.", "potok: вход в Google Drive выполнен. Вкладку можно закрыть.")
}

fn access_token(client: &Client) -> Result<String> {
    let refresh = crate::secrets::get(KEY_REFRESH)?.context(tr!("Google Drive is not set up — potok cloud login", "Google Drive не настроен — potok cloud login"))?;
    let body = format!(
        "client_id={}&client_secret={}&refresh_token={}&grant_type=refresh_token",
        encode(&client.id),
        encode(&client.secret),
        encode(&refresh)
    );
    let token = json(&curl(&[("url", "https://oauth2.googleapis.com/token"), ("data", &body)])?)?;
    token["access_token"].as_str().map(str::to_string).context(tr!("Google gave no access token", "Google не выдал токен доступа"))
}

// ── Drive ───────────────────────────────────────────────────────────────────

struct Drive {
    token: String,
    /// With gcloud's sign-in: the project Drive counts the calls against.
    quota_project: Option<String>,
}

/// gcloud's application-default credentials, if it has them.
fn gcloud_credentials() -> Option<PathBuf> {
    let base = match std::env::var_os("CLOUDSDK_CONFIG") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".config/gcloud"),
    };
    let file = base.join("application_default_credentials.json");
    file.exists().then_some(file)
}

/// The project Drive counts the calls against: potok's own setting, else the
/// one gcloud's sign-in was given. A device paired from another one has the
/// setting and no gcloud at all.
fn quota_project(conn: &rusqlite::Connection) -> Result<Option<String>> {
    if let Some(mine) = potok_core::settings::get(conn, potok_core::settings::CLOUD_PROJECT)? {
        return Ok(Some(mine));
    }
    let Some(file) = gcloud_credentials() else { return Ok(None) };
    Ok(std::fs::read(&file).ok().and_then(|b| json(&b).ok()).and_then(|v| v["quota_project_id"].as_str().map(str::to_string)))
}

/// A token from gcloud's sign-in, and the project to count it against.
fn gcloud_token(conn: &rusqlite::Connection) -> Result<(String, Option<String>)> {
    gcloud_credentials().context(tr!(
        "gcloud has no sign-in for applications — gcloud auth application-default login (README)",
        "у gcloud нет входа для приложений — gcloud auth application-default login (README)"
    ))?;
    let quota = quota_project(conn)?;
    let out = Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .map_err(|e| anyhow::anyhow!(tr!("gcloud did not start: {e}", "gcloud не запустился: {e}")))?;
    if !out.status.success() {
        bail!("gcloud: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok((String::from_utf8_lossy(&out.stdout).trim().to_string(), quota))
}

#[derive(Debug, Clone)]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub created: String,
    /// Which send this is: each device's send is one more than the copy it
    /// started from. Copies from before generations are 0.
    pub generation: u64,
    /// The device that sent it.
    pub device: String,
    /// Sent while another device's copy was newer — kept beside, never
    /// counted as the newest and never pruned.
    pub conflict: bool,
}

/// The file in the folder that says who writes.
const MARK: &str = "writer.json";
/// A mark older than this is a device that went away without saying so.
// ponytail: no heartbeat; a session longer than this can be taken over
// without asking — its own send then lands as a conflict copy, nothing lost.
const STALE_HOURS: i64 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub device: String,
    /// When it took the mark, RFC 3339.
    pub since: String,
}

impl Mark {
    fn stale(&self) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.since)
            .map(|t| chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(STALE_HOURS))
            .unwrap_or(true)
    }

    /// «14:02» today, else «18.10 14:02».
    pub fn since_local(&self) -> String {
        match chrono::DateTime::parse_from_rfc3339(&self.since) {
            Ok(t) => {
                let t = t.with_timezone(&chrono::Local);
                if t.date_naive() == chrono::Local::now().date_naive() { t.format("%H:%M").to_string() } else { t.format("%d.%m %H:%M").to_string() }
            }
            Err(_) => self.since.clone(),
        }
    }
}

impl Drive {
    /// The client of the person's own if there is one, else gcloud's.
    fn connect(conn: &rusqlite::Connection) -> Result<Self> {
        match client() {
            // The project goes with an own client too: a device paired from a
            // gcloud sign-in keeps those credentials as its own, and Google
            // refuses them without the project to count the calls against.
            Ok(client) => Ok(Self { token: access_token(&client)?, quota_project: quota_project(conn)? }),
            Err(_) => {
                let (token, quota_project) = gcloud_token(conn)?;
                Ok(Self { token, quota_project })
            }
        }
    }

    fn call(&self, url: &str, more: &[(&str, &str)]) -> Result<Vec<u8>> {
        let auth = format!("Authorization: Bearer {}", self.token);
        let quota = self.quota_project.as_ref().map(|p| format!("x-goog-user-project: {p}"));
        let mut options = vec![("url", url), ("header", auth.as_str())];
        if let Some(quota) = &quota {
            options.push(("header", quota.as_str()));
        }
        options.extend_from_slice(more);
        curl(&options)
    }

    fn folder(&self) -> Result<String> {
        let name = folder_name();
        let q = format!("name = '{name}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false");
        let found = json(&self.call(&format!("https://www.googleapis.com/drive/v3/files?q={}&fields=files(id)", encode(&q)), &[])?)?;
        if let Some(id) = found["files"][0]["id"].as_str() {
            return Ok(id.to_string());
        }
        let made = json(&self.call(
            "https://www.googleapis.com/drive/v3/files?fields=id",
            &[("header", "Content-Type: application/json"), ("data", &serde_json::json!({ "name": name, "mimeType": "application/vnd.google-apps.folder" }).to_string())],
        )?)?;
        made["id"].as_str().map(str::to_string).context(tr!("Drive made no folder", "Drive не создал папку"))
    }

    fn list(&self, folder: &str) -> Result<Vec<Remote>> {
        let q = format!("'{folder}' in parents and trashed = false");
        let found = json(&self.call(
            &format!("https://www.googleapis.com/drive/v3/files?q={}&orderBy=createdTime%20desc&pageSize=200&fields=files(id,name,size,createdTime,appProperties)", encode(&q)),
            &[],
        )?)?;
        Ok(found["files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .map(|f| Remote {
                        id: f["id"].as_str().unwrap_or_default().to_string(),
                        name: f["name"].as_str().unwrap_or_default().to_string(),
                        size: f["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0),
                        created: f["createdTime"].as_str().unwrap_or_default().to_string(),
                        generation: f["appProperties"]["generation"].as_str().and_then(|g| g.parse().ok()).unwrap_or(0),
                        device: f["appProperties"]["device"].as_str().unwrap_or_default().to_string(),
                        conflict: f["appProperties"]["conflict"].as_str() == Some("1"),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Two calls: the bytes, then the name and the folder. One multipart
    /// call would need JSON inside curl's form syntax, where `;` and `"` mean
    /// something else.
    fn upload(&self, folder: &str, name: &str, file: &Path, properties: &[(&str, String)]) -> Result<String> {
        let data = format!("@{}", file.display());
        let made = json(&self.call(
            "https://www.googleapis.com/upload/drive/v3/files?uploadType=media&fields=id",
            &[("max-time", "300"), ("header", "Content-Type: application/octet-stream"), ("data-binary", &data)],
        )?)?;
        let id = made["id"].as_str().context(tr!("Drive took the file but gave no id", "Drive принял файл, но не дал id"))?.to_string();
        let mut metadata = serde_json::json!({ "name": name });
        if !properties.is_empty() {
            metadata["appProperties"] = properties.iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v.clone()))).collect::<serde_json::Map<_, _>>().into();
        }
        self.call(
            &format!("https://www.googleapis.com/drive/v3/files/{id}?addParents={folder}&fields=id"),
            &[("request", "PATCH"), ("header", "Content-Type: application/json"), ("data", &metadata.to_string())],
        )?;
        Ok(id)
    }

    /// The ledger copies, newest first; the mark and anything else left out.
    fn copies(&self, folder: &str) -> Result<Vec<Remote>> {
        Ok(self.list(folder)?.into_iter().filter(|r| r.name.ends_with(SUFFIX)).collect())
    }

    /// Who writes, and the mark's file id. If two devices raced to create
    /// `writer.json`, the newest wins and stale duplicates are cleaned up.
    fn mark(&self, folder: &str) -> Result<Option<(String, Mark)>> {
        let mut marks = self.list(folder)?.into_iter().filter(|r| r.name == MARK);
        let Some(file) = marks.next() else { return Ok(None) };
        for duplicate in marks {
            let _ = self.delete(&duplicate.id);
        }
        let body = json(&self.call(&format!("https://www.googleapis.com/drive/v3/files/{}?alt=media", file.id), &[])?)?;
        Ok(Some((
            file.id,
            Mark { device: body["device"].as_str().unwrap_or_default().to_string(), since: body["since"].as_str().unwrap_or_default().to_string() },
        )))
    }

    fn put_mark(&self, folder: &str, existing: Option<&str>, mark: &Mark) -> Result<()> {
        let body = serde_json::json!({ "device": mark.device, "since": mark.since }).to_string();
        match existing {
            Some(id) => {
                self.call(
                    &format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=media&fields=id"),
                    &[("request", "PATCH"), ("header", "Content-Type: application/json"), ("data", &body)],
                )?;
            }
            None => {
                let made = json(&self.call(
                    "https://www.googleapis.com/upload/drive/v3/files?uploadType=media&fields=id",
                    &[("header", "Content-Type: application/json"), ("data", &body)],
                )?)?;
                let id = made["id"].as_str().context("mark id")?;
                self.call(
                    &format!("https://www.googleapis.com/drive/v3/files/{id}?addParents={folder}&fields=id"),
                    &[("request", "PATCH"), ("header", "Content-Type: application/json"), ("data", &format!("{{\"name\":\"{MARK}\"}}"))],
                )?;
            }
        }
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.call(&format!("https://www.googleapis.com/drive/v3/files/{id}"), &[("request", "DELETE")])?;
        Ok(())
    }

    fn download(&self, id: &str, to: &Path) -> Result<()> {
        let output = to.display().to_string();
        self.call(&format!("https://www.googleapis.com/drive/v3/files/{id}?alt=media"), &[("max-time", "300"), ("output", &output)])?;
        Ok(())
    }
}

// ── What the commands do ────────────────────────────────────────────────────

/// Set up or redo: the client, the passphrase, the sign-in. Then a first copy.
pub fn login(conn: &rusqlite::Connection, db: &Path, email: Option<&str>, project: Option<&str>) -> Result<()> {
    if let Some(project) = project {
        potok_core::settings::set(conn, potok_core::settings::CLOUD_PROJECT, project)?;
    }
    let own = client().ok();
    let via_gcloud = own.is_none() && gcloud_credentials().is_some();
    if via_gcloud {
        println!("{}", tr!("Google Drive copy of the ledger, signed in through gcloud.", "Копия леджера в Google Drive, вход через gcloud."));
        // Prove the sign-in reaches Drive before asking for anything.
        Drive::connect(conn)?.folder()?;
    } else {
        println!("{}", tr!(
            "Google Drive copy of the ledger. You need an OAuth client of your own (README, «Cloud copy»).",
            "Копия леджера в Google Drive. Нужен свой OAuth-клиент (README, «Копия в облаке»)."
        ));
    }
    // Asked once: a client already in the Keychain is used again.
    let (id, secret) = match own {
        Some(stored) => (stored.id, stored.secret),
        None if via_gcloud => (String::new(), String::new()),
        None => {
            let id = prompt(&tr!("Client ID: ", "Client ID: "), false)?;
            let secret = prompt(&tr!("Client secret: ", "Client secret: "), true)?;
            if id.is_empty() || secret.is_empty() {
                bail!(tr!("both are needed", "нужны оба"));
            }
            (id, secret)
        }
    };
    let passphrase = match crate::secrets::get(KEY_PASSPHRASE)? {
        Some(existing) if prompt(&tr!("Keep the encryption passphrase already set? [Y/n] ", "Оставить уже заданный пароль шифрования? [Y/n] "), false)?.to_lowercase() != "n" => existing,
        _ => {
            println!("{}", tr!(
                "The passphrase encrypts every copy before it leaves. Write it down somewhere other than this laptop:\nwithout it the copies in the cloud cannot be opened, by anyone.",
                "Пароль шифрует каждую копию до отправки. Запиши его не на этом ноутбуке:\nбез него копии в облаке не открыть никому."
            ));
            let first = prompt(&tr!("Passphrase: ", "Пароль: "), true)?;
            if first.chars().count() < 10 {
                bail!(tr!("at least 10 characters — it is all that guards the copies", "не короче 10 знаков — только он и защищает копии"));
            }
            if prompt(&tr!("Again: ", "Ещё раз: "), true)? != first {
                bail!(tr!("the two do not match", "не совпали"));
            }
            first
        }
    };
    if via_gcloud {
        crate::secrets::put(KEY_PASSPHRASE, &passphrase)?;
    } else {
        let client = Client { id, secret };
        let refresh = sign_in(&client, email)?;
        crate::secrets::put(KEY_CLIENT, &format!("{}\n{}", client.id, client.secret))?;
        crate::secrets::put(KEY_PASSPHRASE, &passphrase)?;
        crate::secrets::put(KEY_REFRESH, &refresh)?;
    }
    println!("{}", tr!("Signed in. Sending the first copy…", "Вход выполнен. Отправляю первую копию…"));
    let sent = push(conn, db)?;
    let folder = folder_name();
    println!("{}", tr!("in Google Drive, folder {folder}: {sent}", "в Google Drive, папка {folder}: {sent}"));
    Ok(())
}

// ── Pairing a second device ─────────────────────────────────────────────────

/// What a pairing code carries: the OAuth client and the refresh token, the
/// two things a second device cannot work out for itself.
///
/// The passphrase is **not** in it. It is the key the code is sealed with, so
/// a code seen by somebody else opens nothing, and the second device asks for
/// the passphrase it would have had to know anyway — the copies in Drive are
/// encrypted with it.
fn pack(client: &Client, refresh: &str, project: &str, passphrase: &str) -> Result<String> {
    // The same seal as a backup, deliberately: one encryption path in this
    // file, one set of Argon2 parameters, one thing to get right.
    let plain = format!("{}\n{}\n{}\n{}", client.id, client.secret, refresh, project);
    Ok(base64url(&seal(plain.as_bytes(), passphrase)?))
}

fn unpack(code: &str, passphrase: &str) -> Result<(Client, String, String)> {
    let sealed = from_base64url(code)?;
    let plain = unseal(&sealed, passphrase).map_err(|_| {
        anyhow::anyhow!(tr!(
            "wrong passphrase, or the code is not whole — it is one line, copy all of it",
            "не тот пароль, или код не целиком — он одной строкой, копируй весь"
        ))
    })?;
    let text = String::from_utf8(plain).map_err(|_| anyhow::anyhow!(tr!("the code is damaged", "код повреждён")))?;
    let mut lines = text.split('\n');
    match (lines.next(), lines.next(), lines.next()) {
        (Some(id), Some(secret), Some(refresh)) if !id.is_empty() && !secret.is_empty() && !refresh.is_empty() => {
            // The project is the fourth line and may be empty: a code from a
            // device with an OAuth client of its own needs none.
            Ok((Client { id: id.into(), secret: secret.into() }, refresh.into(), lines.next().unwrap_or_default().into()))
        }
        _ => bail!(tr!("the code is damaged", "код повреждён")),
    }
}

/// gcloud's own client and refresh token, out of its credentials file.
///
/// A device signed in through gcloud has no client of potok's own, and the
/// second device should not need gcloud installed to read a backup. These are
/// the same three fields a client of one's own gives, so what the other device
/// keeps is a potok sign-in and `gcloud` is never called there.
fn gcloud_authorized_user() -> Result<(Client, String)> {
    let file = gcloud_credentials().context(tr!("Google Drive is not set up — potok cloud login", "Google Drive не настроен — potok cloud login"))?;
    let value = json(&std::fs::read(&file)?)?;
    let field = |name: &str| value[name].as_str().map(str::to_string).filter(|v| !v.is_empty());
    match (field("client_id"), field("client_secret"), field("refresh_token")) {
        (Some(id), Some(secret), Some(refresh)) => Ok((Client { id, secret }, refresh)),
        // A service account, or a shape gcloud may yet change: better to say
        // so than to hand over a code that cannot work.
        _ => bail!(tr!(
            "gcloud's sign-in here is not one that can be handed over — on the other device: gcloud auth application-default login, then potok cloud login",
            "вход gcloud здесь не из тех, что можно передать — на другом устройстве: gcloud auth application-default login, потом potok cloud login"
        )),
    }
}

/// The code to read out on the device that is already set up.
pub fn pair_code(conn: &rusqlite::Connection) -> Result<String> {
    let passphrase = crate::secrets::get(KEY_PASSPHRASE)?.context(tr!("Google Drive is not set up — potok cloud login", "Google Drive не настроен — potok cloud login"))?;
    let (client, refresh) = match (client(), crate::secrets::get(KEY_REFRESH)?) {
        (Ok(client), Some(refresh)) => (client, refresh),
        // Signed in through gcloud: hand over the credentials gcloud holds.
        _ => gcloud_authorized_user()?,
    };
    // The project the calls count against travels with the code. gcloud's
    // credentials need one — without it Google refuses the call and names the
    // missing project — and the other device has no gcloud to ask.
    let project = quota_project(conn)?.unwrap_or_default();
    pack(&client, &refresh, &project, &passphrase)
}

/// Set this device up from a code: the same sign-in, the same passphrase, no
/// OAuth client to register and no browser.
///
/// The sign-in is proved against Drive before it is kept. A device left with
/// a stored client that Google refuses looks set up and fails once a day in
/// the background, which is the one outcome worth a network call here.
pub fn pair(conn: &rusqlite::Connection, code: &str) -> Result<()> {
    let passphrase = prompt(&tr!("Passphrase (the same as on the other device): ", "Пароль (тот же, что на другом устройстве): "), true)?;
    let (client, refresh, project) = unpack(code, &passphrase)?;
    if !project.is_empty() {
        potok_core::settings::set(conn, potok_core::settings::CLOUD_PROJECT, &project)?;
    }
    let before: Vec<(&str, Option<String>)> =
        [KEY_CLIENT, KEY_REFRESH, KEY_PASSPHRASE].iter().map(|key| (*key, crate::secrets::get(key).ok().flatten())).collect();
    crate::secrets::put(KEY_CLIENT, &format!("{}\n{}", client.id, client.secret))?;
    crate::secrets::put(KEY_REFRESH, &refresh)?;
    crate::secrets::put(KEY_PASSPHRASE, &passphrase)?;
    if let Err(why) = Drive::connect(conn).and_then(|drive| drive.folder()) {
        // Put back whatever was here, so a failed pairing leaves the device as
        // it was rather than half set up.
        for (key, value) in before {
            match value {
                Some(value) => crate::secrets::put(key, &value)?,
                None => crate::secrets::forget(key),
            }
        }
        return Err(why);
    }
    println!("{}", tr!("Paired. The copies in Drive open with this passphrase.", "Привязано. Копии в Drive открываются этим паролем."));
    Ok(())
}

pub fn logout() {
    for key in [KEY_REFRESH, KEY_CLIENT, KEY_PASSPHRASE] {
        crate::secrets::forget(key);
    }
}

/// Whether this ledger goes to Drive: the default ledger only. Another one
/// opened with `--db` — a copy to try something on — shares the Keychain and
/// so the folder, and could send itself as the newest and be taken by the
/// real ledger next start. A trial says where with `POTOK_CLOUD_FOLDER`.
pub fn applies_to(db: &Path) -> bool {
    let trial = std::env::var("POTOK_CLOUD_FOLDER").is_ok_and(|f| !f.is_empty());
    let default = potok_core::db::default_path().ok();
    (trial || default.as_deref() == Some(db)) && configured()
}

/// Set up at all: a passphrase, and a sign-in of potok's own or gcloud's.
pub fn configured() -> bool {
    matches!(crate::secrets::get(KEY_PASSPHRASE), Ok(Some(_))) && (matches!(crate::secrets::get(KEY_REFRESH), Ok(Some(_))) || gcloud_credentials().is_some())
}

/// This device's name in the writer mark: the setting, else the Mac's own.
pub fn device_name(conn: &rusqlite::Connection) -> String {
    if let Ok(Some(name)) = potok_core::settings::get(conn, potok_core::settings::DEVICE_NAME) {
        return name;
    }
    // A Mac's name reads better than its hostname («Dmitry's MacBook Pro»
    // against «dmitrys-mbp»), but `scutil` is only there; after that, hostname.
    let named = if cfg!(target_os = "macos") {
        Command::new("scutil").args(["--get", "ComputerName"]).output().ok()
    } else {
        Command::new("hostname").output().ok()
    };
    named
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| tr!("this machine", "эта машина"))
}

fn generation(conn: &rusqlite::Connection) -> Result<Option<u64>> {
    Ok(potok_core::settings::get(conn, potok_core::settings::CLOUD_GENERATION)?.and_then(|g| g.parse().ok()))
}

/// Changed since the last send or take: a journal entry after the mark, or
/// never synced at all.
pub fn dirty(conn: &rusqlite::Connection) -> Result<bool> {
    let mark: Option<i64> = potok_core::settings::get(conn, potok_core::settings::CLOUD_LOG_MARK)?.and_then(|m| m.parse().ok());
    Ok(match (generation(conn)?, mark) {
        (Some(_), Some(mark)) => potok_core::journal::last_id(conn)? > mark,
        _ => true,
    })
}

fn synced(conn: &rusqlite::Connection, generation: u64) -> Result<()> {
    potok_core::settings::set(conn, potok_core::settings::CLOUD_GENERATION, &generation.to_string())?;
    potok_core::settings::set(conn, potok_core::settings::CLOUD_LOG_MARK, &potok_core::journal::last_id(conn)?.to_string())?;
    Ok(())
}

/// The newest copy that is not a conflict copy.
fn newest(copies: &[Remote]) -> Option<&Remote> {
    copies.iter().filter(|r| !r.conflict).max_by_key(|r| (r.generation, r.created.clone()))
}

/// This ledger as it is now, sealed into a hidden file in the backup folder;
/// its name in Drive.
///
/// Always a fresh copy, and outside the rotation of daily backups. The daily
/// send used to take the newest file in the folder: one made in the morning,
/// before the day's changes, which were then counted as sent — or one another
/// ledger had put there.
fn sealed_copy(conn: &rusqlite::Connection, db: &Path) -> Result<(PathBuf, String)> {
    let dir = potok_core::backup::dir(conn, db)?;
    std::fs::create_dir_all(&dir)?;
    // A send cut short — the program quit under it — leaves its staged file.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if (name.starts_with(".ledger-") && name.ends_with(SUFFIX)) || name == ".sending.db" {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let plain = dir.join(".sending.db");
    potok_core::backup::snapshot(conn, &plain)?;
    let read = std::fs::read(&plain);
    let _ = std::fs::remove_file(&plain);
    let passphrase = crate::secrets::get(KEY_PASSPHRASE)?.context(tr!("no encryption passphrase — potok cloud login", "нет пароля шифрования — potok cloud login"))?;
    let sealed = seal(&read?, &passphrase)?;
    let name = format!("ledger-{}.db{SUFFIX}", chrono::Local::now().format("%Y-%m-%d-%H%M%S-%3f"));
    let staged = dir.join(format!(".{name}"));
    potok_core::db::write_private(&staged, &sealed)?;
    Ok((staged, name))
}

/// A name from Drive used as a file name on this disk: its last component
/// only, and nothing in it that could mean a folder. `ledger-…db.potok`
/// passes untouched; `x/../../../y` cannot leave the backup folder.
fn safe_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = last.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
    let cleaned = cleaned.trim_start_matches('.').to_string();
    if cleaned.is_empty() { "copy.db".into() } else { cleaned }
}

/// A device name safe inside a file name.
fn slug(device: &str) -> String {
    device.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect()
}

/// Send this ledger as a conflict copy: beside the newer one, not over it.
fn send_conflict(conn: &rusqlite::Connection, db: &Path, drive: &Drive, folder: &str, device: &str) -> Result<String> {
    let (staged, name) = sealed_copy(conn, db)?;
    let name = name.replacen(".db", &format!("-conflict-{}.db", slug(device)), 1);
    let base = generation(conn)?.unwrap_or(0).to_string();
    let sent = drive.upload(folder, &name, &staged, &[("generation", base), ("device", device.to_string()), ("conflict", "1".into())]);
    let _ = std::fs::remove_file(&staged);
    sent.map(|_| name)
}

/// Send this ledger as the next generation — unless another device sent a
/// newer one since this ledger's own: then it goes beside as a conflict copy
/// and the error says so. Keeps the newest [`KEEP`] ordinary copies. Returns
/// the name it has in Drive.
pub fn push(conn: &rusqlite::Connection, db: &Path) -> Result<String> {
    let device = device_name(conn);
    let base = generation(conn)?.unwrap_or(0);
    let (staged, name) = sealed_copy(conn, db)?;
    let sent = (|| {
        let drive = Drive::connect(conn)?;
        let folder = drive.folder()?;
        let copies = drive.copies(&folder)?;
        if let Some(other) = newest(&copies).filter(|r| r.generation > base) {
            let _ = std::fs::remove_file(&staged);
            let kept = send_conflict(conn, db, &drive, &folder, &device)?;
            bail!(tr!(
                "{} sent a newer copy (generation {}) after this one was taken; this ledger went beside it as {kept}, nothing was overwritten — compare the two before writing more",
                "{} отправил копию новее (поколение {}) после того, как эту взяли; этот леджер лёг рядом как {kept}, ничего не затёрто — сравни их, прежде чем писать дальше",
                if other.device.is_empty() { "another device" } else { &other.device },
                other.generation
            ));
        }
        let next = base + 1;
        if !copies.iter().any(|r| r.name == name) {
            drive.upload(&folder, &name, &staged, &[("generation", next.to_string()), ("device", device.clone())])?;
        }
        for old in drive.copies(&folder)?.into_iter().filter(|r| !r.conflict).skip(KEEP) {
            drive.delete(&old.id)?;
        }
        Ok::<_, anyhow::Error>(next)
    })();
    let _ = std::fs::remove_file(&staged);
    match sent {
        Ok(next) => {
            synced(conn, next)?;
            potok_core::settings::set(conn, potok_core::settings::CLOUD_SENT, &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string())?;
            potok_core::settings::clear(conn, potok_core::settings::CLOUD_ERROR)?;
            Ok(name)
        }
        Err(error) => {
            potok_core::settings::set(conn, potok_core::settings::CLOUD_ERROR, &format!("{error:#}"))?;
            Err(error)
        }
    }
}

/// What starting with Drive came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// Not set up: a ledger on this Mac only.
    Off,
    /// This device writes. `took_from`: a mark that was another's (stale, or
    /// taken over); `pulled`: a newer copy came from that device;
    /// `conflict`: this ledger had changes of its own and went to Drive
    /// beside it under this name first.
    Writer { took_from: Option<String>, pulled: Option<String>, conflict: Option<String> },
    /// Another device writes: open to read only.
    ReadOnly(Mark),
    /// Drive did not answer; the ledger opens as it is, unchecked.
    Unreachable(String),
}

/// What to tell the person about a start, if anything.
pub fn start_note(start: &Start) -> Option<String> {
    match start {
        Start::Off | Start::ReadOnly(_) => None,
        Start::Unreachable(why) => Some(tr!("Google Drive did not answer — opened as it is, unchecked: {why}", "Google Drive не ответил — открыт как есть, без проверки: {why}")),
        Start::Writer { took_from, pulled, conflict } => {
            let mut parts = Vec::new();
            if let Some(device) = took_from {
                parts.push(tr!("took writing over from {device}", "запись забрана у {device}"));
            }
            if let Some(device) = pulled {
                parts.push(tr!("took the newer copy from {device}", "взята свежая копия от {device}"));
            }
            if let Some(name) = conflict {
                parts.push(tr!("this Mac's own changes are kept in Drive as {name}", "свои правки этого мака лежат в Drive как {name}"));
            }
            (!parts.is_empty()).then(|| parts.join(" · "))
        }
    }
}

/// Before the ledger opens: who writes, and is there something newer.
pub fn start(db: &Path, take_over: bool) -> Start {
    if !applies_to(db) {
        return Start::Off;
    }
    match start_checked(db, take_over) {
        Ok(start) => start,
        Err(error) => Start::Unreachable(format!("{error:#}")),
    }
}

fn start_checked(db: &Path, take_over: bool) -> Result<Start> {
    let conn = potok_core::db::open(db)?;
    let me = device_name(&conn);
    let drive = Drive::connect(&conn)?;
    let folder = drive.folder()?;
    let mark = drive.mark(&folder)?;
    let mut took_from = None;
    if let Some((_, other)) = mark.as_ref().filter(|(_, m)| m.device != me) {
        if !take_over && !other.stale() {
            return Ok(Start::ReadOnly(other.clone()));
        }
        took_from = Some(other.device.clone());
    }
    drive.put_mark(&folder, mark.as_ref().map(|(id, _)| id.as_str()), &Mark { device: me.clone(), since: chrono::Utc::now().to_rfc3339() })?;

    let copies = drive.copies(&folder)?;
    let base = generation(&conn)?.unwrap_or(0);
    let Some(newer) = newest(&copies).filter(|r| r.generation > base).cloned() else {
        return Ok(Start::Writer { took_from, pulled: None, conflict: None });
    };
    let conflict = if dirty(&conn)? && potok_core::journal::last_id(&conn)? > 0 { Some(send_conflict(&conn, db, &drive, &folder, &me)?) } else { None };
    pull(conn, db, &drive, &newer)?;
    Ok(Start::Writer { took_from, pulled: Some(if newer.device.is_empty() { "?".into() } else { newer.device.clone() }), conflict })
}

/// Put a copy from Drive in place of this ledger. This ledger is backed up
/// first, and the settings that belong to this Mac stay this Mac's.
///
/// Through SQLite's own backup into the open ledger, not by writing over the
/// file: another connection on it — a second window — then sees the change
/// or waits, where a file written under it is a corrupt ledger. Found by the
/// two-device trial, which did exactly that to its copy.
fn pull(conn: rusqlite::Connection, db: &Path, drive: &Drive, remote: &Remote) -> Result<()> {
    let dir = potok_core::backup::dir(&conn, db)?;
    std::fs::create_dir_all(&dir)?;
    let staged = dir.join(format!(".{}", safe_name(&remote.name)));
    drive.download(&remote.id, &staged)?;
    let sealed = std::fs::read(&staged);
    let _ = std::fs::remove_file(&staged);
    let passphrase = crate::secrets::get(KEY_PASSPHRASE)?.context(tr!("no encryption passphrase — potok cloud login", "нет пароля шифрования — potok cloud login"))?;
    let plain = unseal(&sealed?, &passphrase)?;
    let check = dir.join(".pulled.db");
    potok_core::db::write_private(&check, &plain)?;
    let placed = potok_core::backup::make(&conn, &dir).and_then(|_| {
        let device = if remote.device.is_empty() { "?" } else { remote.device.as_str() };
        potok_core::backup::put_in_place(conn, db, &check, &tr!("taken from Google Drive: {} ({device})", "взято из Google Drive: {} ({device})", remote.name))
    });
    let _ = std::fs::remove_file(&check);
    let conn = placed?;
    synced(&conn, remote.generation)
}

/// On leaving: send what changed, and let go of the mark if it is ours.
pub fn finish(db: &Path) -> Result<Option<String>> {
    if !applies_to(db) {
        return Ok(None);
    }
    let conn = potok_core::db::open(db)?;
    let drive = Drive::connect(&conn)?;
    let folder = drive.folder()?;
    let here = device_name(&conn);
    let mark = drive.mark(&folder)?;
    if let Some((_, ref m)) = mark {
        if m.device != here {
            return Ok(None);
        }
    }
    let sent = if dirty(&conn)? { Some(push(&conn, db)?) } else { None };
    if let Some((id, _)) = mark {
        drive.delete(&id)?;
    }
    Ok(sent)
}

/// Who writes now, for `potok cloud`.
pub fn writer(conn: &rusqlite::Connection) -> Result<Option<Mark>> {
    let drive = Drive::connect(conn)?;
    let folder = drive.folder()?;
    Ok(drive.mark(&folder)?.map(|(_, m)| m))
}

/// The start check, off the screen's way: potok opens at once and the answer
/// — who writes, was there something newer — arrives when Drive gives one.
/// `None` when Drive is not set up for this ledger: nothing to wait for.
///
/// What it costs to do this late: a few seconds in which the ledger is open
/// for writing before a mark of another device is known. An edit made in
/// those seconds is not lost — the next start finds this ledger changed and
/// puts it in Drive beside the newer copy — but it is made on a ledger this
/// device did not yet have the right to write. With one Mac that window
/// never contains anything; with two it is the price of opening now.
pub fn start_in_background(db: &Path) -> Option<std::sync::mpsc::Receiver<Start>> {
    if !applies_to(db) {
        return None;
    }
    let (send, receive) = std::sync::mpsc::channel();
    let db = db.to_path_buf();
    std::thread::spawn(move || {
        let start = start(&db, false);
        // The daily copy, but only where this device writes, and after the
        // check: a send must not race the start that decides the generation.
        if matches!(start, Start::Writer { .. }) {
            push_if_due(&db);
        }
        let _ = send.send(start);
    });
    Some(receive)
}

/// Send a copy if the last one is a day old. A failure is written to the
/// ledger for Settings and Home to show; the next start tries again.
fn push_if_due(db: &Path) {
    let Ok(conn) = potok_core::db::open(db) else { return };
    let _ = potok_core::settings::apply_language(&conn);
    let fresh = potok_core::settings::get(&conn, potok_core::settings::CLOUD_SENT)
        .ok()
        .flatten()
        .and_then(|at| chrono::NaiveDateTime::parse_from_str(&at, "%Y-%m-%d %H:%M").ok())
        .is_some_and(|at| chrono::Local::now().naive_local() - at < chrono::Duration::hours(24));
    if fresh || !dirty(&conn).unwrap_or(true) {
        return;
    }
    let _ = push(&conn, db);
}

pub fn list(conn: &rusqlite::Connection) -> Result<Vec<Remote>> {
    let drive = Drive::connect(conn)?;
    let folder = drive.folder()?;
    drive.copies(&folder)
}

/// Download one copy (or the newest), open it, check it is a ledger, and put
/// it with the local backups — `potok restore <file>` takes it from there.
pub fn get(conn: &rusqlite::Connection, db: &Path, which: Option<&str>) -> Result<PathBuf> {
    let drive = Drive::connect(conn)?;
    let folder = drive.folder()?;
    let remote = drive.copies(&folder)?;
    let chosen = match which {
        None | Some("latest") => newest(&remote).or_else(|| remote.first()),
        Some(name) => remote.iter().find(|r| r.name == name || r.name.trim_end_matches(SUFFIX) == name),
    }
    .context(tr!("no such copy in Google Drive — potok cloud list", "такой копии в Google Drive нет — potok cloud list"))?;
    let dir = potok_core::backup::dir(conn, db)?;
    std::fs::create_dir_all(&dir)?;
    let staged = dir.join(format!(".{}", safe_name(&chosen.name)));
    drive.download(&chosen.id, &staged)?;
    let sealed = std::fs::read(&staged);
    let _ = std::fs::remove_file(&staged);
    let passphrase = match crate::secrets::get(KEY_PASSPHRASE)? {
        Some(p) => p,
        None => prompt(&tr!("Encryption passphrase: ", "Пароль шифрования: "), true)?,
    };
    let plain = unseal(&sealed?, &passphrase)?;
    let target = dir.join(safe_name(chosen.name.trim_end_matches(SUFFIX)).replacen("ledger-", "ledger-cloud-", 1));
    potok_core::db::write_private(&target, &plain)?;
    rusqlite::Connection::open_with_flags(&target, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|c| c.query_row("select count(*) from operation", [], |r| r.get::<_, i64>(0)))
        .with_context(|| tr!("the copy opened but is not a potok ledger", "копия открылась, но это не леджер potok"))?;
    Ok(target)
}

/// Read a line; with `hidden`, without echo — `stty` rather than a crate, it
/// is on every Mac.
fn prompt(question: &str, hidden: bool) -> Result<String> {
    print!("{question}");
    std::io::stdout().flush()?;
    let echo = |on: bool| {
        let _ = Command::new("stty").arg(if on { "echo" } else { "-echo" }).stdin(Stdio::inherit()).status();
    };
    if hidden {
        echo(false);
    }
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if hidden {
        echo(true);
        println!();
    }
    read?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_copy_opens_with_its_passphrase_only() {
        let plain = b"SQLite format 3\0 the whole ledger";
        let sealed = seal(plain, "correct horse battery").unwrap();
        assert!(sealed.starts_with(MAGIC));
        assert!(!sealed.windows(6).any(|w| w == b"ledger"), "nothing of the ledger is readable");
        assert_eq!(unseal(&sealed, "correct horse battery").unwrap(), plain);
        assert!(unseal(&sealed, "correct horse batterY").is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(unseal(&tampered, "correct horse battery").is_err(), "a changed byte is caught");
        assert_ne!(seal(plain, "correct horse battery").unwrap(), sealed, "salt and nonce are fresh each time");
        assert!(unseal(b"SQLite format 3", "x").is_err());
    }

    /// The real Keychain and the real Google, so not in the normal run:
    /// `cargo test -- --ignored cloud`. Writes one item and removes it.
    #[test]
    #[ignore]
    fn cloud_the_keychain_keeps_a_secret_and_google_names_a_bad_client() {
        let account = "selftest-delete-me";
        let secret = "пароль \"с\" кавычками и пробелами";
        crate::secrets::put(account, secret).unwrap();
        assert_eq!(crate::secrets::get(account).unwrap().as_deref(), Some(secret));
        crate::secrets::forget(account);
        assert_eq!(crate::secrets::get(account).unwrap(), None);

        let error = curl(&[("url", "https://oauth2.googleapis.com/token"), ("data", "grant_type=refresh_token&client_id=nobody&client_secret=x&refresh_token=y")])
            .unwrap_err()
            .to_string();
        assert!(error.to_lowercase().contains("client") && !error.contains('{'), "Google's own words, one line: {error}");
    }

    fn remote(name: &str, generation: u64, conflict: bool) -> Remote {
        Remote { id: name.into(), name: name.into(), size: 0, created: format!("2026-10-19T00:00:0{generation}Z"), generation, device: "A".into(), conflict }
    }

    /// The rules the two-device protocol rests on: what counts as a change,
    /// which copy is the newest, when a mark has been left behind.
    #[test]
    fn changes_generations_and_marks_are_judged_by_the_rules() {
        let conn = potok_core::db::open_memory().unwrap();
        assert!(dirty(&conn).unwrap(), "never synced is changed");
        synced(&conn, 3).unwrap();
        assert!(!dirty(&conn).unwrap(), "just synced");
        potok_core::settings::set(&conn, potok_core::settings::THEME, "light").unwrap();
        assert!(!dirty(&conn).unwrap(), "a setting is not a change to send");
        conn.execute("insert into account (name,currency,kind) values ('Cash','EUR','CASH')", []).unwrap();
        potok_core::account::rename(&conn, conn.last_insert_rowid(), "Wallet").unwrap();
        assert!(dirty(&conn).unwrap(), "a journal entry is");

        let copies = [remote("a.potok", 2, false), remote("b.potok", 5, true), remote("c.potok", 4, false), remote("old.potok", 0, false)];
        assert_eq!(newest(&copies).map(|r| r.name.as_str()), Some("c.potok"), "a conflict copy is never the newest");
        assert_eq!(newest(&[]).map(|r| r.generation), None);

        let fresh = Mark { device: "A".into(), since: chrono::Utc::now().to_rfc3339() };
        let left = Mark { device: "A".into(), since: (chrono::Utc::now() - chrono::Duration::hours(13)).to_rfc3339() };
        assert!(!fresh.stale() && left.stale());
        assert!(Mark { device: "A".into(), since: "garbage".into() }.stale(), "an unreadable mark does not hold anyone out");
        assert_eq!(slug("Someone's MacBook Pro"), "Someone-s-MacBook-Pro");

        let note = start_note(&Start::Writer { took_from: Some("A".into()), pulled: Some("A".into()), conflict: Some("x.potok".into()) }).unwrap();
        assert!(note.contains("took writing over from A") && note.contains("x.potok"), "{note}");
        assert_eq!(start_note(&Start::Writer { took_from: None, pulled: None, conflict: None }), None);
    }

    /// A name in Drive is somebody else's string: it becomes a file name
    /// here and a value in curl's config, and both have ways out.
    #[test]
    fn a_name_from_drive_cannot_leave_the_folder_or_add_a_curl_option() {
        assert_eq!(safe_name("ledger-2026-10-20-083712-721.db.potok"), "ledger-2026-10-20-083712-721.db.potok");
        assert_eq!(safe_name("x/../../../y"), "y");
        assert_eq!(safe_name("../../.zshrc"), "zshrc");
        assert_eq!(safe_name("..\\..\\win"), "win");
        assert_eq!(safe_name("/"), "copy.db");
        std::env::set_var("POTOK_CLOUD_FOLDER", "o'brien's trial");
        assert_eq!(folder_name(), "o-brien-s trial", "a quote never sends a trial to the real folder");
        std::env::remove_var("POTOK_CLOUD_FOLDER");
        assert_eq!(folder_name(), "potok-backups");
        assert!(!safe_name("a b;c$d").contains([' ', ';', '$']));
        let folder = Path::new("/tmp/potok-backups");
        assert_eq!(folder.join(format!(".{}", safe_name("x/../../../y"))).parent(), Some(folder));

        let injected = format!("ledger.db{SUFFIX}\noutput = /tmp/pwned\nurl = https://example.com/");
        let refused = curl(&[("output", &injected)]).unwrap_err().to_string();
        assert!(refused.contains("output"), "{refused}");
    }

    /// RFC 7636, appendix B.
    #[test]
    fn the_pkce_challenge_is_the_rfcs() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
    }

    /// A pairing code is the sign-in, not the passphrase: it travels, and what
    /// opens it stays in the person's head.
    #[test]
    fn a_pairing_code_carries_the_sign_in_and_needs_the_passphrase() {
        let client = Client { id: "123.apps.googleusercontent.com".into(), secret: "GOCSPX-secret".into() };
        let code = pack(&client, "1//refresh-token", "potok-backup-x1", "a long passphrase").unwrap();
        assert!(!code.contains("a long passphrase"), "the passphrase must not be in the code");
        assert!(!code.contains("refresh"), "the code is ciphertext");

        let (same, refresh, project) = unpack(&code, "a long passphrase").unwrap();
        assert_eq!((same.id, same.secret, refresh), (client.id, client.secret, "1//refresh-token".to_string()));
        // The project travels: gcloud's credentials are refused without it,
        // and the other device has no gcloud to ask for it.
        assert_eq!(project, "potok-backup-x1");

        // A code from a device with a client of its own carries no project.
        let plain = pack(&Client { id: "a".into(), secret: "b".into() }, "c", "", "pass").unwrap();
        assert_eq!(unpack(&plain, "pass").unwrap().2, "");

        // A code read out and wrapped by a terminal still works; a wrong
        // passphrase and a truncated code do not.
        let wrapped = format!("{}\n  {}", &code[..20], &code[20..]);
        assert!(unpack(&wrapped, "a long passphrase").is_ok());
        assert!(unpack(&code, "another passphrase").is_err());
        assert!(unpack(&code[..code.len() - 8], "a long passphrase").is_err());
        assert!(unpack("not a code at all!", "a long passphrase").is_err());
    }

    #[test]
    fn base64url_goes_both_ways_at_every_length() {
        for n in 0..32 {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 7 + 13) as u8).collect();
            assert_eq!(from_base64url(&base64url(&bytes)).unwrap(), bytes, "{n} bytes");
        }
    }

    #[test]
    fn the_redirect_is_read_and_values_travel_encoded() {
        let line = "GET /?state=a%2Bb&code=4%2F0Ab_x&scope=https%3A%2F%2Fwww.googleapis.com HTTP/1.1\r\n";
        assert_eq!(query_value(line, "code").as_deref(), Some("4/0Ab_x"));
        assert_eq!(query_value(line, "state").as_deref(), Some("a+b"));
        assert_eq!(query_value(line, "error"), None);
        assert_eq!(encode("a b/+é"), "a%20b%2F%2B%C3%A9");
        assert_eq!(quoted(r#"say "hi" \ bye"#), r#""say \"hi\" \\ bye""#);
    }
}
