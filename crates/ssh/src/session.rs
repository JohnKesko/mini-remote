//! Logging in to the remote computer and opening tunnels through that login.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use russh::client::{self, AuthResult, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::key::PrivateKeyWithHashAlg;
use russh::keys::{Algorithm, HashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, ChannelOpenFailure, ChannelStream, Disconnect, MethodKind};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::error::SshError;
use crate::sftp::Sftp;

/// How long to wait for the network connection to the remote computer to be made.
const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the secure handshake may stay silent (time spent asking the user does not count).
const HANDSHAKE_SILENCE_LIMIT: Duration = Duration::from_secs(20);
/// How long the remote computer may take to answer one login attempt.
const AUTH_TIMEOUT: Duration = Duration::from_secs(60);
/// Seconds of silence after which a keepalive message is sent.
const KEEPALIVE_SECS: u64 = 15;

/// Where and how to log in.
#[derive(Clone)]
pub struct SshConfig {
    /// Name or address of the remote computer.
    pub host: String,
    /// Port of the remote login service (normally 22).
    pub port: u16,
    /// Account name on the remote computer.
    pub username: String,
    /// How to prove who you are.
    pub auth: SshAuth,
}

impl fmt::Debug for SshConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth", &self.auth)
            .finish()
    }
}

/// The way to prove who you are to the remote computer.
#[derive(Clone)]
pub enum SshAuth {
    /// A password.
    Password(String),
    /// A private key file, with the passphrase that unlocks it (if it has one).
    KeyFile {
        /// Where the private key file is.
        path: PathBuf,
        /// Passphrase for the key file, if it is locked.
        passphrase: Option<String>,
    },
    /// Keys held by the running key agent (the `SSH_AUTH_SOCK` helper on Mac and Linux).
    Agent,
}

impl fmt::Debug for SshAuth {
    // Never print secrets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshAuth::Password(_) => f.write_str("Password(<hidden>)"),
            SshAuth::KeyFile { path, passphrase } => f
                .debug_struct("KeyFile")
                .field("path", path)
                .field("passphrase", &passphrase.as_ref().map(|_| "<hidden>"))
                .finish(),
            SshAuth::Agent => f.write_str("Agent"),
        }
    }
}

/// Identity of the remote computer, shown to the user so they can decide whether to trust it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyInfo {
    /// Name or address that was connected to.
    pub host: String,
    /// Port that was connected to.
    pub port: u16,
    /// Kind of key, for example `ssh-ed25519`.
    pub algorithm: String,
    /// OpenSSH style fingerprint: `SHA256:` followed by base64 without padding.
    pub fingerprint: String,
}

/// Called while connecting so the app can compare the remote computer's identity with what the
/// user trusted before (and ask the user if it is new or changed). Return `true` to continue.
pub type HostKeyCheck =
    Arc<dyn Fn(HostKeyInfo) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Lists the private key files that exist in the user's `.ssh` folder, most modern first.
pub fn default_key_files() -> Vec<PathBuf> {
    let Some(home) = home_folder() else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|name| home.join(".ssh").join(name))
        .filter(|p| p.is_file())
        .collect()
}

/// The current user's home folder, taken from the environment.
fn home_folder() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Turns a leading `~` into the home folder, the way a shell would.
fn expand_home(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(home) = home_folder()
    {
        return home.join(rest);
    }
    path.to_path_buf()
}

/// Receives events from the secure connection; we only care about the host key.
struct Handler {
    host: String,
    port: u16,
    check: HostKeyCheck,
    asking_user: Arc<AtomicBool>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let info = host_key_info(&self.host, self.port, key);
        self.asking_user.store(true, Ordering::SeqCst);
        let ok = (self.check)(info).await;
        self.asking_user.store(false, Ordering::SeqCst);
        Ok(ok)
    }
}

/// Builds the identity shown to the user from the key the server presented.
fn host_key_info(host: &str, port: u16, key: &PublicKeyOrCertificate) -> HostKeyInfo {
    let (algorithm, fingerprint) = match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => (
            key.algorithm().to_string(),
            key.fingerprint(HashAlg::Sha256).to_string(),
        ),
        PublicKeyOrCertificate::Certificate(cert) => (
            cert.algorithm().to_string(),
            cert.public_key().fingerprint(HashAlg::Sha256).to_string(),
        ),
    };
    HostKeyInfo {
        host: host.to_string(),
        port,
        algorithm,
        fingerprint,
    }
}

struct Inner {
    handle: Option<client::Handle<Handler>>,
    closed: AtomicBool,
}

