//! Tests with key files, the key agent and shortcuts, against an OpenSSH server that the developer
//! starts on this computer with throw-away test keys.
//!
//! They are marked `#[ignore]` and quietly do nothing unless these environment variables are set:
//!
//! * `MINI_REMOTE_TEST_LOCAL_SSH_KEYS`: folder holding the test keys `id_plain` (no passphrase),
//!   `id_locked` (passphrase `test-passphrase`), `id_rsa`, `id_ecdsa`, `id_rsa_pem` and `id_other`
//!   (the last one is NOT allowed to log in).
//! * `MINI_REMOTE_TEST_LOCAL_SSH_PORT`: port of the server (the server listens on 127.0.0.1 only).
//! * `MINI_REMOTE_TEST_LOCAL_SSH_WORK`: an empty scratch folder the tests may fill and delete.
//! * `MINI_REMOTE_TEST_LOCAL_SSH_AGENT=1` (optional): a key agent holding `id_plain` is running.
//! * `MINI_REMOTE_TEST_LOCAL_SSH_NOSFTP_PORT` (optional): port of a second server with file transfer
//!   switched off.
//! * `MINI_REMOTE_TEST_LOCAL_SSH_GOODBYE_PORT` and `MINI_REMOTE_TEST_LOCAL_SSH_GOODBYE_LOG` (optional): port
//!   and log file (log level INFO) of a server that only the goodbye test uses, to check that
//!   dropping the last handle says goodbye properly.
//!
//! The account name is taken from `USER`.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ssh::{CancelToken, HostKeyCheck, Progress, SshAuth, SshConfig, SshError, SshSession};

struct Setup {
    keys: PathBuf,
    port: u16,
    work: PathBuf,
}

fn setup() -> Option<Setup> {
    let keys = PathBuf::from(std::env::var_os("MINI_REMOTE_TEST_LOCAL_SSH_KEYS")?);
    let port = std::env::var("MINI_REMOTE_TEST_LOCAL_SSH_PORT")
        .ok()?
        .parse()
        .ok()?;
    let work = PathBuf::from(std::env::var_os("MINI_REMOTE_TEST_LOCAL_SSH_WORK")?);
    Some(Setup { keys, port, work })
}

macro_rules! setup_or_skip {
    () => {
        match setup() {
            Some(s) => s,
            None => {
                println!("skipped: the local test server is not set up");
                return;
            }
        }
    };
}

fn accept_all() -> HostKeyCheck {
    Arc::new(|_| Box::pin(async { true }))
}

fn config(s: &Setup, auth: SshAuth) -> SshConfig {
    SshConfig {
        host: "127.0.0.1".into(),
        port: s.port,
        username: std::env::var("USER").expect("USER is set"),
        auth,
    }
}

fn key(s: &Setup, name: &str, passphrase: Option<&str>) -> SshAuth {
    SshAuth::KeyFile {
        path: s.keys.join(name),
        passphrase: passphrase.map(str::to_string),
    }
}

fn no_progress() -> Progress {
    Box::new(|_, _| {})
}

/// The server refuses logins that arrive too many at once, and (OpenSSH 9.8 and newer) briefly
/// turns away an address that has just failed to log in. So the tests log in one by one, and wait
/// a few seconds after a failed login before the next one may start.
static LOGIN_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn login(cfg: SshConfig) -> Result<SshSession, SshError> {
    let _turn = LOGIN_TURN.lock().await;
    let result = SshSession::connect(cfg, accept_all()).await;
    if matches!(
        result,
        Err(SshError::AuthFailed | SshError::HostKeyRejected)
    ) {
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    }
    result
}

