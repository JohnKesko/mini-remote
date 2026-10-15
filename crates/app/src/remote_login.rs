//! Secure remote login (SSH) helpers shared by the screen tunnel and file
//! transfer: building the login, and asking the user about new identities.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use ssh::{HostKeyCheck, HostKeyInfo, SshAuth, SshConfig};
use tokio::sync::oneshot;

use crate::model::{Connection, SshLogin};
use crate::session::Wake;

/// A question about an SSH identity, waiting for the user's answer.
pub struct HostKeyQuestion {
    pub info: HostKeyInfo,
    /// The identity trusted before, when it changed.
    pub previous: Option<String>,
    pub answer: oneshot::Sender<bool>,
}

/// Where the network side leaves identity questions for the UI.
#[derive(Default)]
pub struct HostKeyLink {
    pending: Mutex<Option<HostKeyQuestion>>,
}

impl HostKeyLink {
    pub fn take(&self) -> Option<HostKeyQuestion> {
        self.pending.lock().take()
    }
}

/// Key in the trusted list for an SSH server.
pub fn trusted_key(host: &str, port: u16) -> String {
    format!("ssh:{}:{}", host.trim().to_lowercase(), port)
}

/// An identity check that accepts the trusted identity right away and
/// otherwise asks the user (through `link`) and waits for the answer.
pub fn host_key_check(link: Arc<HostKeyLink>, trusted: Option<String>, wake: Wake) -> HostKeyCheck {
    Arc::new(move |info: HostKeyInfo| {
        let link = link.clone();
        let trusted = trusted.clone();
        let wake = wake.clone();
        Box::pin(async move {
            if trusted.as_deref() == Some(info.fingerprint.as_str()) {
                return true;
            }
            let (tx, rx) = oneshot::channel();
            *link.pending.lock() = Some(HostKeyQuestion { info, previous: trusted, answer: tx });
            wake();
            rx.await.unwrap_or(false)
        })
    })
}

/// What is needed from the user before an SSH login can be tried.
#[derive(Debug, PartialEq, Eq)]
pub enum Missing {
    Username,
    Password,
}

/// Build the SSH login for a connection. `screen_password` is the screen
/// sharing password (used when the login says "same password"), and
/// `ssh_secret` is the separate password or key passphrase.
pub fn ssh_config(
    conn: &Connection,
    screen_password: Option<&str>,
    ssh_secret: Option<&str>,
) -> Result<SshConfig, Missing> {
    let username = conn.ssh_username().to_string();
    if username.is_empty() {
        return Err(Missing::Username);
    }
    let auth = match &conn.ssh.login {
        SshLogin::SamePassword => SshAuth::Password(screen_password.ok_or(Missing::Password)?.to_string()),
        SshLogin::Password => SshAuth::Password(ssh_secret.ok_or(Missing::Password)?.to_string()),
        SshLogin::KeyFile(path) => SshAuth::KeyFile {
            path: PathBuf::from(path),
            passphrase: ssh_secret.filter(|s| !s.is_empty()).map(str::to_string),
        },
        SshLogin::Agent => SshAuth::Agent,
    };
    Ok(SshConfig { host: conn.ssh_host().to_string(), port: conn.ssh.port, username, auth })
}

/// The login needs a password typed by the user (not a key or agent).
pub fn uses_password(conn: &Connection) -> bool {
    matches!(conn.ssh.login, SshLogin::SamePassword | SshLogin::Password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_password_needs_one() {
        let mut c = Connection { host: "pi".into(), username: "pi".into(), ..Connection::default() };
        assert_eq!(ssh_config(&c, None, None).err(), Some(Missing::Password));
        let cfg = ssh_config(&c, Some("pw"), None).unwrap();
        assert_eq!((cfg.host.as_str(), cfg.port, cfg.username.as_str()), ("pi", 22, "pi"));
        c.username.clear();
        assert_eq!(ssh_config(&c, Some("pw"), None).err(), Some(Missing::Username));
        c.ssh.username = "admin".into();
        c.ssh.login = SshLogin::Agent;
        assert!(ssh_config(&c, None, None).is_ok());
        assert_eq!(c.ssh_host_key(), trusted_key("pi", 22));
    }
}