impl Inner {
    fn handle(&self) -> &client::Handle<Handler> {
        self.handle.as_ref().expect("handle is only taken on drop")
    }
}

impl Drop for Inner {
    // Say goodbye properly when the last copy of the session goes away.
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        if self.closed.load(Ordering::SeqCst) || handle.is_closed() {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

/// A logged-in connection to a remote computer. Cheap to clone; the connection is closed when
/// the last copy (and every tunnel and file transfer session made from it) is gone.
#[derive(Clone)]
pub struct SshSession {
    inner: Arc<Inner>,
}

impl fmt::Debug for SshSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshSession")
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl SshSession {
    /// Connects, checks the remote computer's identity with `check`, and logs in.
    pub async fn connect(cfg: SshConfig, check: HostKeyCheck) -> Result<SshSession, SshError> {
        let host = cfg
            .host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let tcp = connect_tcp(&host, cfg.port).await?;

        let asking_user = Arc::new(AtomicBool::new(false));
        let handler = Handler {
            host: host.clone(),
            port: cfg.port,
            check,
            asking_user: asking_user.clone(),
        };
        let config = Arc::new(client_config());

        // The handshake can pause while the user is asked about a new host key. Only give up when
        // it is silent for a long time and nobody is being asked.
        let handshake = client::connect_stream(config, tcp, handler);
        tokio::pin!(handshake);
        let mut handle = loop {
            tokio::select! {
                result = &mut handshake => break result?,
                _ = tokio::time::sleep(HANDSHAKE_SILENCE_LIMIT) => {
                    if !asking_user.load(Ordering::SeqCst) {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "handshake timed out").into());
                    }
                }
            }
        };

        authenticate(&mut handle, &cfg).await?;
        log::debug!("logged in to {}:{}", host, cfg.port);
        Ok(SshSession {
            inner: Arc::new(Inner {
                handle: Some(handle),
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Opens a tunnel to `host:port` as seen from the remote computer (for example `localhost`
    /// and `5900` for the screen-sharing server on the Pi itself).
    pub async fn open_tunnel(&self, host: &str, port: u16) -> Result<TunnelStream, SshError> {
        if self.is_closed() {
            return Err(SshError::Closed);
        }
        let channel = self
            .inner
            .handle()
            .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0)
            .await
            .map_err(|e| match e {
                russh::Error::ChannelOpenFailure(why) => SshError::TunnelRefused {
                    host: host.to_string(),
                    port,
                    reason: match why {
                        ChannelOpenFailure::ConnectFailed => {
                            "nothing answered there. Check that the program you want to reach is running on the remote computer.".to_string()
                        }
                        ChannelOpenFailure::AdministrativelyProhibited => {
                            "the remote computer does not allow tunnels (port forwarding is switched off in its login settings).".to_string()
                        }
                        ChannelOpenFailure::ResourceShortage => {
                            "the remote computer has too many connections open.".to_string()
                        }
                        _ => "the remote computer refused.".to_string(),
                    },
                },
                other => other.into(),
            })?;
        Ok(TunnelStream {
            stream: channel.into_stream(),
            _session: self.clone(),
        })
    }

    /// Starts a file transfer session (SFTP) over this login.
    pub async fn sftp(&self) -> Result<Sftp, SshError> {
        if self.is_closed() {
            return Err(SshError::Closed);
        }
        let mut channel = self.inner.handle().channel_open_session().await?;
        let no_sftp = || {
            SshError::Sftp(
                "The remote computer does not offer file transfer. Its file transfer service (SFTP) may be switched off.".to_string(),
            )
        };
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| match e {
                russh::Error::RequestDenied => no_sftp(),
                other => other.into(),
            })?;
        // Wait for the yes or no before treating the channel as a file transfer line.
        let answer = tokio::time::timeout(AUTH_TIMEOUT, async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Success) => return Ok(()),
                    Some(ChannelMsg::Failure) => return Err(no_sftp()),
                    Some(ChannelMsg::Close | ChannelMsg::Eof) | None => {
                        return Err(SshError::Closed);
                    }
                    Some(_) => {}
                }
            }
        })
        .await;
        match answer {
            Ok(result) => result?,
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "no answer").into()),
        }
        Sftp::start(self.clone(), channel.into_stream()).await
    }

    /// Logs out and closes the connection, including every tunnel and transfer using it.
    pub async fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self
            .inner
            .handle()
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }

    /// True once the connection is closed, whether by us, by the remote computer or by the network.
    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst) || self.inner.handle().is_closed()
    }
}