async fn login_with_key(
    s: &Setup,
    name: &str,
    passphrase: Option<&str>,
) -> Result<SshSession, SshError> {
    login(config(s, key(s, name, passphrase))).await
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn every_kind_of_key_file_works() {
    let s = setup_or_skip!();
    for name in ["id_plain", "id_rsa", "id_ecdsa", "id_rsa_pem"] {
        let session = login_with_key(&s, name, None)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let sftp = session
            .sftp()
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(sftp.home_dir().await.unwrap().starts_with('/'));
    }
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn locked_key_needs_the_right_passphrase() {
    let s = setup_or_skip!();
    let err = login_with_key(&s, "id_locked", None)
        .await
        .expect_err("no passphrase");
    assert!(matches!(err, SshError::PassphraseNeeded), "{err:?}");
    println!("{err}");
    let err = login_with_key(&s, "id_locked", Some("wrong"))
        .await
        .expect_err("wrong passphrase");
    assert!(matches!(err, SshError::WrongPassphrase), "{err:?}");
    println!("{err}");
    login_with_key(&s, "id_locked", Some("test-passphrase"))
        .await
        .expect("right passphrase");
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn a_key_the_server_does_not_know_is_refused() {
    let s = setup_or_skip!();
    let err = login_with_key(&s, "id_other", None)
        .await
        .expect_err("unknown key");
    assert!(matches!(err, SshError::AuthFailed), "{err:?}");
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn a_file_that_is_not_a_key_is_reported_plainly() {
    let s = setup_or_skip!();
    let junk = s.work.join("junk-key");
    std::fs::create_dir_all(&s.work).unwrap();
    std::fs::write(&junk, "this is not a key\n").unwrap();
    let auth = SshAuth::KeyFile {
        path: junk.clone(),
        passphrase: None,
    };
    let err = login(config(&s, auth)).await.expect_err("junk");
    assert!(matches!(err, SshError::KeyFile(_)), "{err:?}");
    println!("{err}");
    let _ = std::fs::remove_file(junk);
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn key_agent_login() {
    let s = setup_or_skip!();
    if std::env::var("MINI_REMOTE_TEST_LOCAL_SSH_AGENT").is_err() {
        println!("skipped: no test key agent");
        return;
    }
    let session = login(config(&s, SshAuth::Agent))
        .await
        .expect("agent login");
    assert!(session.sftp().await.is_ok());
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn tunnel_reaches_a_local_port() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let s = setup_or_skip!();
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = echo.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = echo.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                while let Ok(n) = sock.read(&mut buf).await {
                    if n == 0 || sock.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    let mut tunnel = session.open_tunnel("127.0.0.1", port).await.unwrap();
    // Push a few megabytes through and check that they come back unchanged and in order.
    let data: Vec<u8> = (0..3_000_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
        .collect();
    let (mut rd, mut wr) = tokio::io::split(&mut tunnel);
    let send = data.clone();
    let writer = async move {
        wr.write_all(&send).await.unwrap();
        wr.flush().await.unwrap();
    };
    let reader = async {
        let mut got = vec![0u8; data.len()];
        rd.read_exact(&mut got).await.unwrap();
        got
    };
    let (_, got) = tokio::join!(writer, reader);
    assert!(got == data, "bytes came back different");
}

/// Makes sure the scratch folder is empty and returns a fresh sub-folder in it.
fn fresh(s: &Setup, name: &str) -> PathBuf {
    let dir = s.work.join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn p(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn shortcuts_are_listed_copied_and_deleted_safely() {
    let s = setup_or_skip!();
    let base = fresh(&s, "links");
    let outside = base.join("outside");
    let work = base.join("work");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(work.join("real")).unwrap();
    std::fs::write(outside.join("precious.txt"), b"keep me").unwrap();
    std::fs::write(work.join("real/f.txt"), b"12345").unwrap();
    std::os::unix::fs::symlink(&outside, work.join("link_dir")).unwrap();
    std::os::unix::fs::symlink("real/f.txt", work.join("link_file")).unwrap();
    std::os::unix::fs::symlink("nowhere", work.join("broken")).unwrap();

    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    let sftp = session.sftp().await.unwrap();

    let entries = sftp.list_dir(p(&work)).await.unwrap();
    let by_name = |n: &str| {
        entries
            .iter()
            .find(|e| e.name == n)
            .unwrap_or_else(|| panic!("{n} missing"))
    };
    assert!(by_name("link_dir").is_dir && by_name("link_dir").is_symlink);
    assert!(!by_name("link_file").is_dir && by_name("link_file").is_symlink);
    assert_eq!(
        by_name("link_file").size,
        5,
        "size of what the shortcut leads to"
    );
    assert!(!by_name("broken").is_dir && by_name("broken").is_symlink);
    assert!(by_name("real").is_dir && !by_name("real").is_symlink);
    // Folders (including the shortcut to a folder) come first.
    assert!(entries[0].is_dir && entries[1].is_dir && !entries[2].is_dir);

    // Copying the folder down skips shortcuts to folders and broken ones, copies shortcuts to files.
    let down = fresh(&s, "links-down");
    sftp.download_dir(p(&work), &down, no_progress(), CancelToken::default())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(down.join("work/real/f.txt")).unwrap(),
        b"12345"
    );
    assert_eq!(
        std::fs::read(down.join("work/link_file")).unwrap(),
        b"12345"
    );
    assert!(!down.join("work/link_dir").exists());
    assert!(!down.join("work/broken").exists());

    // Same going up.
    let up_remote = fresh(&s, "links-up");
    sftp.upload_dir(&work, p(&up_remote), no_progress(), CancelToken::default())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(up_remote.join("work/link_file")).unwrap(),
        b"12345"
    );
    assert!(!up_remote.join("work/link_dir").exists());

    // Deleting a shortcut to a folder leaves the folder; deleting the work folder never follows it.
    sftp.remove(p(&work.join("link_dir"))).await.unwrap();
    assert!(outside.join("precious.txt").exists());
    std::os::unix::fs::symlink(&outside, work.join("link_dir")).unwrap();
    sftp.remove(p(&work)).await.unwrap();
    assert!(!work.exists());
    assert!(
        outside.join("precious.txt").exists(),
        "the target of a shortcut must survive"
    );
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&down);
    let _ = std::fs::remove_dir_all(&up_remote);
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn program_files_stay_runnable_both_ways() {
    let s = setup_or_skip!();
    let dir = fresh(&s, "perms");
    let script = dir.join("run.sh");
    std::fs::write(&script, b"#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    let sftp = session.sftp().await.unwrap();
    let remote = dir.join("uploaded.sh");
    sftp.upload(&script, p(&remote), no_progress(), CancelToken::default())
        .await
        .unwrap();
    let entry = sftp
        .list_dir(p(&dir))
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.name == "uploaded.sh")
        .unwrap();
    assert_eq!(entry.permissions, Some(0o755));

    let back = dir.join("downloaded.sh");
    sftp.download(p(&remote), &back, no_progress(), CancelToken::default())
        .await
        .unwrap();
    assert_eq!(
        std::fs::metadata(&back).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn download_failures_leave_nothing_behind() {
    let s = setup_or_skip!();
    let dir = fresh(&s, "failures");
    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    let sftp = session.sftp().await.unwrap();

    // A file that does not exist.
    let target = dir.join("never.txt");
    let err = sftp
        .download(
            p(&dir.join("missing.txt")),
            &target,
            no_progress(),
            CancelToken::default(),
        )
        .await
        .expect_err("missing");
    assert!(err.to_string().contains("not found"), "{err}");
    // A folder given instead of a file.
    let err = sftp
        .download(p(&dir), &target, no_progress(), CancelToken::default())
        .await
        .expect_err("folder");
    assert!(matches!(err, SshError::Sftp(_)), "{err:?}");
    // A target folder that does not exist.
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    let err = sftp
        .download(
            p(&dir.join("a.txt")),
            &dir.join("no-such-folder/x.txt"),
            no_progress(),
            CancelToken::default(),
        )
        .await
        .expect_err("no target folder");
    assert!(matches!(err, SshError::LocalFile(_)), "{err:?}");
    println!("{err}");
    // Uploading a file that does not exist.
    let err = sftp
        .upload(
            &dir.join("absent"),
            p(&dir.join("x")),
            no_progress(),
            CancelToken::default(),
        )
        .await
        .expect_err("absent");
    assert!(matches!(err, SshError::LocalFile(_)), "{err:?}");

    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["a.txt"], "no temporary files left");
    let _ = std::fs::remove_dir_all(&dir);
}

fn goodbyes_in(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .matches("Received disconnect")
        .count()
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn dropping_the_last_handle_says_goodbye() {
    let mut s = setup_or_skip!();
    let (Some(port), Some(log)) = (
        std::env::var("MINI_REMOTE_TEST_LOCAL_SSH_GOODBYE_PORT")
            .ok()
            .and_then(|p| p.parse().ok()),
        std::env::var_os("MINI_REMOTE_TEST_LOCAL_SSH_GOODBYE_LOG").map(PathBuf::from),
    ) else {
        println!("skipped: no server dedicated to this test given");
        return;
    };
    s.port = port;
    let before = goodbyes_in(&log);

    // Dropping the session handle.
    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(goodbyes_in(&log), before + 1, "plain session");

    // Dropping the last tunnel / file session that was keeping the login open.
    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    let sftp = session.sftp().await.unwrap();
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        goodbyes_in(&log),
        before + 1,
        "still open while a file session exists"
    );
    drop(sftp);
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert_eq!(
        goodbyes_in(&log),
        before + 2,
        "after the last handle is gone"
    );
}

#[tokio::test]
#[ignore = "needs a local test server"]
async fn missing_file_transfer_service_is_reported_plainly() {
    let mut s = setup_or_skip!();
    let Some(port) = std::env::var("MINI_REMOTE_TEST_LOCAL_SSH_NOSFTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
    else {
        println!("skipped: no server without file transfer given");
        return;
    };
    s.port = port;
    let session = login_with_key(&s, "id_plain", None).await.unwrap();
    let started = std::time::Instant::now();
    let err = session.sftp().await.expect_err("no file transfer here");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "must not hang"
    );
    assert!(matches!(err, SshError::Sftp(_)), "{err:?}");
    println!("{err}");
    // The login is still usable for tunnels.
    assert!(!session.is_closed());
}