/// A two-way byte stream carried through the secure connection to a port on the remote computer.
///
/// Keeps the secure connection open for as long as it exists.
pub struct TunnelStream {
    stream: ChannelStream<client::Msg>,
    _session: SshSession,
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl fmt::Debug for TunnelStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TunnelStream")
    }
}

// The VNC code needs a stream it can move between tasks, and handles that can be shared.
const _: fn() = || {
    fn assert_stream<T: AsyncRead + AsyncWrite + Send + Unpin + 'static>() {}
    fn assert_shareable<T: Send + Sync + Clone + 'static>() {}
    assert_stream::<TunnelStream>();
    assert_shareable::<SshSession>();
    assert_shareable::<Sftp>();
};

/// Settings for the secure connection: keepalives so idle tunnels survive, and roomy buffers.
fn client_config() -> client::Config {
    client::Config {
        keepalive_interval: Some(Duration::from_secs(KEEPALIVE_SECS)),
        keepalive_max: 3,
        inactivity_timeout: None,
        window_size: 4 * 1024 * 1024,
        maximum_packet_size: 64 * 1024,
        nodelay: true,
        ..Default::default()
    }
}

/// Makes the plain network connection, with a time limit.
async fn connect_tcp(host: &str, port: u16) -> Result<TcpStream, SshError> {
    let attempt = tokio::time::timeout(TCP_CONNECT_TIMEOUT, TcpStream::connect((host, port)));
    let stream = match attempt.await {
        Ok(result) => result?,
        Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out").into()),
    };
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Runs one login step with a time limit.
async fn within_limit<T>(
    fut: impl Future<Output = Result<T, russh::Error>>,
) -> Result<T, SshError> {
    match tokio::time::timeout(AUTH_TIMEOUT, fut).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "login timed out").into()),
    }
}

async fn authenticate(
    handle: &mut client::Handle<Handler>,
    cfg: &SshConfig,
) -> Result<(), SshError> {
    match &cfg.auth {
        SshAuth::Password(password) => auth_password(handle, &cfg.username, password).await,
        SshAuth::KeyFile { path, passphrase } => {
            auth_key_file(handle, &cfg.username, path, passphrase.as_deref()).await
        }
        SshAuth::Agent => auth_agent(handle, &cfg.username).await,
    }
}

async fn auth_password(
    handle: &mut client::Handle<Handler>,
    user: &str,
    password: &str,
) -> Result<(), SshError> {
    let result = within_limit(handle.authenticate_password(user, password)).await?;
    let remaining = match result {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    // Some servers only offer the "type your answer to each question" style of password login.
    if remaining.contains(&MethodKind::KeyboardInteractive) {
        return auth_keyboard_interactive(handle, user, password).await;
    }
    Err(SshError::AuthFailed)
}

async fn auth_keyboard_interactive(
    handle: &mut client::Handle<Handler>,
    user: &str,
    password: &str,
) -> Result<(), SshError> {
    let mut reply =
        within_limit(handle.authenticate_keyboard_interactive_start(user, None)).await?;
    for _ in 0..6 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Err(SshError::AuthFailed),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                // Only the password goes out, and only to questions that ask for it.
                let answers = prompts
                    .iter()
                    .map(|p| {
                        if p.prompt.to_lowercase().contains("password") {
                            password.to_string()
                        } else {
                            String::new()
                        }
                    })
                    .collect();
                reply =
                    within_limit(handle.authenticate_keyboard_interactive_respond(answers)).await?;
            }
        }
    }
    Err(SshError::AuthFailed)
}

/// Reads a private key file (the unlocking can take a moment, so it runs off the main thread).
async fn load_key(
    path: &Path,
    passphrase: Option<&str>,
) -> Result<russh::keys::PrivateKey, SshError> {
    let path = expand_home(path);
    let path = path.as_path();
    let path_owned = path.to_path_buf();
    let pass = passphrase.map(str::to_string);
    let had_passphrase = pass.is_some();
    let loaded = tokio::task::spawn_blocking(move || {
        russh::keys::load_secret_key(&path_owned, pass.as_deref())
    })
    .await
    .map_err(|e| SshError::KeyFile(format!("Reading the key file failed: {e}")))?;
    loaded.map_err(|e| classify_key_error(path, e, had_passphrase))
}

/// Explains why a key file could not be loaded.
fn classify_key_error(path: &Path, e: russh::keys::Error, had_passphrase: bool) -> SshError {
    use russh::keys::Error as K;
    match e {
        K::KeyIsEncrypted if !had_passphrase => SshError::PassphraseNeeded,
        K::KeyIsEncrypted => SshError::WrongPassphrase,
        K::IO(io) => SshError::local(path, &io),
        other => {
            let text = other.to_string();
            let lower = text.to_lowercase();
            if lower.contains("encrypt") || lower.contains("decrypt") || lower.contains("crypto") {
                if had_passphrase {
                    SshError::WrongPassphrase
                } else {
                    SshError::PassphraseNeeded
                }
            } else {
                log::debug!("key file {} was not accepted: {text}", path.display());
                SshError::KeyFile(format!(
                    "The file \"{}\" is not a private key file that this app can read.",
                    path.display()
                ))
            }
        }
    }
}

async fn auth_key_file(
    handle: &mut client::Handle<Handler>,
    user: &str,
    path: &Path,
    passphrase: Option<&str>,
) -> Result<(), SshError> {
    let key = load_key(path, passphrase).await?;
    let hash = within_limit(handle.best_supported_rsa_hash())
        .await?
        .flatten();
    let result = within_limit(
        handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)),
    )
    .await?;
    if result.success() {
        Ok(())
    } else {
        Err(SshError::AuthFailed)
    }
}

/// Opens the key agent on this computer.
async fn connect_agent() -> Result<AgentClient<Box<dyn AgentStream + Send + Unpin>>, SshError> {
    let missing = || {
        SshError::Agent(
            "No key agent was found. Start your SSH agent and add a key to it, or choose a key file or password instead.".to_string(),
        )
    };
    #[cfg(unix)]
    {
        AgentClient::connect_env()
            .await
            .map(|a| a.dynamic())
            .map_err(|_| missing())
    }
    #[cfg(windows)]
    {
        if let Ok(agent) = AgentClient::connect_pageant().await {
            return Ok(agent.dynamic());
        }
        AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .map(|a| a.dynamic())
            .map_err(|_| missing())
    }
}

async fn auth_agent(handle: &mut client::Handle<Handler>, user: &str) -> Result<(), SshError> {
    let mut agent = connect_agent().await?;
    let identities = agent.request_identities().await.map_err(|_| {
        SshError::Agent(
            "The key agent did not answer. Try again, or choose a key file or password instead."
                .to_string(),
        )
    })?;
    let mut tried_any = false;
    for identity in identities {
        let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        tried_any = true;
        let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
            within_limit(handle.best_supported_rsa_hash())
                .await?
                .flatten()
        } else {
            None
        };
        match tokio::time::timeout(
            AUTH_TIMEOUT,
            handle.authenticate_publickey_with(user, key, hash, &mut agent),
        )
        .await
        {
            Ok(Ok(result)) if result.success() => return Ok(()),
            Ok(Ok(_)) => {}
            Ok(Err(e)) => log::debug!("key agent could not sign: {e}"),
            Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "login timed out").into()),
        }
    }
    if tried_any {
        Err(SshError::AuthFailed)
    } else {
        Err(SshError::Agent(
            "The key agent holds no keys. Add one to it, or choose a key file or password instead."
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_secrets() {
        let cfg = SshConfig {
            host: "h".into(),
            port: 22,
            username: "u".into(),
            auth: SshAuth::Password("hunter2".into()),
        };
        assert!(!format!("{cfg:?}").contains("hunter2"));
        let key = SshAuth::KeyFile {
            path: "k".into(),
            passphrase: Some("hunter2".into()),
        };
        assert!(!format!("{key:?}").contains("hunter2"));
    }

    #[test]
    fn fingerprint_looks_like_openssh() {
        // A throwaway public key made just for tests; the fingerprint is what `ssh-keygen -l` prints.
        let line =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOyWZH+zcy381AhdfHyl+o8RmOJTIhYp0yCq2lZIUKaJ test";
        let key = russh::keys::PublicKey::from_openssh(line).unwrap();
        let info = host_key_info("pi.local", 22, &PublicKeyOrCertificate::from(key));
        assert_eq!(info.algorithm, "ssh-ed25519");
        assert_eq!(info.host, "pi.local");
        assert_eq!(info.port, 22);
        assert_eq!(
            info.fingerprint,
            "SHA256:W1CkR4ZIn4be9Uy96AV89pQjdAgeurKvhR7Q8cTkkmg"
        );
    }

    #[test]
    fn tilde_means_the_home_folder() {
        let plain = Path::new("/etc/some-key");
        assert_eq!(expand_home(plain), plain);
        if let Some(home) = home_folder() {
            assert_eq!(
                expand_home(Path::new("~/.ssh/id_x")),
                home.join(".ssh/id_x")
            );
        }
    }

    #[test]
    fn default_keys_are_existing_files() {
        for p in default_key_files() {
            assert!(p.is_file());
        }
    }
}
